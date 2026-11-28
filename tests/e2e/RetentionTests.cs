using System.Globalization;
using Microsoft.Data.Sqlite;

namespace Comptroller.E2E;

/// <summary>A database with requests from long ago and from today, made before the service starts.</summary>
public sealed class OldDataHarness : Harness
{
    protected override void BeforeStart(string databasePath)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(databasePath)!);
        using var connection = new SqliteConnection($"Data Source={databasePath}");
        connection.Open();
        using var command = connection.CreateCommand();
        var at = (DateTime local) => local.ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ss.fffZ", CultureInfo.InvariantCulture);
        command.CommandText = File.ReadAllText(Path.Combine(RepositoryRoot(), "contracts", "schema.sql")) + $"""
            INSERT INTO schema_version (version) VALUES (3);
            INSERT INTO requests (started_utc, user_name, method, path, status, duration_ms, response_bytes) VALUES
              ('{at(DateTime.Now.AddDays(-30))}', 'old', 'GET', '/v1/models', 200, 1, 1),
              ('{at(DateTime.Today.AddDays(-13).AddMinutes(1))}', 'kept', 'GET', '/v1/models', 200, 1, 1),
              ('{at(DateTime.Now)}', 'new', 'GET', '/v1/models', 200, 1, 1);
            """;
        command.ExecuteNonQuery();
    }
}

public sealed class RetentionTests(OldDataHarness harness) : IClassFixture<OldDataHarness>
{
    [Fact]
    public async Task Requests_older_than_the_retention_are_pruned_on_start()
    {
        // The prune at start runs a moment after the service is up; /health reports it once done.
        var retention = default(System.Text.Json.JsonElement);
        for (var i = 0; i < 50; i++)
        {
            retention = (await Harness.Json(await harness.Admin.GetAsync("/health"))).GetProperty("retention");
            if (retention.GetProperty("lastPrune").ValueKind != System.Text.Json.JsonValueKind.Null) break;
            await Task.Delay(200);
        }

        // The default keeps 14 days: today and the 13 before it.
        var left = harness.Rows("SELECT user_name FROM requests WHERE user_name IN ('old', 'kept', 'new') ORDER BY id").Select(r => r["user_name"]);
        Assert.Equal(["kept", "new"], left);

        Assert.Equal(14, retention.GetProperty("days").GetInt32());
        Assert.Equal(1, retention.GetProperty("lastPrune").GetProperty("removed").GetInt64());
        // Read as UTC: parsed as local time, the comparison fails in the evening west of UTC.
        var next = DateTime.Parse(retention.GetProperty("nextPruneUtc").GetString()!, CultureInfo.InvariantCulture, DateTimeStyles.AdjustToUniversal | DateTimeStyles.AssumeUniversal);
        Assert.True(next > DateTime.UtcNow.AddMinutes(-1), $"next prune {next:u} is in the past");
    }
}
