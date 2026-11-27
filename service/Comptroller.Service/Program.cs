using System.Diagnostics;
using Comptroller.Service.Config;
using Comptroller.Service.Proxy;
using Microsoft.Extensions.Options;
using Serilog;
using Serilog.Events;
using Serilog.Formatting.Compact;
using Yarp.ReverseProxy.Configuration;

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

try
{
    var builder = WebApplication.CreateSlimBuilder(args);
    builder.Services.AddSerilog();
    builder.Services.AddSingleton<IOptionsMonitor<ComptrollerConfig>>(store);
    builder.Services.AddSingleton<IProxyConfigProvider, ComptrollerProxyConfig>();
    builder.Services.AddReverseProxy()
        // Forward the client's headers as they are, without adding .NET tracing headers.
        .ConfigureHttpClient((_, handler) => handler.ActivityHeadersPropagator = null);

    builder.WebHost.UseUrls(config.Proxy.ListenUrl);
    builder.WebHost.UseKestrelHttpsConfiguration();
    builder.WebHost.ConfigureKestrel(kestrel =>
    {
        kestrel.AddServerHeader = false;
        // Requests with images or long histories exceed the 30 MB default; the upstream sets the limit.
        kestrel.Limits.MaxRequestBodySize = null;
    });

    var app = builder.Build();

    // Metadata only: no bodies, no query strings, no header values.
    app.Use(async (context, next) =>
    {
        var started = Stopwatch.GetTimestamp();
        var hasToken = context.Request.Headers.ContainsKey(store.CurrentValue.Identity.TokenHeaderName);
        try
        {
            await next();
        }
        finally
        {
            Log.Information(
                "{Method} {Path} {Status} {ElapsedMs:0} ms, token header {TokenHeader}",
                context.Request.Method,
                context.Request.Path.Value,
                context.Response.StatusCode,
                Stopwatch.GetElapsedTime(started).TotalMilliseconds,
                hasToken ? "present" : "missing");
        }
    });

    app.MapReverseProxy();

    Log.Information("Listening on {ListenUrl}, forwarding to {UpstreamUrl}, config {ConfigPath}",
        config.Proxy.ListenUrl, config.Proxy.UpstreamUrl, configPath);

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
