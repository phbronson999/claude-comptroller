namespace Comptroller.Service.Config;

/// <summary>The shape of comptroller.json. See contracts/comptroller.sample.json.</summary>
public sealed record ComptrollerConfig
{
    public ProxyConfig Proxy { get; init; } = new();
    public LoggingConfig Logging { get; init; } = new();
    public IdentityConfig Identity { get; init; } = new();

    /// <summary>Returns every problem found; an empty list means the config is usable.</summary>
    public IReadOnlyList<string> Validate()
    {
        var errors = new List<string>();

        if (Proxy is null) return ["Proxy section is missing."];
        if (!IsHttpUrl(Proxy.ListenUrl)) errors.Add("Proxy.ListenUrl must be an http:// or https:// URL.");
        if (!IsHttpUrl(Proxy.UpstreamUrl)) errors.Add("Proxy.UpstreamUrl must be an http:// or https:// URL.");
        if (Proxy.ActivityTimeoutMinutes is < 1 or > 60) errors.Add("Proxy.ActivityTimeoutMinutes must be 1 to 60.");

        if (string.IsNullOrWhiteSpace(Logging?.Folder)) errors.Add("Logging.Folder must not be empty.");
        if (string.IsNullOrWhiteSpace(Identity?.TokenHeaderName)) errors.Add("Identity.TokenHeaderName must not be empty.");

        return errors;
    }

    private static bool IsHttpUrl(string? value) =>
        Uri.TryCreate(value, UriKind.Absolute, out var uri)
        && (uri.Scheme == Uri.UriSchemeHttp || uri.Scheme == Uri.UriSchemeHttps);
}

public sealed record ProxyConfig
{
    public string ListenUrl { get; init; } = "http://localhost:8080";
    public string UpstreamUrl { get; init; } = "https://api.anthropic.com";
    public int ActivityTimeoutMinutes { get; init; } = 10;
    public bool StripForwardedHeaders { get; init; } = true;
}

public sealed record LoggingConfig
{
    public string Folder { get; init; } = "logs";
    public string Level { get; init; } = "Information";
    public int RetainedFiles { get; init; } = 14;
    public int FileSizeLimitMb { get; init; } = 20;
}

public sealed record IdentityConfig
{
    public string TokenHeaderName { get; init; } = "X-Comptroller-Token";
}
