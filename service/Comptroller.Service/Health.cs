using System.Reflection;
using System.Text.Json;
using Comptroller.Service.Config;
using Comptroller.Service.Identity;
using Microsoft.Extensions.Hosting.WindowsServices;

namespace Comptroller.Service;

/// <summary>GET /health on its own loopback-only port, for the admin TUI. See contracts/api.md.</summary>
public static class Health
{
    private static readonly string Version = typeof(Health).Assembly
        .GetCustomAttribute<AssemblyInformationalVersionAttribute>()!.InformationalVersion.Split('+')[0];

    /// <summary>
    /// Answers everything arriving on the admin port, so none of it reaches the proxy.
    /// On the public port /health is an ordinary proxied path.
    /// </summary>
    /// <param name="store">The live config.</param>
    /// <param name="startup">The config the service started with: its listeners don't change on reload.</param>
    public static void UseAdminPort(this WebApplication app, ConfigStore store, ComptrollerConfig startup)
    {
        var started = DateTime.UtcNow;
        var adminPort = startup.Proxy.AdminPort;
        var json = new JsonSerializerOptions(JsonSerializerDefaults.Web);
        var inFlight = app.Services.GetRequiredService<Recording.InFlight>();
        var log = app.Services.GetRequiredService<Recording.RequestLog>();
        var pruner = app.Services.GetRequiredService<Recording.Pruner>();

        app.Use(async (context, next) =>
        {
            if (context.Connection.LocalPort != adminPort)
            {
                await next();
                return;
            }

            if (!HttpMethods.IsGet(context.Request.Method) || context.Request.Path != "/health")
            {
                await ApiErrors.Write(context, 404, "The admin port only serves GET /health.");
                return;
            }

            var error = store.LastError;
            await context.Response.WriteAsJsonAsync(new
            {
                version = Version,
                startedUtc = IdentityStore.Format(started),
                uptimeSeconds = (long)(DateTime.UtcNow - started).TotalSeconds,
                windowsService = WindowsServiceHelpers.IsWindowsService(),
                config = new
                {
                    status = error is null ? "ok" : "error",
                    error,
                    path = store.FilePath,
                    loadedUtc = IdentityStore.Format(store.LoadedUtc),
                },
                schemaVersion = IdentityStore.SchemaVersion,
                listenUrl = startup.Proxy.ListenUrl,
                upstreamUrl = store.CurrentValue.Proxy.UpstreamUrl,
                inFlight = new
                {
                    count = inFlight.Count,
                    requests = inFlight.Oldest(50).Select(r => new
                    {
                        user = r.User,
                        device = r.Device,
                        method = r.Method,
                        path = r.Path,
                        startedUtc = IdentityStore.Format(r.StartedUtc),
                    }),
                },
                recording = new { queued = log.Queued, dropped = log.Dropped, error = log.LastError },
                retention = new
                {
                    days = store.CurrentValue.Data.RetentionDays,
                    nextPruneUtc = IdentityStore.Format(pruner.Next.ToUniversalTime()),
                    lastPrune = pruner.Last is { } last
                        ? new { finishedUtc = IdentityStore.Format(last.FinishedUtc), removed = last.Removed, freedBytes = last.FreedBytes, error = last.Error }
                        : null,
                },
            }, json);
        });
    }
}
