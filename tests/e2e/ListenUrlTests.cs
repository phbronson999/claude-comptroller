namespace Comptroller.E2E;

public sealed class ListenUrlTests(Harness harness) : IClassFixture<Harness>
{
    [Theory]
    [InlineData("http://+:9")]
    [InlineData("http://*:9")]
    [InlineData("http://0.0.0.0:9")]
    public async Task A_listen_url_for_every_address_is_accepted(string listenUrl)
    {
        // The listen URL itself only applies at the next start; the organization name changes
        // at once, and only if the file is accepted.
        var marker = "accepted " + listenUrl;
        await harness.Reconfigure(
            config =>
            {
                config["Proxy"]!["ListenUrl"] = listenUrl;
                config["General"] = new System.Text.Json.Nodes.JsonObject { ["OrganizationName"] = marker };
            },
            async () => (await Harness.Json(await harness.Send(HttpMethod.Get, "/_comptroller/info"))).GetProperty("organizationName").GetString() == marker);
    }
}
