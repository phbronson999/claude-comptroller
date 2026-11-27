using System.Text.Json;
using Microsoft.Extensions.FileProviders;
using Microsoft.Extensions.Options;
using Microsoft.Extensions.Primitives;

namespace Comptroller.Service.Config;

/// <summary>
/// Loads comptroller.json and reloads it when it changes. An invalid file never replaces
/// the last good config. The built-in JSON configuration provider isn't used because it
/// discards its values when a reload fails to parse.
/// </summary>
public sealed class ConfigStore : IOptionsMonitor<ComptrollerConfig>, IDisposable
{
    private static readonly JsonSerializerOptions JsonOptions = new()
    {
        PropertyNameCaseInsensitive = true,
        ReadCommentHandling = JsonCommentHandling.Skip,
        AllowTrailingCommas = true,
    };

    private readonly string _path;
    private readonly PhysicalFileProvider _files;
    private readonly IDisposable _watch;
    private readonly Lock _gate = new();
    private volatile ComptrollerConfig _current;

    private event Action<ComptrollerConfig, string?>? Changed;

    public ConfigStore(string path)
    {
        _path = Path.GetFullPath(path);
        var (config, error) = Read(_path);
        _current = config ?? throw new InvalidOperationException($"Can't load {_path}: {error}");

        var name = Path.GetFileName(_path);
        _files = new PhysicalFileProvider(Path.GetDirectoryName(_path)!);
        _watch = ChangeToken.OnChange(() => _files.Watch(name), Reload);
    }

    public ComptrollerConfig CurrentValue => _current;

    /// <summary>Why the file on disk was rejected, or null if the current config matches it.</summary>
    public string? LastError { get; private set; }

    public ComptrollerConfig Get(string? name) => _current;

    public IDisposable OnChange(Action<ComptrollerConfig, string?> listener)
    {
        Changed += listener;
        return new Subscription(() => Changed -= listener);
    }

    private void Reload()
    {
        // Editors often write in several steps; let the file settle first.
        Thread.Sleep(250);

        lock (_gate)
        {
            var (config, error) = Read(_path);
            LastError = error;
            if (config is null)
            {
                Serilog.Log.Warning("Ignoring invalid config, keeping the last good one: {Error}", error);
                return;
            }

            if (config == _current) return;
            _current = config;
            Serilog.Log.Information("Config reloaded");
            Changed?.Invoke(config, Options.DefaultName);
        }
    }

    private static (ComptrollerConfig? Config, string? Error) Read(string path)
    {
        try
        {
            var config = JsonSerializer.Deserialize<ComptrollerConfig>(File.ReadAllText(path), JsonOptions);
            if (config is null) return (null, "The file is empty.");

            var errors = config.Validate();
            return errors.Count == 0 ? (config, null) : (null, string.Join(" ", errors));
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or JsonException)
        {
            return (null, ex.Message);
        }
    }

    public void Dispose()
    {
        _watch.Dispose();
        _files.Dispose();
    }

    private sealed class Subscription(Action dispose) : IDisposable
    {
        public void Dispose() => dispose();
    }
}
