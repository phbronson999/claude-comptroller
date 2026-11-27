using System.Diagnostics;
using System.Net;

namespace Comptroller.E2E;

public sealed class ProxyTests(Harness harness) : IClassFixture<Harness>
{
    [Fact]
    public async Task Streams_without_buffering()
    {
        var token = await harness.Enroll("stream");
        var clock = Stopwatch.StartNew();
        using var response = await harness.Send(HttpMethod.Post, "/v1/messages", token,
            new { model = "claude-mock-1", stream = true }, completion: HttpCompletionOption.ResponseHeadersRead);
        using var reader = new StreamReader(await response.Content.ReadAsStreamAsync());

        Assert.Equal("event: message_start", await reader.ReadLineAsync());
        var first = clock.Elapsed;
        await reader.ReadToEndAsync();

        // The mock spreads its events over more than a second.
        Assert.True(clock.Elapsed - first > TimeSpan.FromSeconds(1), $"first {first}, last {clock.Elapsed}");
    }

    [Fact]
    public async Task Credentials_pass_through_and_the_token_is_stripped()
    {
        var token = await harness.Enroll("headers");
        await harness.Upstream.DeleteAsync("/_mock/requests");

        await harness.Send(HttpMethod.Get, "/v1/models?probe=headers", token, headers:
            [("Authorization", "Bearer seat-login"), ("anthropic-beta", "oauth-2025-04-20,other")]);

        var seen = (await Harness.Json(await harness.Upstream.GetAsync("/_mock/requests"))).EnumerateArray()
            .Select(r => r.GetProperty("headers"))
            .Single(h => h.TryGetProperty("authorization", out _));
        Assert.Equal("Bearer seat-login", seen.GetProperty("authorization").GetString());
        Assert.Equal("oauth-2025-04-20,other", seen.GetProperty("anthropic-beta").GetString());
        Assert.False(seen.TryGetProperty("x-comptroller-token", out _));
        Assert.False(seen.TryGetProperty("x-forwarded-for", out _));
        Assert.False(seen.TryGetProperty("traceparent", out _));
    }

    [Theory]
    [InlineData(null)]
    [InlineData("not-a-real-token")]
    public async Task Missing_or_unknown_token_gets_401_in_anthropic_format(string? token)
    {
        var response = await harness.Models(token);

        Assert.Equal(HttpStatusCode.Unauthorized, response.StatusCode);
        var body = await Harness.Json(response);
        Assert.Equal("error", body.GetProperty("type").GetString());
        Assert.Equal("authentication_error", body.GetProperty("error").GetProperty("type").GetString());
        Assert.Contains("isn't connected", body.GetProperty("error").GetProperty("message").GetString());
    }

    [Fact]
    public async Task Only_the_hello_probe_needs_no_token()
    {
        var head = await harness.Client.SendAsync(new HttpRequestMessage(HttpMethod.Head, "/api/hello"));
        var get = await harness.Client.GetAsync("/api/hello");

        Assert.Equal(HttpStatusCode.OK, head.StatusCode);
        Assert.Equal(HttpStatusCode.Unauthorized, get.StatusCode);
    }

    [Fact]
    public async Task Own_paths_are_never_forwarded()
    {
        await harness.Upstream.DeleteAsync("/_mock/requests");

        var response = await harness.Send(HttpMethod.Get, "/_comptroller/nope");

        Assert.Equal(HttpStatusCode.NotFound, response.StatusCode);
        var forwarded = (await Harness.Json(await harness.Upstream.GetAsync("/_mock/requests"))).EnumerateArray()
            .Count(r => r.GetProperty("path").GetString()!.StartsWith("/_comptroller"));
        Assert.Equal(0, forwarded);
    }
}
