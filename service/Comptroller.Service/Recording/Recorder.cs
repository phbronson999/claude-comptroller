using System.Collections.Concurrent;
using System.Threading.Channels;
using Comptroller.Service.Identity;
using Microsoft.Data.Sqlite;
using Yarp.ReverseProxy.Forwarder;

namespace Comptroller.Service.Recording;

/// <summary>One finished request, as stored in the requests table. Metadata only.</summary>
public sealed record RequestRecord(
    DateTime StartedUtc,
    string? User,
    string? Device,
    string? ClientIp,
    string Method,
    string Path,
    int Status,
    long DurationMs,
    long? RequestBytes,
    long ResponseBytes,
    string? ProxyError,
    Usage? Usage,
    bool UsageExpected);

/// <summary>A request the proxy is forwarding right now.</summary>
public sealed record InFlightRequest(long Id, DateTime StartedUtc, string? User, string? Device, string Method, string Path);

/// <summary>Requests in flight, for /health: the admin terminal's Radar and Terminate read it.</summary>
public sealed class InFlight
{
    private readonly ConcurrentDictionary<long, InFlightRequest> _requests = new();
    private long _next;

    public long Add(string? user, string? device, string method, string path)
    {
        var id = Interlocked.Increment(ref _next);
        _requests[id] = new InFlightRequest(id, DateTime.UtcNow, user, device, method, path);
        return id;
    }

    public void Remove(long id) => _requests.TryRemove(id, out _);

    public int Count => _requests.Count;

    /// <summary>Oldest first.</summary>
    public IReadOnlyList<InFlightRequest> Oldest(int max) => [.. _requests.Values.OrderBy(r => r.Id).Take(max)];
}

/// <summary>Counts the bytes of a reply and shows them to a <see cref="UsageReader"/> on their way out.</summary>
internal sealed class TapStream(Stream inner, UsageReader? reader) : Stream
{
    public Stream Inner => inner;
    public UsageReader? Reader => reader;
    public long Bytes { get; private set; }

    public override void Write(ReadOnlySpan<byte> buffer)
    {
        inner.Write(buffer);
        Seen(buffer);
    }

    public override void Write(byte[] buffer, int offset, int count) => Write(buffer.AsSpan(offset, count));

    public override async ValueTask WriteAsync(ReadOnlyMemory<byte> buffer, CancellationToken cancellationToken = default)
    {
        await inner.WriteAsync(buffer, cancellationToken);
        Seen(buffer.Span);
    }

    public override Task WriteAsync(byte[] buffer, int offset, int count, CancellationToken cancellationToken) =>
        WriteAsync(buffer.AsMemory(offset, count), cancellationToken).AsTask();

    private void Seen(ReadOnlySpan<byte> bytes)
    {
        Bytes += bytes.Length;
        try
        {
            reader?.Feed(bytes);
        }
        catch (Exception ex)
        {
            // Reading usage must never break a reply.
            Serilog.Log.Debug(ex, "Usage reader failed");
        }
    }

    public override void Flush() => inner.Flush();
    public override Task FlushAsync(CancellationToken cancellationToken) => inner.FlushAsync(cancellationToken);
    public override bool CanRead => false;
    public override bool CanSeek => false;
    public override bool CanWrite => true;
    public override long Length => throw new NotSupportedException();
    public override long Position { get => throw new NotSupportedException(); set => throw new NotSupportedException(); }
    public override int Read(byte[] buffer, int offset, int count) => throw new NotSupportedException();
    public override long Seek(long offset, SeekOrigin origin) => throw new NotSupportedException();
    public override void SetLength(long value) => throw new NotSupportedException();
}

/// <summary>
/// Queues finished requests and writes them to SQLite in batches on a background thread,
/// so a request never waits on the database. If the database falls far behind, the oldest
/// queued rows are dropped and counted rather than slowing the proxy.
/// </summary>
public sealed class RequestLog : BackgroundService
{
    private const int QueueLimit = 50_000;

    private readonly string _connectionString;
    private readonly Channel<RequestRecord> _queue = Channel.CreateBounded<RequestRecord>(
        new BoundedChannelOptions(QueueLimit) { FullMode = BoundedChannelFullMode.DropOldest, SingleReader = true },
        _ => Interlocked.Increment(ref _droppedCount));

    private static long _droppedCount;

    public RequestLog(string databasePath)
    {
        _connectionString = new SqliteConnectionStringBuilder { DataSource = databasePath, DefaultTimeout = 5 }.ToString();
    }

    public int Queued => _queue.Reader.Count;
    public long Dropped => Interlocked.Read(ref _droppedCount);
    public string? LastError { get; private set; }

    public void Add(RequestRecord record) => _queue.Writer.TryWrite(record);

    protected override async Task ExecuteAsync(CancellationToken stopping)
    {
        var batch = new List<RequestRecord>();
        try
        {
            while (await _queue.Reader.WaitToReadAsync(stopping))
            {
                // Let a burst gather, then write it in one transaction.
                await Task.Delay(250, stopping);
                Drain(batch);
            }
        }
        catch (OperationCanceledException)
        {
            // Stopping: write what's left.
        }
        Drain(batch);
    }

