using System.Security.Principal;
using System.Text.Json;
using Comptroller.Service.Config;
using Microsoft.AspNetCore.Authentication;
using Microsoft.AspNetCore.Authentication.Negotiate;
using Microsoft.Extensions.Options;

namespace Comptroller.Service.Identity;

/// <summary>The service's own endpoints under /_comptroller/. See contracts/api.md.</summary>
public static class Endpoints
{
    private const string ClientHeader = "X-Comptroller-Client";

    public static void MapComptrollerEndpoints(this WebApplication app)
    {
        var options = app.Services.GetRequiredService<IOptionsMonitor<ComptrollerConfig>>();
        var store = app.Services.GetRequiredService<IdentityStore>();

        var group = app.MapGroup("/_comptroller");
        group.AddEndpointFilter(async (invocation, next) =>
            invocation.HttpContext.Request.Headers.ContainsKey(ClientHeader)
                ? await next(invocation)
                : ApiErrors.Result(400, $"Send the {ClientHeader} header."));

        group.MapGet("/connect", (Delegate)(Task<IResult> (HttpContext context) => options.CurrentValue.Identity.Mode == "windows"
            ? WindowsSignIn(context, options.CurrentValue.Identity, store, reconnect: false)
            : Task.FromResult(ApiErrors.Result(400, "This server uses one-time codes. POST the code to connect."))));

        group.MapPost("/connect", (Delegate)(async Task<IResult> (HttpContext context) =>
        {
            var identity = options.CurrentValue.Identity;
            if (identity.Mode != "manual") return ApiErrors.Result(400, "This server signs you in with Windows. Use GET.");

            var code = await ReadCode(context.Request);
            return code is null ? ManualRefresh(context, identity, store) : RedeemCode(code, identity, store);
        }));

        group.MapGet("/me", (HttpContext context) =>
        {
            if (DeviceFromToken(context, options.CurrentValue.Identity, store) is not { } device)
                return ApiErrors.Result(401, ApiErrors.NotConnected);

            return Results.Json(new
            {
                user = device.User,
                device = device.Name,
                status = device.EffectiveStatus,
                suspendedBy = device.SuspendedBy,
                expired = device.Expired,
                expiresUtc = IdentityStore.Format(device.ExpiresUtc),
                // Filled in once requests are recorded.
                today = new { requests = (int?)null, limitHits = (int?)null, lastRequestUtc = (string?)null },
            });
        });

        group.MapPost("/disconnect", (HttpContext context) =>
        {
            if (DeviceFromToken(context, options.CurrentValue.Identity, store) is not { } device)
                return ApiErrors.Result(401, ApiErrors.NotConnected);

            store.SuspendFromTray(device.Id);
            Serilog.Log.Information("{User} disconnected {Device}", device.User, device.Name);
            return Results.Json(new { status = store.FindByUserAndName(device.User, device.Name)!.EffectiveStatus });
        });

        group.MapPost("/reconnect", (Delegate)(async Task<IResult> (HttpContext context) =>
        {
            var identity = options.CurrentValue.Identity;
            if (identity.Mode == "windows") return await WindowsSignIn(context, identity, store, reconnect: true);

            if (DeviceFromToken(context, identity, store) is not { } device)
                return ApiErrors.Result(401, ApiErrors.NotConnected);

            if (!device.Expired) store.Reactivate(device.Id);
            return Session(context.Request.Headers[identity.TokenHeaderName].ToString(), device, store);
        }));

        // Unknown own paths stop here; they never reach the upstream.
        app.Map("/_comptroller/{**rest}", () => ApiErrors.Result(404, "No such claude-comptroller endpoint."));
    }

