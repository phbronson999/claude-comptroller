// Fills the database with realistic traffic from several example people, so the Dashboard,
// Radar, Cost and Users screens can be tried without waiting weeks for real use.
//
//   dotnet run --project tools/seed-db -- --config dev/comptroller.json [--days 21] [--seat-plan "Team standard"]
//
// The people are EXAMPLE\maverick, EXAMPLE\goose, EXAMPLE\iceman and EXAMPLE\viper. Running it
// again replaces everyone under EXAMPLE\ and their requests; nothing else in the database is touched. It creates the
// database from contracts/schema.sql if there isn't one yet.
using System.Globalization;
using System.Text.Json;
using Microsoft.Data.Sqlite;

var configArg = Array.IndexOf(args, "--config");
if (configArg < 0 || configArg + 1 >= args.Length)
{
    Console.Error.WriteLine("Usage: seed-db --config <comptroller.json> [--days 21]");
    return 1;
}

var configPath = Path.GetFullPath(args[configArg + 1]);
var daysArg = Array.IndexOf(args, "--days");
var days = daysArg >= 0 && daysArg + 1 < args.Length ? int.Parse(args[daysArg + 1], CultureInfo.InvariantCulture) : 21;
// Puts everyone seeded on this seat plan (a name from Cost.SeatPlans).
var planArg = Array.IndexOf(args, "--seat-plan");
var seatPlan = planArg >= 0 && planArg + 1 < args.Length ? args[planArg + 1] : null;

// The data folder, relative to the config file, as the service resolves it.
var dataFolder = "data";
using (var doc = JsonDocument.Parse(File.ReadAllText(configPath), new JsonDocumentOptions { CommentHandling = JsonCommentHandling.Skip, AllowTrailingCommas = true }))
{
    foreach (var section in doc.RootElement.EnumerateObject().Where(p => p.Name.Equals("Data", StringComparison.OrdinalIgnoreCase)))
    {
        foreach (var folder in section.Value.EnumerateObject().Where(p => p.Name.Equals("Folder", StringComparison.OrdinalIgnoreCase)))
        {
            dataFolder = folder.Value.GetString() ?? dataFolder;
        }
    }
}

var dbPath = Path.Combine(Path.GetFullPath(dataFolder, Path.GetDirectoryName(configPath)!), "comptroller.db");
Directory.CreateDirectory(Path.GetDirectoryName(dbPath)!);
using var connection = new SqliteConnection(new SqliteConnectionStringBuilder { DataSource = dbPath, ForeignKeys = true }.ToString());
connection.Open();

void Run(string sql, params (string Name, object? Value)[] parameters)
{
    using var command = connection.CreateCommand();
    command.CommandText = sql;
    foreach (var (name, value) in parameters) command.Parameters.AddWithValue(name, value ?? DBNull.Value);
    command.ExecuteNonQuery();
}

object? Scalar(string sql)
{
    using var command = connection.CreateCommand();
    command.CommandText = sql;
    return command.ExecuteScalar();
}

// A new database gets the shipped schema, at the version it describes.
if (Scalar("SELECT COUNT(*) FROM sqlite_master WHERE name = 'schema_version'") is 0L)
{
    using var stream = typeof(Program).Assembly.GetManifestResourceStream("schema.sql")!;
    Run("PRAGMA auto_vacuum = INCREMENTAL; PRAGMA journal_mode = WAL;");
    Run(new StreamReader(stream).ReadToEnd());
    Run("INSERT INTO schema_version (version) VALUES (3)");
    Console.WriteLine($"Created {dbPath}");
}

const string Format = "yyyy-MM-ddTHH:mm:ss.fffZ";
static string Utc(DateTime time) => time.ToUniversalTime().ToString(Format, CultureInfo.InvariantCulture);

// Who, which computers, how busy, which models they lean on, and how often they hit a limit.
var people = new[]
{
    (Name: @"EXAMPLE\iceman", Devices: new[] { "ICEMAN-LAPTOP", "ICEMAN-DESKTOP" }, PerDay: 140, Opus: 0.45, LimitRate: 0.004),
    (Name: @"EXAMPLE\goose", Devices: new[] { "GOOSE-LAPTOP" }, PerDay: 90, Opus: 0.2, LimitRate: 0.002),
    (Name: @"EXAMPLE\viper", Devices: new[] { "VIPER-LAPTOP" }, PerDay: 35, Opus: 0.1, LimitRate: 0.0),
    (Name: @"EXAMPLE\maverick", Devices: new[] { "MAVERICK-LAPTOP" }, PerDay: 110, Opus: 0.6, LimitRate: 0.02),
};

using var transaction = connection.BeginTransaction();
Run(@"DELETE FROM requests WHERE user_name LIKE 'EXAMPLE\%'");
Run(@"DELETE FROM users WHERE name LIKE 'EXAMPLE\%'");

