using System.Diagnostics;
using Comptroller.Service;
using Comptroller.Service.Config;
using Comptroller.Service.Identity;
using Comptroller.Service.Proxy;
using Microsoft.AspNetCore.Authentication.Negotiate;
using Microsoft.Extensions.Hosting.WindowsServices;
using Microsoft.Extensions.Options;
using Serilog;
using Serilog.Events;
using Serilog.Formatting.Compact;
using Yarp.ReverseProxy.Configuration;

// Runs in a console, or under the Windows Service Control Manager, which it detects by itself.
// Config file: --config <path>, then COMPTROLLER_CONFIG, then comptroller.json beside the exe.
var configArg = Array.IndexOf(args, "--config");
var configPath = Path.GetFullPath(
    configArg >= 0 && configArg + 1 < args.Length
        ? args[configArg + 1]
        : Environment.GetEnvironmentVariable("COMPTROLLER_CONFIG")
          ?? Path.Combine(AppContext.BaseDirectory, "comptroller.json"));

ConfigStore store;
try
{
    store = new ConfigStore(configPath);
}
catch (Exception ex) when (ex is InvalidOperationException or ArgumentException or DirectoryNotFoundException)
{
    // A service has no console. Unhandled, the reason lands in the Windows Application event log.
    if (WindowsServiceHelpers.IsWindowsService()) throw;
    Console.Error.WriteLine(ex.Message);
    return 1;
}

var config = store.CurrentValue;
// Relative folders are relative to the config file, not to wherever the process started.
var logFolder = Path.GetFullPath(config.Logging.Folder, Path.GetDirectoryName(configPath)!);
var level = Enum.TryParse<LogEventLevel>(config.Logging.Level, ignoreCase: true, out var parsed)
    ? parsed
    : LogEventLevel.Information;

Log.Logger = new LoggerConfiguration()
    .MinimumLevel.Is(level)
    .MinimumLevel.Override("Microsoft", LogEventLevel.Warning)
    .MinimumLevel.Override("Yarp", LogEventLevel.Warning)
    .WriteTo.Console()
    .WriteTo.File(
        new CompactJsonFormatter(),
        Path.Combine(logFolder, "comptroller-.log"),
        rollingInterval: RollingInterval.Day,
        rollOnFileSizeLimit: true,
        fileSizeLimitBytes: config.Logging.FileSizeLimitMb * 1024L * 1024L,
        retainedFileCountLimit: config.Logging.RetainedFiles)
    .CreateLogger();

// Relative to the config file, like the log folder.
var databasePath = Path.Combine(Path.GetFullPath(config.Data.Folder, Path.GetDirectoryName(configPath)!), "comptroller.db");

try
{
    var identityStore = new IdentityStore(databasePath);

    // Manual identity mode without the admin terminal (whose Users screen does the same): print a one-time code and exit.
    var codeArg = Array.IndexOf(args, "--create-code");
    if (codeArg >= 0)
    {
        if (codeArg + 2 >= args.Length)
        {
            Console.Error.WriteLine("Usage: --create-code <user> <device>");
            return 1;
        }

        var (userId, revoked) = identityStore.EnsureUser(args[codeArg + 1], "manual");
        if (revoked)
        {
            Console.Error.WriteLine($"{args[codeArg + 1]} is revoked.");
            return 1;
        }

        Console.WriteLine(identityStore.CreateCode(userId, args[codeArg + 2], TimeSpan.FromHours(config.Identity.CodeLifetimeHours)));
        return 0;
    }

    var builder = WebApplication.CreateSlimBuilder(new WebApplicationOptions
    {
        // A service starts in System32.
        ContentRootPath = WindowsServiceHelpers.IsWindowsService() ? AppContext.BaseDirectory : null,
    });
    builder.Services.AddWindowsService();
    // On stop, requests in flight get this long to finish.
    builder.Services.Configure<HostOptions>(host => host.ShutdownTimeout = TimeSpan.FromSeconds(config.Proxy.ShutdownTimeoutSeconds));
    builder.Services.AddSerilog();
    builder.Services.AddSingleton(identityStore);
    builder.Services.AddAuthentication(NegotiateDefaults.AuthenticationScheme).AddNegotiate();
    builder.Services.AddAuthorization();
    builder.Services.AddSingleton<IOptionsMonitor<ComptrollerConfig>>(store);
    builder.Services.AddSingleton<IProxyConfigProvider, ComptrollerProxyConfig>();
    builder.Services.AddReverseProxy()
        // Forward the client's headers as they are, without adding .NET tracing headers.
        .ConfigureHttpClient((_, handler) => handler.ActivityHeadersPropagator = null);

    builder.WebHost.UseUrls(config.Proxy.ListenUrl, $"http://127.0.0.1:{config.Proxy.AdminPort}");
    builder.WebHost.UseKestrelHttpsConfiguration();
    builder.WebHost.ConfigureKestrel(kestrel =>
    {
        kestrel.AddServerHeader = false;
        // Requests with images or long histories exceed the 30 MB default; the upstream sets the limit.
        kestrel.Limits.MaxRequestBodySize = null;
    });

    var app = builder.Build();

    // Before logging: the admin TUI polls every few seconds.
    app.UseAdminPort(store, config);

    // Metadata only: no bodies, no query strings, no header values.
    app.Use(async (context, next) =>
    {
        var started = Stopwatch.GetTimestamp();
        try
        {
            await next();
        }
        finally
        {
            Log.Information(
                "{Method} {Path} {Status} {ElapsedMs:0} ms {User} {Device}",
                context.Request.Method,
                context.Request.Path.Value,
                context.Response.StatusCode,
                Stopwatch.GetElapsedTime(started).TotalMilliseconds,
                context.Items[ProxyGate.UserItem] ?? "-",
                context.Items[ProxyGate.DeviceItem] ?? "-");
        }
    });

    app.UseAllowedAddresses();
    app.UseAuthentication();
    app.UseDeviceTokens();

    app.MapComptrollerEndpoints();
    app.MapReverseProxy();

    Log.Information("Listening on {ListenUrl}, health on port {AdminPort}, forwarding to {UpstreamUrl}, config {ConfigPath}",
        config.Proxy.ListenUrl, config.Proxy.AdminPort, config.Proxy.UpstreamUrl, configPath);
    app.Lifetime.ApplicationStopping.Register(() => Log.Information("Stopping: waiting for requests in flight"));
    app.Lifetime.ApplicationStopped.Register(() => Log.Information("Stopped"));

    await app.RunAsync();
    return 0;
}
catch (Exception ex)
{
    Log.Fatal(ex, "The service stopped unexpectedly");
    return 1;
}
finally
{
    store.Dispose();
    await Log.CloseAndFlushAsync();
}
