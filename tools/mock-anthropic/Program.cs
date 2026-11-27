// A fake api.anthropic.com for local testing. Listens on localhost only.
//
//   --urls http://localhost:5055   where to listen
//   --delay-ms 150                 pause between streamed events
//   --status 429                   make every /v1/* call fail with this status
//
// A single request can also ask for a failure with the header "x-mock-status: 529".
// GET /_mock/requests returns the headers of recent requests; DELETE clears them.
using System.Collections.Concurrent;
using System.Text.Json;

var builder = WebApplication.CreateSlimBuilder(args);
builder.Logging.SetMinimumLevel(LogLevel.Warning);
builder.WebHost.ConfigureKestrel(kestrel => kestrel.Limits.MaxRequestBodySize = null);
if (string.IsNullOrEmpty(builder.Configuration["urls"]))
{
    builder.WebHost.UseUrls("http://localhost:5055");
}

var delayMs = builder.Configuration.GetValue("delay-ms", 150);
var forcedStatus = builder.Configuration.GetValue("status", 0);
var seen = new ConcurrentQueue<object>();

var app = builder.Build();

app.Use(async (context, next) =>
{
    var request = context.Request;
    if (request.Path.StartsWithSegments("/_mock"))
    {
        await next();
        return;
    }

    seen.Enqueue(new
    {
        method = request.Method,
        path = request.Path.Value,
        headers = request.Headers.ToDictionary(h => h.Key.ToLowerInvariant(), h => h.Value.ToString()),
    });
    while (seen.Count > 100) seen.TryDequeue(out _);
    Console.WriteLine($"{request.Method} {request.Path}");

    var status = int.TryParse(request.Headers["x-mock-status"], out var asked) ? asked : forcedStatus;
    if (status >= 400 && request.Path.StartsWithSegments("/v1"))
    {
        var type = status switch
        {
            401 => "authentication_error",
            429 => "rate_limit_error",
            529 => "overloaded_error",
            _ => "api_error",
        };
        context.Response.StatusCode = status;
        if (status == 429) context.Response.Headers.RetryAfter = "30";
        await context.Response.WriteAsJsonAsync(new
        {
            type = "error",
            error = new { type, message = $"Mock {status} from mock-anthropic." },
        });
        return;
    }

    await next();
});

app.MapGet("/_mock/requests", () => seen.ToArray());
app.MapDelete("/_mock/requests", () => seen.Clear());

app.MapMethods("/api/hello", ["GET", "HEAD"], () => Results.Json(new { message = "hello" }));

app.MapGet("/v1/models", () => Results.Json(new
{
    data = new[] { new { type = "model", id = "claude-mock-1", display_name = "Mock model" } },
    has_more = false,
}));

app.MapPost("/v1/messages/count_tokens", () => Results.Json(new { input_tokens = 42 }));

app.MapPost("/v1/messages", async (HttpContext context) =>
{
    var cancel = context.RequestAborted;
    using var body = await JsonDocument.ParseAsync(context.Request.Body, cancellationToken: cancel);
    var root = body.RootElement;
    var model = root.TryGetProperty("model", out var m) ? m.GetString() : "claude-mock-1";
    var stream = root.TryGetProperty("stream", out var s) && s.ValueKind == JsonValueKind.True;

    string[] words = ["This ", "is ", "the ", "mock ", "upstream ", "replying ", "one ", "piece ", "at ", "a ", "time."];
    var usage = new
    {
        input_tokens = 25,
        cache_creation_input_tokens = 300,
        cache_read_input_tokens = 1200,
        cache_creation = new { ephemeral_5m_input_tokens = 100, ephemeral_1h_input_tokens = 200 },
        output_tokens = 1,
    };
    const int outputTokens = 17;

    if (!stream)
    {
        await context.Response.WriteAsJsonAsync(new
        {
            id = "msg_mock",
            type = "message",
            role = "assistant",
            model,
            content = new[] { new { type = "text", text = string.Concat(words) } },
            stop_reason = "end_turn",
            usage = new
            {
                usage.input_tokens,
                usage.cache_creation_input_tokens,
                usage.cache_read_input_tokens,
                usage.cache_creation,
                output_tokens = outputTokens,
            },
        }, cancel);
        return;
    }

    context.Response.ContentType = "text/event-stream; charset=utf-8";
    context.Response.Headers.CacheControl = "no-cache";

    async Task Send(string type, object data)
    {
        await context.Response.WriteAsync($"event: {type}\ndata: {JsonSerializer.Serialize(data)}\n\n", cancel);
        await context.Response.Body.FlushAsync(cancel);
        await Task.Delay(delayMs, cancel);
    }

    await Send("message_start", new
    {
        type = "message_start",
        message = new
        {
            id = "msg_mock", type = "message", role = "assistant", model,
            content = Array.Empty<object>(), stop_reason = (string?)null, usage,
        },
    });
    await Send("content_block_start", new { type = "content_block_start", index = 0, content_block = new { type = "text", text = "" } });
    await Send("ping", new { type = "ping" });

    for (var i = 0; i < words.Length; i++)
    {
        await Send("content_block_delta", new { type = "content_block_delta", index = 0, delta = new { type = "text_delta", text = words[i] } });
        if (i == words.Length / 2)
        {
            // A longer pause with a keep-alive, like a model thinking mid-reply.
            await Task.Delay(delayMs * 5, cancel);
            await Send("ping", new { type = "ping" });
        }
    }

    await Send("content_block_stop", new { type = "content_block_stop", index = 0 });
    await Send("message_delta", new { type = "message_delta", delta = new { stop_reason = "end_turn" }, usage = new { output_tokens = outputTokens } });
    await Send("message_stop", new { type = "message_stop" });
});

app.Run();