    private static async Task<IResult> WindowsSignIn(HttpContext context, IdentityConfig identity, IdentityStore store, bool reconnect)
    {
        if (!OperatingSystem.IsWindows()) return ApiErrors.Result(500, "Windows identity mode needs a Windows server.");

        var auth = await context.AuthenticateAsync(NegotiateDefaults.AuthenticationScheme);
        if (!auth.Succeeded || auth.Principal.Identity is not WindowsIdentity windows || !windows.IsAuthenticated)
            return Results.Challenge(authenticationSchemes: [NegotiateDefaults.AuthenticationScheme]);

        var user = windows.Name;
        if (!WindowsGroup.Contains(windows, identity.AllowedGroup))
        {
            // Out of the group means out now, not when the token would have expired.
            store.ExpireAll(user);
            Serilog.Log.Warning("{User} is not in the allowed group", user);
            return ApiErrors.Result(403, $"{user} isn't in the group that may use claude-comptroller.");
        }

        var deviceName = context.Request.Query["device"].ToString().Trim();
        if (!IsDeviceName(deviceName)) return ApiErrors.Result(400, "The device name must be 1 to 64 printable characters.");

        var (userId, revoked) = store.EnsureUser(user, "windows");
        if (revoked) return ApiErrors.Result(403, $"{user}'s access to claude-comptroller was revoked. Ask your administrator.");

        var device = store.FindByUserAndName(user, deviceName);
        if (device is { Status: "revoked" } || device is { Status: "suspended", SuspendedBy: not "tray" })
            return ApiErrors.Result(403, ApiErrors.Refusal(device)!);

        // A refresh never undoes a disconnect.
        if (device is { Status: "suspended" } && !reconnect) return ApiErrors.Result(403, ApiErrors.Refusal(device)!);

        var expires = DateTime.UtcNow.AddDays(identity.TokenLifetimeDays);
        var presented = context.Request.Headers[identity.TokenHeaderName].ToString();

        // Keep the same token when the client still has it, so running sessions carry on.
        if (device is not null && presented.Length > 0 && device.TokenHash == Tokens.Hash(presented))
        {
            if (reconnect) store.Reactivate(device.Id);
            store.Extend(device.Id, expires);
            return Session(presented, device, store);
        }

        var token = Tokens.NewToken();
        store.IssueToken(userId, deviceName, token, expires);
        Serilog.Log.Information("{User} connected {Device}", user, deviceName);
        return Session(token, store.FindByUserAndName(user, deviceName)!, store);
    }

    private static IResult RedeemCode(string code, IdentityConfig identity, IdentityStore store)
    {
        if (store.RedeemCode(code) is not { } redeemed || store.FindUser(redeemed.UserId) is not { Revoked: false } user)
            return ApiErrors.Result(403, "That code is wrong, already used or expired. Ask your administrator for a new one.");

        var token = Tokens.NewToken();
        store.IssueToken(redeemed.UserId, redeemed.DeviceName, token, DateTime.UtcNow.AddDays(identity.TokenLifetimeDays));
        Serilog.Log.Information("{User} connected {Device} with a code", user.Name, redeemed.DeviceName);
        return Session(token, store.FindByUserAndName(user.Name, redeemed.DeviceName)!, store);
    }

    private static IResult ManualRefresh(HttpContext context, IdentityConfig identity, IdentityStore store)
    {
        if (DeviceFromToken(context, identity, store) is not { } device)
            return ApiErrors.Result(401, ApiErrors.NotConnected);
        if (ApiErrors.Refusal(device) is { } refusal) return ApiErrors.Result(403, refusal);

        store.Extend(device.Id, DateTime.UtcNow.AddDays(identity.TokenLifetimeDays));
        return Session(context.Request.Headers[identity.TokenHeaderName].ToString(), device, store);
    }

    /// <summary>The device as it now stands, with its token.</summary>
    private static IResult Session(string token, Device device, IdentityStore store)
    {
        var current = store.FindByUserAndName(device.User, device.Name)!;
        if (ApiErrors.Refusal(current) is { } refusal) return ApiErrors.Result(403, refusal);

        return Results.Json(new
        {
            token,
            user = current.User,
            device = current.Name,
            status = current.EffectiveStatus,
            expiresUtc = IdentityStore.Format(current.ExpiresUtc),
        });
    }

    private static Device? DeviceFromToken(HttpContext context, IdentityConfig identity, IdentityStore store)
    {
        var token = context.Request.Headers[identity.TokenHeaderName].ToString();
        return token.Length == 0 ? null : store.FindByToken(token);
    }

    private static bool IsDeviceName(string name) => name.Length is > 0 and <= 64 && !name.Any(char.IsControl);

    private static async Task<string?> ReadCode(HttpRequest request)
    {
        if (request.ContentLength is not > 0 and not null) return null;
        try
        {
            var body = await JsonSerializer.DeserializeAsync<JsonElement>(request.Body, cancellationToken: request.HttpContext.RequestAborted);
            return body.ValueKind == JsonValueKind.Object && body.TryGetProperty("code", out var code) && code.ValueKind == JsonValueKind.String
                ? code.GetString()
                : null;
        }
        catch (JsonException)
        {
            return null;
        }
    }
}