    private void Drain(List<RequestRecord> batch)
    {
        while (true)
        {
            batch.Clear();
            while (batch.Count < 500 && _queue.Reader.TryRead(out var record)) batch.Add(record);
            if (batch.Count == 0) return;
            try
            {
                Write(batch);
                LastError = null;
            }
            catch (SqliteException ex)
            {
                LastError = ex.Message;
                Serilog.Log.Warning(ex, "Couldn't record {Count} requests", batch.Count);
                return;
            }
        }
    }

    private void Write(List<RequestRecord> batch)
    {
        using var connection = new SqliteConnection(_connectionString);
        connection.Open();
        using var transaction = connection.BeginTransaction();
        using var insert = connection.CreateCommand();
        insert.CommandText = """
            INSERT INTO requests (started_utc, user_name, device_name, client_ip, method, path, status, duration_ms,
                request_bytes, response_bytes, proxy_error, model, input_tokens, output_tokens,
                cache_write_tokens, cache_write_5m_tokens, cache_write_1h_tokens, cache_read_tokens, usage)
            VALUES ($started, $user, $device, $ip, $method, $path, $status, $duration,
                $requestBytes, $responseBytes, $error, $model, $input, $output,
                $cacheWrite, $cacheWrite5m, $cacheWrite1h, $cacheRead, $usage)
            """;
        string[] names = ["$started", "$user", "$device", "$ip", "$method", "$path", "$status", "$duration", "$requestBytes", "$responseBytes",
            "$error", "$model", "$input", "$output", "$cacheWrite", "$cacheWrite5m", "$cacheWrite1h", "$cacheRead", "$usage"];
        foreach (var name in names) insert.Parameters.Add(new SqliteParameter { ParameterName = name });

        foreach (var r in batch)
        {
            var u = r.Usage;
            object?[] values = [IdentityStore.Format(r.StartedUtc), r.User, r.Device, r.ClientIp, r.Method, r.Path, r.Status, r.DurationMs,
                r.RequestBytes, r.ResponseBytes, r.ProxyError, u?.Model, u?.InputTokens, u?.OutputTokens,
                u?.CacheWriteTokens, u?.CacheWrite5mTokens, u?.CacheWrite1hTokens, u?.CacheReadTokens,
                u is not null ? "parsed" : r.UsageExpected ? "missing" : null];
            for (var i = 0; i < values.Length; i++) insert.Parameters[i].Value = values[i] ?? DBNull.Value;
            insert.ExecuteNonQuery();
        }

        transaction.Commit();
    }

    /// <summary>Today's counts for one computer, for the tray app. "Today" is the server's local day.</summary>
    public (long Requests, long LimitHits, string? LastUtc) Today(string user, string device)
    {
        var midnight = IdentityStore.Format(DateTime.Today.ToUniversalTime());
        using var connection = new SqliteConnection(_connectionString);
        connection.Open();
        using var query = connection.CreateCommand();
        query.CommandText = """
            SELECT COUNT(*), COALESCE(SUM(status = 429), 0), MAX(started_utc) FROM requests
            WHERE user_name = $user AND device_name = $device AND started_utc >= $midnight
            """;
        query.Parameters.AddWithValue("$user", user);
        query.Parameters.AddWithValue("$device", device);
        query.Parameters.AddWithValue("$midnight", midnight);
        using var reader = query.ExecuteReader();
        reader.Read();
        return (reader.GetInt64(0), reader.GetInt64(1), reader.IsDBNull(2) ? null : reader.GetString(2));
    }
}

public static class Recording
{
    /// <summary>
    /// Records every proxied request once it has finished, and lists it as in flight meanwhile.
    /// Runs after the token check, so it knows who sent it.
    /// </summary>
    public static void UseRecording(this WebApplication app)
    {
        var log = app.Services.GetRequiredService<RequestLog>();
        var inFlight = app.Services.GetRequiredService<InFlight>();

        app.Use(async (context, next) =>
        {
            var request = context.Request;
            if (request.Path.StartsWithSegments("/_comptroller"))
            {
                await next();
                return;
            }

            var started = DateTime.UtcNow;
            var clock = System.Diagnostics.Stopwatch.StartNew();
            var user = context.Items[Identity.ProxyGate.UserItem] as string;
            var device = context.Items[Identity.ProxyGate.DeviceItem] as string;
            var path = request.Path.Value ?? "/";
            var usageExpected = HttpMethods.IsPost(request.Method)
                && string.Equals(path.TrimEnd('/'), "/v1/messages", StringComparison.OrdinalIgnoreCase);

            var id = inFlight.Add(user, device, request.Method, path);
            var tap = new TapStream(context.Response.Body, usageExpected ? new UsageReader(context.Response) : null);
            context.Response.Body = tap;
            try
            {
                await next();
            }
            finally
            {
                context.Response.Body = tap.Inner;
                inFlight.Remove(id);

                var status = context.Response.StatusCode;
                var error = context.Features.Get<IForwarderErrorFeature>()?.Error;
                var expected = usageExpected && status == 200 && error is null;
                log.Add(new RequestRecord(
                    started, user, device,
                    context.Connection.RemoteIpAddress?.ToString(),
                    request.Method, path, status, clock.ElapsedMilliseconds,
                    request.ContentLength, tap.Bytes,
                    error is null or ForwarderError.None ? null : error.ToString(),
                    expected ? tap.Reader?.Finish() : null,
                    expected));
            }
        });
    }
}
