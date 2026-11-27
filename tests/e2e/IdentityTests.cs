using System.Net;

namespace Comptroller.E2E;

public sealed class IdentityTests(Harness harness) : IClassFixture<Harness>
{
    private const string Past = "2000-01-01T00:00:00.000Z";

    private async Task<string> Message(HttpResponseMessage response) =>
        (await Harness.Json(response)).GetProperty("error").GetProperty("message").GetString()!;

    [Fact]
    public async Task A_code_works_once()
    {
        var code = harness.CreateCode("tester", "once");

        var first = await harness.Send(HttpMethod.Post, "/_comptroller/connect", body: new { code = code.ToLowerInvariant() });
        var second = await harness.Send(HttpMethod.Post, "/_comptroller/connect", body: new { code });

        Assert.Equal(HttpStatusCode.OK, first.StatusCode);
        Assert.Equal(HttpStatusCode.Forbidden, second.StatusCode);
    }

    [Fact]
    public async Task Disconnect_blocks_until_reconnect_and_refresh_does_not_undo_it()
    {
        var token = await harness.Enroll("tray");

        await harness.Send(HttpMethod.Post, "/_comptroller/disconnect", token);
        var blocked = await harness.Models(token);
        Assert.Equal(HttpStatusCode.Forbidden, blocked.StatusCode);
        Assert.Equal("Disconnected from the tray on tray. Click Reconnect.", await Message(blocked));

        var me = await Harness.Json(await harness.Send(HttpMethod.Get, "/_comptroller/me", token));
        Assert.Equal("suspended", me.GetProperty("status").GetString());

        var refresh = await harness.Send(HttpMethod.Post, "/_comptroller/connect", token);
        Assert.Equal(HttpStatusCode.Forbidden, refresh.StatusCode);

        var reconnect = await harness.Send(HttpMethod.Post, "/_comptroller/reconnect", token);
        Assert.Equal(HttpStatusCode.OK, reconnect.StatusCode);
        Assert.Equal(HttpStatusCode.OK, (await harness.Models(token)).StatusCode);
    }

    [Fact]
    public async Task Reconnect_does_not_undo_an_admin_suspension()
    {
        var token = await harness.Enroll("admin-suspended");
        harness.Sql("UPDATE devices SET status = 'suspended', suspended_by = 'admin' WHERE name = 'admin-suspended'");

        await harness.Send(HttpMethod.Post, "/_comptroller/disconnect", token);
        var reconnect = await harness.Send(HttpMethod.Post, "/_comptroller/reconnect", token);

        Assert.Equal(HttpStatusCode.Forbidden, reconnect.StatusCode);
        Assert.Equal(HttpStatusCode.Forbidden, (await harness.Models(token)).StatusCode);
    }

    [Fact]
    public async Task Refresh_keeps_the_same_token_and_an_expired_one_is_refused()
    {
        var token = await harness.Enroll("expiry");

        var refresh = await Harness.Json(await harness.Send(HttpMethod.Post, "/_comptroller/connect", token));
        Assert.Equal(token, refresh.GetProperty("token").GetString());

        harness.Sql("UPDATE devices SET expires_utc = $past WHERE name = 'expiry'", ("$past", Past));
        var response = await harness.Models(token);
        Assert.Equal(HttpStatusCode.Forbidden, response.StatusCode);
        Assert.Contains("expired", await Message(response));
        Assert.Equal(HttpStatusCode.Forbidden, (await harness.Send(HttpMethod.Post, "/_comptroller/connect", token)).StatusCode);
    }

    [Fact]
    public async Task Endpoints_require_the_client_header()
    {
        var response = await harness.Client.GetAsync("/_comptroller/me");

        Assert.Equal(HttpStatusCode.BadRequest, response.StatusCode);
    }
}

// These change the config, so each class gets its own service.
public sealed class WindowsModeTests(Harness harness) : IClassFixture<Harness>
{
    private Task UseWindows(string group) => harness.Reconfigure(
        config =>
        {
            config["Identity"]!["Mode"] = "windows";
            config["Identity"]!["AllowedGroup"] = group;
        },
        // GET connect answers 400 only while still in manual mode.
        async () => (await harness.Send(HttpMethod.Get, "/_comptroller/connect?device=probe")).StatusCode != HttpStatusCode.BadRequest);

    private Task<HttpResponseMessage> Connect(string? token = null) =>
        harness.Send(HttpMethod.Get, "/_comptroller/connect?device=win", token, client: harness.WindowsClient);

    [Fact]
    public async Task Windows_sign_in_issues_a_token_and_leaving_the_group_cuts_it_off()
    {
        await UseWindows(Harness.EveryoneGroup);

        var first = await Connect();
        Assert.Equal(HttpStatusCode.OK, first.StatusCode);
        var session = await Harness.Json(first);
        var token = session.GetProperty("token").GetString()!;
        Assert.Equal(Environment.UserDomainName + "\\" + Environment.UserName, session.GetProperty("user").GetString(), ignoreCase: true);
        Assert.Equal(HttpStatusCode.OK, (await harness.Models(token)).StatusCode);

        // Refresh keeps the token.
        Assert.Equal(token, (await Harness.Json(await Connect(token))).GetProperty("token").GetString());

        // Disconnect, then reconnect with Windows sign-in.
        await harness.Send(HttpMethod.Post, "/_comptroller/disconnect", token);
        Assert.Equal(HttpStatusCode.Forbidden, (await Connect(token)).StatusCode);
        var reconnect = await harness.Send(HttpMethod.Post, "/_comptroller/reconnect?device=win", token, client: harness.WindowsClient);
        Assert.Equal(HttpStatusCode.OK, reconnect.StatusCode);
        Assert.Equal(HttpStatusCode.OK, (await harness.Models(token)).StatusCode);

        // A group nobody here is in.
        await harness.Reconfigure(
            config =>
            {
                config["Identity"]!["Mode"] = "windows";
                config["Identity"]!["AllowedGroup"] = "S-1-5-21-1-2-3-4999";
            },
            async () => (await Connect(token)).StatusCode == HttpStatusCode.Forbidden);
        Assert.Equal(HttpStatusCode.Forbidden, (await harness.Models(token)).StatusCode);
    }
}

public sealed class ConfigTests(Harness harness) : IClassFixture<Harness>
{
    [Fact]
    public async Task An_invalid_config_keeps_the_last_good_one()
    {
        var token = await harness.Enroll("config");

        File.WriteAllText(harness.ConfigPath, "{ broken");
        await Task.Delay(1500);

        Assert.Equal(HttpStatusCode.OK, (await harness.Models(token)).StatusCode);
        // The other test here shares this service and needs a readable file.
        harness.Configure();
    }

    [Fact]
    public async Task Addresses_outside_the_allow_list_are_refused()
    {
        var token = await harness.Enroll("cidr");

        await harness.Reconfigure(
            config => config["Access"]!["AllowedCidrs"] = new System.Text.Json.Nodes.JsonArray("10.255.255.0/24"),
            async () => (await harness.Models(token)).StatusCode == HttpStatusCode.Forbidden);

        await harness.Reconfigure(null, async () => (await harness.Models(token)).StatusCode == HttpStatusCode.OK);
    }
}
