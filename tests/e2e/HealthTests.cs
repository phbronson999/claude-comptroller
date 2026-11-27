using System.Net;
using System.Text.Json;

namespace Comptroller.E2E;

public sealed class HealthTests(Harness harness) : IClassFixture<Harness>
{
    private async Task<JsonElement> Health() => await Harness.Json(await harness.Admin.GetAsync("/health"));

    [Fact]
    public async Task Reports_the_running_service()
    {
        var health = await Health();

        Assert.Equal("0.1.0", health.GetProperty("version").GetString());
        Assert.Equal(1, health.GetProperty("schemaVersion").GetInt32());
        Assert.False(health.GetProperty("windowsService").GetBoolean());
        Assert.Equal(harness.Client.BaseAddress!.ToString().TrimEnd('/'), health.GetProperty("listenUrl").GetString());
        Assert.True(health.GetProperty("uptimeSeconds").GetInt64() >= 0);
    }

    [Fact]
    public async Task A_rejected_config_is_reported_until_it_is_fixed()
    {
        File.WriteAllText(harness.ConfigPath, "{ broken");
        await WaitForStatus("error");
        Assert.NotEmpty((await Health()).GetProperty("config").GetProperty("error").GetString()!);

        harness.Configure();
        await WaitForStatus("ok");
    }

    [Fact]
    public async Task The_two_listeners_stay_apart()
    {
        // The admin port never proxies, and the public port doesn't answer health itself.
        Assert.Equal(HttpStatusCode.NotFound, (await harness.Admin.GetAsync("/v1/models")).StatusCode);
        Assert.Equal(HttpStatusCode.Unauthorized, (await harness.Client.GetAsync("/health")).StatusCode);
    }

    private async Task WaitForStatus(string status)
    {
        for (var i = 0; i < 100; i++)
        {
            if ((await Health()).GetProperty("config").GetProperty("status").GetString() == status) return;
            await Task.Delay(100);
        }

        Assert.Fail($"Config status never became {status}.");
    }
}
