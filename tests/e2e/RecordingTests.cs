using System.Net;

namespace Comptroller.E2E;

/// <summary>Every proxied request is recorded with its metadata and, for replies, its token usage.</summary>
public sealed class RecordingTests(Harness harness) : IClassFixture<Harness>
{
    // What mock-anthropic reports for every /v1/messages reply.
    private static void AssertMockUsage(Dictionary<string, object?> row)
    {
        Assert.Equal("parsed", row["usage"]);
        Assert.Equal("claude-mock-1", row["model"]);
        Assert.Equal(25L, row["input_tokens"]);
        Assert.Equal(17L, row["output_tokens"]);
        Assert.Equal(300L, row["cache_write_tokens"]);
        Assert.Equal(100L, row["cache_write_5m_tokens"]);
        Assert.Equal(200L, row["cache_write_1h_tokens"]);
        Assert.Equal(1200L, row["cache_read_tokens"]);
    }

    [Fact]
    public async Task A_streamed_reply_is_recorded_with_its_usage()
    {
        var token = await harness.Enroll("rec-stream");
        using var response = await harness.Send(HttpMethod.Post, "/v1/messages", token, new { model = "claude-mock-1", stream = true });
        var body = await response.Content.ReadAsStringAsync();

        var row = Assert.Single(await harness.Recorded("rec-stream", 1));
        Assert.Equal("tester", row["user_name"]);
        Assert.Equal("POST", row["method"]);
        Assert.Equal("/v1/messages", row["path"]);
        Assert.Equal(200L, row["status"]);
        Assert.Equal((long)body.Length, row["response_bytes"]);
        Assert.Null(row["proxy_error"]);
        AssertMockUsage(row);
    }

    [Fact]
    public async Task A_plain_json_reply_is_recorded_with_its_usage()
    {
        var token = await harness.Enroll("rec-json");
        await harness.Send(HttpMethod.Post, "/v1/messages", token, new { model = "claude-mock-1" });

        AssertMockUsage(Assert.Single(await harness.Recorded("rec-json", 1)));
    }

    [Theory]
    [InlineData(true)]
    [InlineData(false)]
    public async Task A_compressed_reply_is_recorded_with_its_usage(bool stream)
    {
        var device = $"rec-gzip-{stream}";
        var token = await harness.Enroll(device);
        using var response = await harness.Send(HttpMethod.Post, "/v1/messages", token, new { model = "claude-mock-1", stream },
            headers: [("x-mock-encoding", "gzip")]);
        Assert.Contains("gzip", response.Content.Headers.ContentEncoding);

        AssertMockUsage(Assert.Single(await harness.Recorded(device, 1)));
    }

    [Fact]
    public async Task Other_requests_are_recorded_without_usage()
    {
        var token = await harness.Enroll("rec-other");
        await harness.Send(HttpMethod.Post, "/v1/messages/count_tokens", token, new { model = "claude-mock-1" });
        await harness.Send(HttpMethod.Post, "/v1/messages", token, new { model = "claude-mock-1" }, headers: [("x-mock-status", "429")]);
        await harness.Models(token);

        var rows = await harness.Recorded("rec-other", 3);
        Assert.Equal(["/v1/messages/count_tokens", "/v1/messages", "/v1/models"], rows.Select(r => r["path"]));
        Assert.Equal([200L, 429L, 200L], rows.Select(r => r["status"]));
        Assert.All(rows, r => Assert.Null(r["usage"]));
        Assert.All(rows, r => Assert.Null(r["input_tokens"]));
    }

    [Fact]
    public async Task A_request_shows_as_in_flight_until_it_finishes()
    {
        var token = await harness.Enroll("rec-flight");
        using var response = await harness.Send(HttpMethod.Post, "/v1/messages", token,
            new { model = "claude-mock-1", stream = true }, completion: HttpCompletionOption.ResponseHeadersRead);

        var health = await Harness.Json(await harness.Admin.GetAsync("/health"));
        var flying = health.GetProperty("inFlight").GetProperty("requests").EnumerateArray()
            .Single(r => r.GetProperty("device").GetString() == "rec-flight");
        Assert.Equal("/v1/messages", flying.GetProperty("path").GetString());
        Assert.Equal("tester", flying.GetProperty("user").GetString());

        await response.Content.ReadAsStringAsync();
        await harness.Recorded("rec-flight", 1);
        health = await Harness.Json(await harness.Admin.GetAsync("/health"));
        Assert.DoesNotContain(health.GetProperty("inFlight").GetProperty("requests").EnumerateArray(),
            r => r.GetProperty("device").GetString() == "rec-flight");
    }

    [Fact]
    public async Task Me_counts_this_computers_requests_today()
    {
        var token = await harness.Enroll("rec-me");
        await harness.Models(token);
        await harness.Send(HttpMethod.Post, "/v1/messages", token, new { model = "claude-mock-1" }, headers: [("x-mock-status", "429")]);
        await harness.Recorded("rec-me", 2);

        var me = await harness.Send(HttpMethod.Get, "/_comptroller/me", token);
        Assert.Equal(HttpStatusCode.OK, me.StatusCode);
        var today = (await Harness.Json(me)).GetProperty("today");
        Assert.Equal(2, today.GetProperty("requests").GetInt32());
        Assert.Equal(1, today.GetProperty("limitHits").GetInt32());
        Assert.NotNull(today.GetProperty("lastRequestUtc").GetString());
    }

    [Fact]
    public async Task Refused_and_own_requests_are_not_recorded()
    {
        await harness.Models("not-a-real-token");
        await harness.Send(HttpMethod.Get, "/_comptroller/info");
        await Task.Delay(500);

        Assert.Empty(harness.Rows("SELECT * FROM requests WHERE path LIKE '/_comptroller%' OR status = 401"));
    }
}