var random = new Random(7);
var now = DateTime.Now;
var start = now.Date.AddDays(1 - days);
var count = 0;
// Requests are written in time order at the end, as the service would have recorded them,
// so the newest rows are a mix of people rather than all the last person's.
var queued = new List<(DateTime At, string Sql, (string Name, object? Value)[] Parameters)>();
void Queue(DateTime at, string sql, params (string Name, object? Value)[] parameters) => queued.Add((at, sql, parameters));
foreach (var person in people)
{
    Run("INSERT INTO users (name, source, created_utc, seat_plan) VALUES ($name, 'windows', $created, $plan)",
        ("$name", person.Name), ("$created", Utc(start)), ("$plan", seatPlan));
    var userId = (long)Scalar("SELECT last_insert_rowid()")!;
    foreach (var device in person.Devices)
    {
        Run("""
            INSERT INTO devices (user_id, name, token_hash, status, created_utc, last_seen_utc, expires_utc)
            VALUES ($user, $device, NULL, 'active', $created, $seen, $expires)
            """,
            ("$user", userId), ("$device", device), ("$created", Utc(start)), ("$seen", Utc(now.AddMinutes(-random.Next(1, 90)))),
            ("$expires", Utc(now.AddDays(6))));
    }

    for (var day = start; day <= now.Date; day = day.AddDays(1))
    {
        if (day.DayOfWeek is DayOfWeek.Saturday or DayOfWeek.Sunday && random.NextDouble() < 0.85) continue;
        var requests = (int)(person.PerDay * (0.6 + random.NextDouble() * 0.8));
        for (var i = 0; i < requests; i++)
        {
            // Working hours, busiest mid-morning and mid-afternoon.
            var hour = random.NextDouble() < 0.5 ? 9 + random.NextDouble() * 3.5 : 13 + random.NextDouble() * 4.5;
            var at = day.AddHours(hour).AddSeconds(random.Next(0, 60));
            if (at > now) continue;
            var device = person.Devices[random.Next(person.Devices.Length)];
            var roll = random.NextDouble();

            if (roll < 0.06)
            {
                // Model lists and token counts: no usage.
                var countTokens = roll < 0.03;
                Queue(at, """
                    INSERT INTO requests (started_utc, user_name, device_name, client_ip, method, path, status, duration_ms, request_bytes, response_bytes)
                    VALUES ($at, $user, $device, '10.0.0.20', $method, $path, 200, $ms, $in, $out)
                    """,
                    ("$at", Utc(at)), ("$user", person.Name), ("$device", device), ("$method", countTokens ? "POST" : "GET"),
                    ("$path", countTokens ? "/v1/messages/count_tokens" : "/v1/models"), ("$ms", random.Next(40, 300)),
                    ("$in", countTokens ? random.Next(2_000, 90_000) : null), ("$out", random.Next(200, 3_000)));
                count++;
                continue;
            }

            var limit = random.NextDouble() < person.LimitRate;
            var failure = !limit && random.NextDouble() < 0.004;
            var cancelled = !limit && !failure && random.NextDouble() < 0.02;
            var model = random.NextDouble() < person.Opus ? "claude-opus-5-5" : random.NextDouble() < 0.85 ? "claude-sonnet-5-5" : "claude-haiku-5-5";
            var input = random.Next(20, 3_000);
            var cacheRead = random.Next(15_000, 160_000);
            var cacheWrite5m = random.Next(500, 12_000);
            var cacheWrite1h = random.NextDouble() < 0.2 ? random.Next(1_000, 20_000) : 0;
            var output = random.Next(80, 4_500);
            var ms = limit ? random.Next(30, 120) : failure ? 600_000 : (int)(800 + output * (model == "claude-opus-5-5" ? 9.0 : 5.0) * (0.6 + random.NextDouble()));
            var status = limit ? 429 : failure ? 502 : 200;
            var parsed = status == 200 && !cancelled;
            Queue(at, """
                INSERT INTO requests (started_utc, user_name, device_name, client_ip, method, path, status, duration_ms, request_bytes, response_bytes,
                    proxy_error, model, input_tokens, output_tokens, cache_write_tokens, cache_write_5m_tokens, cache_write_1h_tokens, cache_read_tokens, usage)
                VALUES ($at, $user, $device, '10.0.0.20', 'POST', '/v1/messages', $status, $ms, $in, $out,
                    $error, $model, $input, $output, $write, $write5m, $write1h, $read, $usage)
                """,
                ("$at", Utc(at)), ("$user", person.Name), ("$device", device), ("$status", status), ("$ms", ms),
                ("$in", random.Next(20_000, 600_000)), ("$out", limit ? 220 : output * 6 + random.Next(500, 5_000)),
                ("$error", failure ? "RequestTimedOut" : cancelled ? "RequestCanceled" : null),
                ("$model", parsed ? model : null), ("$input", parsed ? input : null), ("$output", parsed ? output : null),
                ("$write", parsed ? cacheWrite5m + cacheWrite1h : null), ("$write5m", parsed ? cacheWrite5m : null),
                ("$write1h", parsed ? cacheWrite1h : null), ("$read", parsed ? cacheRead : null), ("$usage", parsed ? "parsed" : null));
            count++;
        }
    }
}

foreach (var (_, sql, parameters) in queued.OrderBy(q => q.At))
{
    Run(sql, parameters);
}

transaction.Commit();
Console.WriteLine($"Seeded {people.Length} people and {count:N0} requests over {days} days into {dbPath}");
return 0;
