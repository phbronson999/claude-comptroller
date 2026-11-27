namespace Comptroller.Service.Config;

/// <summary>The shape of comptroller.json. See contracts/comptroller.sample.json.</summary>
public sealed record ComptrollerConfig
{
    public GeneralConfig General { get; init; } = new();
    public ProxyConfig Proxy { get; init; } = new();
    public LoggingConfig Logging { get; init; } = new();
    public IdentityConfig Identity { get; init; } = new();
    public AccessConfig Access { get; init; } = new();
    public DataConfig Data { get; init; } = new();

    /// <summary>Returns every problem found; an empty list means the config is usable.</summary>
    public IReadOnlyList<string> Validate()
    {
        var errors = new List<string>();

        if (Proxy is null) return ["Proxy section is missing."];
        if (!IsHttpUrl(Proxy.ListenUrl)) errors.Add("Proxy.ListenUrl must be an http:// or https:// URL.");
        if (!IsHttpUrl(Proxy.UpstreamUrl)) errors.Add("Proxy.UpstreamUrl must be an http:// or https:// URL.");
        if (Proxy.ActivityTimeoutMinutes is < 1 or > 60) errors.Add("Proxy.ActivityTimeoutMinutes must be 1 to 60.");
        if (Proxy.ShutdownTimeoutSeconds is < 5 or > 600) errors.Add("Proxy.ShutdownTimeoutSeconds must be 5 to 600.");
        if (Proxy.AdminPort is < 1 or > 65535) errors.Add("Proxy.AdminPort must be a port number.");
        else if (Uri.TryCreate(Proxy.ListenUrl, UriKind.Absolute, out var listen) && listen.Port == Proxy.AdminPort)
            errors.Add("Proxy.AdminPort must differ from the port in Proxy.ListenUrl.");

        if (string.IsNullOrWhiteSpace(Logging?.Folder)) errors.Add("Logging.Folder must not be empty.");
        if (Logging is not null)
        {
            string[] levels = ["Verbose", "Debug", "Information", "Warning", "Error", "Fatal"];
            if (!levels.Contains(Logging.Level, StringComparer.OrdinalIgnoreCase))
                errors.Add("Logging.Level must be Verbose, Debug, Information, Warning, Error or Fatal.");
            if (Logging.RetainedFiles is < 1 or > 365) errors.Add("Logging.RetainedFiles must be 1 to 365.");
            if (Logging.FileSizeLimitMb is < 1 or > 1024) errors.Add("Logging.FileSizeLimitMb must be 1 to 1024.");
        }
        if (string.IsNullOrWhiteSpace(Data?.Folder)) errors.Add("Data.Folder must not be empty.");
        if (Data is not null)
        {
            if (Data.RetentionDays is < 1 or > 365) errors.Add("Data.RetentionDays must be 1 to 365.");
            if (!TimeOnly.TryParseExact(Data.PruneTime, "HH:mm", System.Globalization.CultureInfo.InvariantCulture, System.Globalization.DateTimeStyles.None, out _))
                errors.Add("Data.PruneTime must be a 24-hour time such as 02:00.");
        }

        if (Identity is null) return [.. errors, "Identity section is invalid."];
        if (string.IsNullOrWhiteSpace(Identity.TokenHeaderName)) errors.Add("Identity.TokenHeaderName must not be empty.");
        if (Identity.Mode is not ("windows" or "manual")) errors.Add("Identity.Mode must be \"windows\" or \"manual\".");
        if (Identity.Mode == "windows" && string.IsNullOrWhiteSpace(Identity.AllowedGroup)) errors.Add("Identity.AllowedGroup must not be empty.");
        if (Identity.TokenLifetimeDays is < 1 or > 90) errors.Add("Identity.TokenLifetimeDays must be 1 to 90.");
        if (Identity.CodeLifetimeHours is < 1 or > 168) errors.Add("Identity.CodeLifetimeHours must be 1 to 168.");

        if (Access?.AllowedCidrs is null) return [.. errors, "Access.AllowedCidrs must be a list."];
        foreach (var cidr in Access.AllowedCidrs)
        {
            if (!System.Net.IPNetwork.TryParse(cidr, out _)) errors.Add($"Access.AllowedCidrs: \"{cidr}\" isn't a CIDR such as 10.0.0.0/24.");
        }

        return errors;
    }

    private static bool IsHttpUrl(string? value) =>
        Uri.TryCreate(value, UriKind.Absolute, out var uri)
        && (uri.Scheme == Uri.UriSchemeHttp || uri.Scheme == Uri.UriSchemeHttps);
}

public sealed record GeneralConfig
{
    /// <summary>Shown to developers in the tray app, if set.</summary>
    public string OrganizationName { get; init; } = "";
}

public sealed record ProxyConfig
{
    public string ListenUrl { get; init; } = "http://localhost:8080";
    public string UpstreamUrl { get; init; } = "https://api.anthropic.com";
    public int ActivityTimeoutMinutes { get; init; } = 10;
    public int AdminPort { get; init; } = 8081;
    public int ShutdownTimeoutSeconds { get; init; } = 60;
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
    public string Mode { get; init; } = "windows";
    public string AllowedGroup { get; init; } = "Claude Code Users";
    public int TokenLifetimeDays { get; init; } = 7;
    public int CodeLifetimeHours { get; init; } = 24;
    public string TokenHeaderName { get; init; } = "X-Comptroller-Token";
}

public sealed record AccessConfig
{
    public IReadOnlyList<string> AllowedCidrs { get; init; } = ["127.0.0.1/32", "::1/128"];
}

public sealed record DataConfig
{
    public string Folder { get; init; } = "data";

    /// <summary>Days of requests kept: today and the days before it.</summary>
    public int RetentionDays { get; init; } = 14;

    /// <summary>Local time of the nightly prune, HH:mm.</summary>
    public string PruneTime { get; init; } = "02:00";

    public TimeOnly PruneAt => TimeOnly.ParseExact(PruneTime, "HH:mm", System.Globalization.CultureInfo.InvariantCulture);
}
