using Comptroller.Service.Config;
using Comptroller.Service.Identity;
using Microsoft.Data.Sqlite;
using Microsoft.Extensions.Options;

namespace Comptroller.Service.Recording;

/// <summary>What the last prune did, for /health.</summary>
public sealed record PruneResult(DateTime FinishedUtc, long Removed, long FreedBytes, string? Error);

/// <summary>
/// Keeps the black box to Data.RetentionDays: today and the days before it, by the server's
/// local calendar. Prunes once at start (in case the service was off at prune time), then
/// nightly at Data.PruneTime. Old rows go in small batches so a request's row is never held
/// up for long; then the freed pages go back to the disk (incremental vacuum).
/// </summary>
public sealed class Pruner(string databasePath, IOptionsMonitor<ComptrollerConfig> options) : BackgroundService
{
    private const int Batch = 5_000;

    private readonly string _connectionString = new SqliteConnectionStringBuilder { DataSource = databasePath, DefaultTimeout = 30 }.ToString();

    public PruneResult? Last { get; private set; }

    /// <summary>When the next nightly prune runs, local time.</summary>
    public DateTime Next => NextRun(DateTime.Now, options.CurrentValue.Data.PruneAt);

    /// <summary>The first local day kept, as the UTC instant it starts.</summary>
    public static DateTime Cutoff(DateTime nowLocal, int retentionDays) =>
        nowLocal.Date.AddDays(1 - retentionDays).ToUniversalTime();

    public static DateTime NextRun(DateTime nowLocal, TimeOnly at)
    {
        var today = nowLocal.Date + at.ToTimeSpan();
        return today > nowLocal ? today : today.AddDays(1);
    }

    protected override async Task ExecuteAsync(CancellationToken stopping)
    {
        try
        {
            // Let the service settle before the first prune.
            await Task.Delay(TimeSpan.FromSeconds(2), stopping);
            Prune();
            while (!stopping.IsCancellationRequested)
            {
                // Checked every minute, so a changed prune time takes effect tonight.
                var next = Next;
                var wait = next - DateTime.Now;
                if (wait > TimeSpan.FromMinutes(1))
                {
                    await Task.Delay(TimeSpan.FromMinutes(1), stopping);
                    continue;
                }

                await Task.Delay(wait < TimeSpan.Zero ? TimeSpan.Zero : wait, stopping);
                Prune();
                // Past the prune minute, so it doesn't run twice.
                await Task.Delay(TimeSpan.FromSeconds(61), stopping);
            }
        }
        catch (OperationCanceledException)
        {
            // Stopping.
        }
    }

    public PruneResult Prune()
    {
        var days = options.CurrentValue.Data.RetentionDays;
        var cutoff = IdentityStore.Format(Cutoff(DateTime.Now, days));
        long removed = 0;
        try
        {
            var before = Size();
            using var connection = new SqliteConnection(_connectionString);
            connection.Open();
            while (true)
            {
                using var delete = connection.CreateCommand();
                delete.CommandText = "DELETE FROM requests WHERE id IN (SELECT id FROM requests WHERE started_utc < $cutoff LIMIT $batch)";
                delete.Parameters.AddWithValue("$cutoff", cutoff);
                delete.Parameters.AddWithValue("$batch", Batch);
                var n = delete.ExecuteNonQuery();
                removed += n;
                if (n < Batch) break;
            }

            using (var codes = connection.CreateCommand())
            {
                // Codes that expired before the cutoff are no use to anyone.
                codes.CommandText = "DELETE FROM enrollment_codes WHERE expires_utc < $cutoff";
                codes.Parameters.AddWithValue("$cutoff", cutoff);
                codes.ExecuteNonQuery();
            }

            using (var vacuum = connection.CreateCommand())
            {
                vacuum.CommandText = "PRAGMA incremental_vacuum; PRAGMA wal_checkpoint(TRUNCATE);";
                vacuum.ExecuteNonQuery();
            }

            Last = new PruneResult(DateTime.UtcNow, removed, Math.Max(0, before - Size()), null);
            Serilog.Log.Information("Pruned {Removed} requests before {Cutoff}, keeping {Days} days", removed, cutoff, days);
        }
        catch (SqliteException ex)
        {
            Last = new PruneResult(DateTime.UtcNow, removed, 0, ex.Message);
            Serilog.Log.Warning(ex, "Couldn't prune the black box");
        }

        return Last;
    }

    private long Size()
    {
        long Length(string path) => File.Exists(path) ? new FileInfo(path).Length : 0;
        return Length(databasePath) + Length(databasePath + "-wal");
    }
}
