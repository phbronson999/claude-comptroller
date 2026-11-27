using Comptroller.Service.Config;
using Microsoft.Extensions.Options;
using Microsoft.Extensions.Primitives;
using Yarp.ReverseProxy.Configuration;
using Yarp.ReverseProxy.Forwarder;
using Yarp.ReverseProxy.Transforms;

namespace Comptroller.Service.Proxy;

/// <summary>Builds YARP's one catch-all route from comptroller.json and rebuilds it on change.</summary>
public sealed class ComptrollerProxyConfig : IProxyConfigProvider, IDisposable
{
    private const string UpstreamCluster = "upstream";

    private readonly IDisposable? _subscription;
    private volatile Snapshot _snapshot;

    public ComptrollerProxyConfig(IOptionsMonitor<ComptrollerConfig> options)
    {
        _snapshot = Build(options.CurrentValue);
        _subscription = options.OnChange((config, _) =>
        {
            var previous = _snapshot;
            _snapshot = Build(config);
            previous.SignalChange();
        });
    }

    public IProxyConfig GetConfig() => _snapshot;

    private static Snapshot Build(ComptrollerConfig config)
    {
        var route = new RouteConfig
        {
            RouteId = "anthropic",
            ClusterId = UpstreamCluster,
            Match = new RouteMatch { Path = "{**catch-all}" },
        }
        // Authorization, x-api-key and anthropic-* pass through untouched: never transform them.
        .WithTransformRequestHeaderRemove(config.Identity.TokenHeaderName);

        if (config.Proxy.StripForwardedHeaders)
        {
            route = route.WithTransformXForwarded(
                xDefault: ForwardedTransformActions.Off);
        }

        var cluster = new ClusterConfig
        {
            ClusterId = UpstreamCluster,
            HttpRequest = new ForwarderRequestConfig
            {
                ActivityTimeout = TimeSpan.FromMinutes(config.Proxy.ActivityTimeoutMinutes),
            },
            Destinations = new Dictionary<string, DestinationConfig>
            {
                ["primary"] = new() { Address = config.Proxy.UpstreamUrl },
            },
        };

        return new Snapshot([route], [cluster]);
    }

    public void Dispose() => _subscription?.Dispose();

    private sealed class Snapshot : IProxyConfig
    {
        private readonly CancellationTokenSource _changed = new();

        public Snapshot(IReadOnlyList<RouteConfig> routes, IReadOnlyList<ClusterConfig> clusters)
        {
            Routes = routes;
            Clusters = clusters;
            ChangeToken = new CancellationChangeToken(_changed.Token);
        }

        public IReadOnlyList<RouteConfig> Routes { get; }
        public IReadOnlyList<ClusterConfig> Clusters { get; }
        public IChangeToken ChangeToken { get; }

        public void SignalChange() => _changed.Cancel();
    }
}
