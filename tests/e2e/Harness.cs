using System.Diagnostics;
using System.Net;
using System.Net.Sockets;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using Microsoft.Data.Sqlite;

namespace Comptroller.E2E;

/// <summary>A mock upstream and a service, each in its own process, on free ports with a temp config.</summary>
public class Harness : IAsyncLifetime
{
    public const string TokenHeader = "X-Comptroller-Token";
    /// <summary>BUILTIN\Users: every local account is in it.</summary>
    public const string EveryoneGroup = "S-1-5-32-545";

    private readonly List<Process> _processes = [];
    private readonly string _folder = Path.Combine(Path.GetTempPath(), "comptroller-e2e-" + Guid.NewGuid().ToString("N"));
    private readonly int _servicePort = FreePort();
    private readonly int _adminPort = FreePort();
    private string _serviceUrl => $"{(Https ? "https" : "http")}://localhost:{_servicePort}";

    /// <summary>Listen on https:// (the derived harness supplies the certificate in <see cref="Customize"/>).</summary>
    protected virtual bool Https => false;

    /// <summary>Changes every config this harness writes, before a test's own change.</summary>
    protected virtual void Customize(JsonNode config)
    {
    }

    /// <summary>How the test clients connect to the service.</summary>
    protected virtual HttpClientHandler NewHandler() => new();

    public string UpstreamUrl { get; } = $"http://localhost:{FreePort()}";
    public string ConfigPath => Path.Combine(_folder, "comptroller.json");
    public HttpClient Client { get; private set; } = null!;
    public HttpClient WindowsClient { get; private set; } = null!;
    public HttpClient Upstream { get; private set; } = null!;
    public HttpClient Admin { get; private set; } = null!;

    public async Task InitializeAsync()
    {
        Directory.CreateDirectory(_folder);
        Configure();
        BeforeStart(Path.Combine(_folder, "data", "comptroller.db"));

        Start("mock-anthropic", "--urls", UpstreamUrl, "--delay-ms", "100");
        Start("claude-comptroller-service", "--config", ConfigPath);

        Client = new HttpClient(NewHandler()) { BaseAddress = new Uri(_serviceUrl) };
        var windows = NewHandler();
        windows.UseDefaultCredentials = true;
        WindowsClient = new HttpClient(windows) { BaseAddress = new Uri(_serviceUrl) };
        Upstream = new HttpClient { BaseAddress = new Uri(UpstreamUrl) };
        Admin = new HttpClient { BaseAddress = new Uri($"http://127.0.0.1:{_adminPort}") };

        await WaitFor(() => Upstream.GetAsync("/v1/models"));
        await WaitFor(() => Client.SendAsync(new HttpRequestMessage(HttpMethod.Head, "/api/hello")));
    }

    public virtual Task DisposeAsync()
    {
        foreach (var process in _processes)
        {
            if (!process.HasExited) process.Kill(entireProcessTree: true);
            process.WaitForExit(5000);
        }

        SqliteConnection.ClearAllPools();
        try { Directory.Delete(_folder, recursive: true); }
        catch (IOException) { /* a log file can linger briefly */ }
        return Task.CompletedTask;
    }

    /// <summary>Runs before the service starts, with where its database will be.</summary>
    protected virtual void BeforeStart(string databasePath)
    {
    }

    /// <summary>The repository root, found from this test's build folder.</summary>
    public static string RepositoryRoot()
    {
        var root = new DirectoryInfo(AppContext.BaseDirectory);
        while (!File.Exists(Path.Combine(root.FullName, "dev.ps1"))) root = root.Parent!;
        return root.FullName;
    }

    /// <summary>Writes the config, then lets the caller change it. The service reloads it.</summary>
    public void Configure(Action<JsonNode>? change = null)
    {
        var config = JsonNode.Parse($$"""
            {
              "Proxy": { "ListenUrl": "{{_serviceUrl}}", "UpstreamUrl": "{{UpstreamUrl}}", "AdminPort": {{_adminPort}} },
              "Access": { "AllowedCidrs": ["127.0.0.1/32", "::1/128"] },
              "Identity": { "Mode": "manual", "AllowedGroup": "{{EveryoneGroup}}" },
              "Data": { "Folder": "data" },
              "Logging": { "Folder": "logs" }
            }
            """)!;
        Customize(config);
        change?.Invoke(config);
        File.WriteAllText(ConfigPath, config.ToJsonString());
    }

    /// <summary>Configures, then waits until the probe shows the service picked it up.</summary>
    public async Task Reconfigure(Action<JsonNode>? change, Func<Task<bool>> applied)
    {
        Configure(change);
        for (var i = 0; i < 100; i++)
        {
            if (await applied()) return;
            await Task.Delay(100);
        }

        throw new TimeoutException("The service didn't pick up the new config.");
    }

    public string CreateCode(string user, string device)
    {
        using var process = Process.Start(StartInfo("claude-comptroller-service", redirect: true, "--config", ConfigPath, "--create-code", user, device))!;
        var output = process.StandardOutput.ReadToEnd();
        process.WaitForExit();
        Assert.Equal(0, process.ExitCode);
        return output.Split('\n', StringSplitOptions.RemoveEmptyEntries)[^1].Trim();
    }

