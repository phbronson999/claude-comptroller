using System.Collections.Concurrent;
using System.Net;
using Comptroller.Service.Config;
using Microsoft.Extensions.Options;

namespace Comptroller.Service.Identity;

/// <summary>The checks every request passes before it is forwarded: client address, then device token.</summary>
public static class ProxyGate
{
    public const string UserItem = "comptroller.user";
    public const string DeviceItem = "comptroller.device";

    private static readonly TimeSpan TouchEvery = TimeSpan.FromMinutes(1);

    public static void UseAllowedAddresses(this WebApplication app)
    {
        var options = app.Services.GetRequiredService<IOptionsMonitor<ComptrollerConfig>>();
        app.Use(async (context, next) =>
        {
            if (!IsAllowed(context.Connection.RemoteIpAddress, options.CurrentValue.Access.AllowedCidrs))
            {
                await ApiErrors.Write(context, 403, "This network address isn't allowed to use claude-comptroller.");
                return;
            }

            await next();
        });
    }

    public static void UseDeviceTokens(this WebApplication app)
    {
        var options = app.Services.GetRequiredService<IOptionsMonitor<ComptrollerConfig>>();
        var store = app.Services.GetRequiredService<IdentityStore>();
        var touched = new ConcurrentDictionary<long, DateTime>();

        app.Use(async (context, next) =>
        {
            var request = context.Request;
            // Own endpoints do their own auth. Claude Code's connectivity probe carries no custom headers.
            if (request.Path.StartsWithSegments("/_comptroller")
                || (HttpMethods.IsHead(request.Method) && request.Path == "/api/hello"))
            {
                await next();
                return;
            }

            var token = request.Headers[options.CurrentValue.Identity.TokenHeaderName].ToString();
            var device = token.Length == 0 ? null : store.FindByToken(token);
            if (device is null)
            {
                await ApiErrors.Write(context, 401, ApiErrors.NotConnected);
                return;
            }

            context.Items[UserItem] = device.User;
            context.Items[DeviceItem] = device.Name;

            if (ApiErrors.Refusal(device) is { } refusal)
            {
                await ApiErrors.Write(context, 403, refusal);
                return;
            }

            var now = DateTime.UtcNow;
            if (!touched.TryGetValue(device.Id, out var last) || now - last >= TouchEvery)
            {
                touched[device.Id] = now;
                // Off the request path: a request never waits on this write.
                _ = Task.Run(() =>
                {
                    try
                    {
                        store.Touch(device.Id);
                    }
                    catch (Exception ex)
                    {
                        Serilog.Log.Warning(ex, "Couldn't update last-seen for device {DeviceId}", device.Id);
                    }
                });
            }

            await next();
        });
    }

    private static bool IsAllowed(IPAddress? address, IReadOnlyList<string> cidrs)
    {
        if (cidrs.Count == 0) return true;
        if (address is null) return false;
        if (address.IsIPv4MappedToIPv6) address = address.MapToIPv4();
        return cidrs.Any(cidr => IPNetwork.TryParse(cidr, out var network) && network.Contains(address));
    }
}
