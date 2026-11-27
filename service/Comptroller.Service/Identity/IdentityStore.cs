using System.Globalization;
using Microsoft.Data.Sqlite;

namespace Comptroller.Service.Identity;

public sealed record Device(
    long Id,
    long UserId,
    string User,
    bool UserRevoked,
    string Name,
    string? TokenHash,
    string Status,
    string? SuspendedBy,
    DateTime ExpiresUtc)
{
    public bool Expired => ExpiresUtc <= DateTime.UtcNow;
    public string EffectiveStatus => UserRevoked ? "revoked" : Status;
}

/// <summary>Users, devices and one-time codes. See contracts/schema.sql.</summary>
public sealed class IdentityStore
{
    public const int SchemaVersion = 1;

    private const string TimeFormat = "yyyy-MM-ddTHH:mm:ss.fffZ";

    private const string SelectDevice = """
        SELECT d.id, d.user_id, u.name, u.status, d.name, d.token_hash, d.status, d.suspended_by, d.expires_utc
        FROM devices d JOIN users u ON u.id = d.user_id
        """;

    private readonly string _connectionString;

    public IdentityStore(string databasePath)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(databasePath)!);
        _connectionString = new SqliteConnectionStringBuilder
        {
            DataSource = databasePath,
            ForeignKeys = true,
            DefaultTimeout = 5,
        }.ToString();
        CreateSchema();
    }

    public static string Format(DateTime utc) => utc.ToString(TimeFormat, CultureInfo.InvariantCulture);

    private static DateTime Parse(string value) => DateTime.Parse(
        value, CultureInfo.InvariantCulture, DateTimeStyles.AdjustToUniversal | DateTimeStyles.AssumeUniversal);

    private static string Now => Format(DateTime.UtcNow);

    public Device? FindByToken(string token) =>
        QueryDevice($"{SelectDevice} WHERE d.token_hash = $hash", ("$hash", Tokens.Hash(token)));

    public Device? FindByUserAndName(string user, string deviceName) =>
        QueryDevice($"{SelectDevice} WHERE u.name = $user AND d.name = $device", ("$user", user), ("$device", deviceName));

    /// <summary>Returns the user's id, creating the user on first sight.</summary>
    public (long Id, bool Revoked) EnsureUser(string name, string source)
    {
        Execute("INSERT INTO users (name, source, created_utc) VALUES ($name, $source, $now) ON CONFLICT (name) DO NOTHING",
            ("$name", name), ("$source", source), ("$now", Now));

        using var connection = Open();
        using var command = Command(connection, "SELECT id, status FROM users WHERE name = $name", ("$name", name));
        using var reader = command.ExecuteReader();
        reader.Read();
        return (reader.GetInt64(0), reader.GetString(1) == "revoked");
    }

    /// <summary>Gives the device a new token and makes it active, creating the device if needed.</summary>
    public void IssueToken(long userId, string deviceName, string token, DateTime expiresUtc) => Execute("""
        INSERT INTO devices (user_id, name, token_hash, status, created_utc, expires_utc)
        VALUES ($user, $device, $hash, 'active', $now, $expires)
        ON CONFLICT (user_id, name) DO UPDATE
        SET token_hash = excluded.token_hash, expires_utc = excluded.expires_utc, status = 'active', suspended_by = NULL
        """,
        ("$user", userId), ("$device", deviceName), ("$hash", Tokens.Hash(token)), ("$now", Now), ("$expires", Format(expiresUtc)));

    public void Extend(long deviceId, DateTime expiresUtc) =>
        Execute("UPDATE devices SET expires_utc = $expires WHERE id = $id",
            ("$id", deviceId), ("$expires", Format(expiresUtc)));

    /// <summary>The user's own disconnect. Leaves admin suspensions and revocations alone.</summary>
    public void SuspendFromTray(long deviceId) =>
        Execute("UPDATE devices SET status = 'suspended', suspended_by = 'tray' WHERE id = $id AND status = 'active'",
            ("$id", deviceId));

    public void Reactivate(long deviceId) =>
        Execute("UPDATE devices SET status = 'active', suspended_by = NULL WHERE id = $id AND status = 'suspended' AND suspended_by = 'tray'",
            ("$id", deviceId));

    public void ExpireAll(string user) =>
        Execute("UPDATE devices SET expires_utc = $now WHERE expires_utc > $now AND user_id = (SELECT id FROM users WHERE name = $user)",
            ("$user", user), ("$now", Now));

    public void Touch(long deviceId) =>
        Execute("UPDATE devices SET last_seen_utc = $now WHERE id = $id", ("$id", deviceId), ("$now", Now));

    public string CreateCode(long userId, string deviceName, TimeSpan lifetime)
    {
        var code = Tokens.NewCode();
        Execute("INSERT INTO enrollment_codes (code_hash, user_id, device_name, created_utc, expires_utc) VALUES ($hash, $user, $device, $now, $expires)",
            ("$hash", Tokens.HashCode(code)), ("$user", userId), ("$device", deviceName), ("$now", Now),
            ("$expires", Format(DateTime.UtcNow + lifetime)));
        return code;
    }

    /// <summary>Marks the code used, in one statement so it can't be redeemed twice.</summary>
    public (long UserId, string DeviceName)? RedeemCode(string code)
    {
        using var connection = Open();
        using var command = Command(connection,
            "UPDATE enrollment_codes SET used_utc = $now WHERE code_hash = $hash AND used_utc IS NULL AND expires_utc > $now RETURNING user_id, device_name",
            ("$hash", Tokens.HashCode(code)), ("$now", Now));
        using var reader = command.ExecuteReader();
        return reader.Read() ? (reader.GetInt64(0), reader.GetString(1)) : null;
    }

    public (string Name, bool Revoked)? FindUser(long userId)
    {
        using var connection = Open();
        using var command = Command(connection, "SELECT name, status FROM users WHERE id = $id", ("$id", userId));
        using var reader = command.ExecuteReader();
        return reader.Read() ? (reader.GetString(0), reader.GetString(1) == "revoked") : null;
    }

    private void CreateSchema()
    {
        using var connection = Open();
        // auto_vacuum only takes effect if set before the first table exists.
        using (var pragma = Command(connection, "PRAGMA auto_vacuum = INCREMENTAL; PRAGMA journal_mode = WAL;"))
        {
            pragma.ExecuteNonQuery();
        }

        using var stream = typeof(IdentityStore).Assembly.GetManifestResourceStream("schema.sql")!;
        using var sql = new StreamReader(stream);
        using var transaction = connection.BeginTransaction();
        using (var create = Command(connection, sql.ReadToEnd()))
        {
            create.ExecuteNonQuery();
        }

        using (var read = Command(connection, "SELECT MAX(version) FROM schema_version"))
        {
            var version = read.ExecuteScalar();
            if (version is long existing && existing > SchemaVersion)
            {
                throw new InvalidOperationException(
                    $"The database is schema version {existing}, newer than this service understands ({SchemaVersion}).");
            }

            if (version is not long)
            {
                using var insert = Command(connection, "INSERT INTO schema_version (version) VALUES ($v)", ("$v", SchemaVersion));
                insert.ExecuteNonQuery();
            }
        }

        transaction.Commit();
    }

    private SqliteConnection Open()
    {
        var connection = new SqliteConnection(_connectionString);
        connection.Open();
        return connection;
    }

    private static SqliteCommand Command(SqliteConnection connection, string sql, params (string Name, object Value)[] parameters)
    {
        var command = connection.CreateCommand();
        command.CommandText = sql;
        foreach (var (name, value) in parameters)
        {
            command.Parameters.AddWithValue(name, value);
        }

        return command;
    }

    private void Execute(string sql, params (string Name, object Value)[] parameters)
    {
        using var connection = Open();
        using var command = Command(connection, sql, parameters);
        command.ExecuteNonQuery();
    }

    private Device? QueryDevice(string sql, params (string Name, object Value)[] parameters)
    {
        using var connection = Open();
        using var command = Command(connection, sql, parameters);
        using var r = command.ExecuteReader();
        if (!r.Read()) return null;

        return new Device(
            r.GetInt64(0), r.GetInt64(1), r.GetString(2), r.GetString(3) == "revoked", r.GetString(4),
            r.IsDBNull(5) ? null : r.GetString(5), r.GetString(6), r.IsDBNull(7) ? null : r.GetString(7),
            Parse(r.GetString(8)));
    }
}
