using System.IO.Compression;
using System.Text;
using System.Text.Json;

namespace Comptroller.Service.Recording;

/// <summary>Token counts from one reply. Null fields: the reply didn't say.</summary>
public sealed record Usage(
    string? Model,
    long? InputTokens,
    long? OutputTokens,
    long? CacheWriteTokens,
    long? CacheWrite5mTokens,
    long? CacheWrite1hTokens,
    long? CacheReadTokens)
{
    /// <summary>Enough was read to count the request.</summary>
    public bool Complete => Model is not null && InputTokens is not null && OutputTokens is not null;
}

/// <summary>
/// Reads the model and token counts from a /v1/messages reply as its bytes pass through to
/// the client. It only looks: bytes are never held back. Only the "model" and "usage" fields
/// are read; the content of a reply is never kept.
/// <list type="bullet">
/// <item>A streamed reply (text/event-stream) is read line by line, and only the
/// message_start and message_delta events are parsed: the first carries the model, input and
/// cache counts, the last the output count.</item>
/// <item>A JSON reply, or any compressed reply, is copied up to <see cref="CopyLimit"/> and
/// read once it's complete.</item>
/// </list>
/// </summary>
public sealed class UsageReader(HttpResponse response)
{
    public const int CopyLimit = 16 * 1024 * 1024;
    private const int LineLimit = 1024 * 1024;

    private enum Mode { Undecided, Events, Copy, Off }

    private Mode _mode;
    private string? _encoding;
    private bool _events;
    private bool _sawDelta;
    private readonly List<byte> _line = [];
    private bool _lineTooLong;
    private MemoryStream? _copy;

    private string? _model;
    private long? _input, _output, _cacheWrite, _cacheWrite5m, _cacheWrite1h, _cacheRead;

    public void Feed(ReadOnlySpan<byte> bytes)
    {
        if (_mode == Mode.Undecided) Decide();
        switch (_mode)
        {
            case Mode.Events:
                FeedLines(bytes);
                break;
            case Mode.Copy:
                if (_copy!.Length + bytes.Length > CopyLimit) _mode = Mode.Off;
                else _copy.Write(bytes);
                break;
        }
    }

    private void Decide()
    {
        var type = response.ContentType ?? "";
        _events = type.StartsWith("text/event-stream", StringComparison.OrdinalIgnoreCase);
        _encoding = response.Headers.ContentEncoding.ToString().Trim().ToLowerInvariant();
        if (_encoding is "" or "identity") _encoding = null;

        if (_events && _encoding is null) _mode = Mode.Events;
        else if (_events || type.Contains("json", StringComparison.OrdinalIgnoreCase))
        {
            _mode = Mode.Copy;
            _copy = new MemoryStream();
        }
        else _mode = Mode.Off;
    }

    /// <summary>What was read, once the reply has ended. Null when nothing could be read.</summary>
    public Usage? Finish()
    {
        try
        {
            if (_mode == Mode.Events && _line.Count > 0) Line(_line.ToArray());
            if (_mode == Mode.Copy) ReadCopy();
        }
        catch (Exception ex) when (ex is JsonException or InvalidDataException or IOException)
        {
            Serilog.Log.Debug(ex, "Couldn't read usage from a reply");
        }

        // A stream cut off before its last event only has a provisional output count.
        if (_events && !_sawDelta) return null;
        var usage = new Usage(_model, _input, _output, _cacheWrite, _cacheWrite5m, _cacheWrite1h, _cacheRead);
        return usage.Complete ? usage : null;
    }

    private void ReadCopy()
    {
        _copy!.Position = 0;
        Stream body = _encoding switch
        {
            null => _copy,
            "gzip" => new GZipStream(_copy, CompressionMode.Decompress),
            "deflate" => new ZLibStream(_copy, CompressionMode.Decompress),
            "br" => new BrotliStream(_copy, CompressionMode.Decompress),
            _ => throw new InvalidDataException($"Unknown content encoding {_encoding}"),
        };
        using (body)
        {
            if (_events)
            {
                using var reader = new StreamReader(body, Encoding.UTF8);
                while (reader.ReadLine() is { } line) Line(Encoding.UTF8.GetBytes(line));
            }
            else
            {
                using var doc = JsonDocument.Parse(body);
                var root = doc.RootElement;
                if (root.TryGetProperty("model", out var model) && model.ValueKind == JsonValueKind.String) _model = model.GetString();
                if (root.TryGetProperty("usage", out var usage)) ReadUsage(usage);
            }
        }
    }

    private void FeedLines(ReadOnlySpan<byte> bytes)
    {
        while (!bytes.IsEmpty)
        {
            var end = bytes.IndexOf((byte)'\n');
            var part = end < 0 ? bytes : bytes[..end];
            if (!_lineTooLong)
            {
                if (_line.Count + part.Length > LineLimit)
                {
                    // A huge line is content, never the events we want.
                    _lineTooLong = true;
                    _line.Clear();
                }
                else _line.AddRange(part);
            }

            if (end < 0) return;
            if (!_lineTooLong) Line(_line.ToArray());
            _line.Clear();
            _lineTooLong = false;
            bytes = bytes[(end + 1)..];
        }
    }

    private static readonly byte[] DataPrefix = "data:"u8.ToArray();

    /// <summary>One line of a stream. Only "data:" lines for the two events that carry usage are parsed.</summary>
    private void Line(ReadOnlySpan<byte> line)
    {
        if (!line.StartsWith(DataPrefix)) return;
        var json = line[DataPrefix.Length..].Trim((byte)' ').TrimEnd((byte)'\r');
        // The event type comes first in each event, so a cheap look rules out content deltas.
        var head = json[..Math.Min(json.Length, 48)];
        var start = head.IndexOf("\"message_start\""u8) >= 0;
        if (!start && head.IndexOf("\"message_delta\""u8) < 0) return;

        using var doc = JsonDocument.Parse(json.ToArray());
        var root = doc.RootElement;
        if (start && root.TryGetProperty("message", out var message))
        {
            if (message.TryGetProperty("model", out var model) && model.ValueKind == JsonValueKind.String) _model = model.GetString();
            if (message.TryGetProperty("usage", out var usage)) ReadUsage(usage);
        }
        else if (root.TryGetProperty("usage", out var usage))
        {
            // message_delta: counts so far, which replace earlier ones.
            _sawDelta = true;
            ReadUsage(usage);
        }
    }

    private void ReadUsage(JsonElement usage)
    {
        if (usage.ValueKind != JsonValueKind.Object) return;
        _input = Number(usage, "input_tokens") ?? _input;
        _output = Number(usage, "output_tokens") ?? _output;
        _cacheWrite = Number(usage, "cache_creation_input_tokens") ?? _cacheWrite;
        _cacheRead = Number(usage, "cache_read_input_tokens") ?? _cacheRead;
        if (usage.TryGetProperty("cache_creation", out var split) && split.ValueKind == JsonValueKind.Object)
        {
            _cacheWrite5m = Number(split, "ephemeral_5m_input_tokens") ?? _cacheWrite5m;
            _cacheWrite1h = Number(split, "ephemeral_1h_input_tokens") ?? _cacheWrite1h;
        }
    }

    private static long? Number(JsonElement parent, string name) =>
        parent.TryGetProperty(name, out var value) && value.TryGetInt64(out var n) ? n : null;
}