    /// <summary>Enrolls a new device in manual mode and returns its token.</summary>
    public async Task<string> Enroll(string device)
    {
        var response = await Send(HttpMethod.Post, "/_comptroller/connect", body: new { code = CreateCode("tester", device) });
        Assert.Equal(HttpStatusCode.OK, response.StatusCode);
        return (await Json(response)).GetProperty("token").GetString()!;
    }

    public Task<HttpResponseMessage> Send(HttpMethod method, string path, string? token = null, object? body = null,
        HttpClient? client = null, HttpCompletionOption completion = HttpCompletionOption.ResponseContentRead,
        params (string Name, string Value)[] headers)
    {
        var request = new HttpRequestMessage(method, path);
        if (path.StartsWith("/_comptroller")) request.Headers.Add("X-Comptroller-Client", "e2e");
        if (token is not null) request.Headers.Add(TokenHeader, token);
        foreach (var (name, value) in headers) request.Headers.TryAddWithoutValidation(name, value);
        if (body is not null) request.Content = new StringContent(JsonSerializer.Serialize(body), Encoding.UTF8, "application/json");
        return (client ?? Client).SendAsync(request, completion);
    }

    public Task<HttpResponseMessage> Models(string? token) => Send(HttpMethod.Get, "/v1/models", token);

    public static async Task<JsonElement> Json(HttpResponseMessage response) =>
        JsonSerializer.Deserialize<JsonElement>(await response.Content.ReadAsStringAsync());

    /// <summary>Changes the database the way the admin side would.</summary>
    public void Sql(string sql, params (string Name, object Value)[] parameters)
    {
        using var connection = new SqliteConnection($"Data Source={Path.Combine(_folder, "data", "comptroller.db")}");
        connection.Open();
        using var command = connection.CreateCommand();
        command.CommandText = sql;
        foreach (var (name, value) in parameters) command.Parameters.AddWithValue(name, value);
        command.ExecuteNonQuery();
    }

    /// <summary>Reads rows from the database as the admin side would.</summary>
    public List<Dictionary<string, object?>> Rows(string sql, params (string Name, object Value)[] parameters)
    {
        using var connection = new SqliteConnection($"Data Source={Path.Combine(_folder, "data", "comptroller.db")};Mode=ReadOnly");
        connection.Open();
        using var command = connection.CreateCommand();
        command.CommandText = sql;
        foreach (var (name, value) in parameters) command.Parameters.AddWithValue(name, value);
        using var reader = command.ExecuteReader();
        var rows = new List<Dictionary<string, object?>>();
        while (reader.Read())
        {
            rows.Add(Enumerable.Range(0, reader.FieldCount).ToDictionary(reader.GetName, i => reader.IsDBNull(i) ? null : reader.GetValue(i)));
        }

        return rows;
    }

    /// <summary>The requests recorded for a device, waiting for the background writer to catch up.</summary>
    public async Task<List<Dictionary<string, object?>>> Recorded(string device, int count)
    {
        for (var i = 0; i < 50; i++)
        {
            var rows = Rows("SELECT * FROM requests WHERE device_name = $device ORDER BY id", ("$device", device));
            if (rows.Count >= count) return rows;
            await Task.Delay(100);
        }

        throw new TimeoutException($"Fewer than {count} requests recorded for {device}.");
    }

    private void Start(string project, params string[] args) =>
        _processes.Add(Process.Start(StartInfo(project, redirect: false, args))!);

    private static ProcessStartInfo StartInfo(string project, bool redirect, params string[] args)
    {
        var info = new ProcessStartInfo("dotnet") { UseShellExecute = false, CreateNoWindow = true, RedirectStandardOutput = redirect };
        info.ArgumentList.Add(Dll(project));
        foreach (var arg in args) info.ArgumentList.Add(arg);
        return info;
    }

    /// <summary>The project's own build output, matching this test run's configuration.</summary>
    private static string Dll(string project)
    {
        var here = new DirectoryInfo(AppContext.BaseDirectory);
        var framework = here.Name;
        var configuration = here.Parent!.Name;
        var root = here;
        while (!File.Exists(Path.Combine(root.FullName, "dev.ps1"))) root = root.Parent!;

        var folder = project == "mock-anthropic" ? Path.Combine("tools", project) : Path.Combine("service", "Comptroller.Service");
        return Path.Combine(root.FullName, folder, "bin", configuration, framework, project + ".dll");
    }

    private static int FreePort()
    {
        var listener = new TcpListener(IPAddress.Loopback, 0);
        listener.Start();
        var port = ((IPEndPoint)listener.LocalEndpoint).Port;
        listener.Stop();
        return port;
    }

    private async Task WaitFor(Func<Task<HttpResponseMessage>> probe)
    {
        for (var i = 0; ; i++)
        {
            try
            {
                (await probe()).Dispose();
                return;
            }
            catch (HttpRequestException) when (i < 100)
            {
                if (_processes.FirstOrDefault(p => p.HasExited) is { } dead)
                    throw new InvalidOperationException($"{dead.StartInfo.ArgumentList[0]} exited with code {dead.ExitCode}.");
                await Task.Delay(200);
            }
        }
    }
}
