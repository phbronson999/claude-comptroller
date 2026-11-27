namespace Comptroller.Service.Identity;

/// <summary>Errors in Anthropic's JSON shape, so Claude Code shows the message.</summary>
public static class ApiErrors
{
    public static IResult Result(int status, string message)
    {
        var type = status switch
        {
            400 => "invalid_request_error",
            401 => "authentication_error",
            403 => "permission_error",
            404 => "not_found_error",
            _ => "api_error",
        };
        return Results.Json(new { type = "error", error = new { type, message } }, statusCode: status);
    }

    public static Task Write(HttpContext context, int status, string message) =>
        Result(status, message).ExecuteAsync(context);

    public const string NotConnected = "This computer isn't connected to claude-comptroller. Open claude-comptroller-connect.";

    /// <summary>Why this device may not use the proxy, or null if it may.</summary>
    public static string? Refusal(Device device) => device switch
    {
        { UserRevoked: true } => $"{device.User}'s access to claude-comptroller was revoked. Ask your administrator.",
        { Status: "revoked" } => $"{device.Name} was revoked. Ask your administrator.",
        { Status: "suspended", SuspendedBy: "tray" } => $"Disconnected from the tray on {device.Name}. Click Reconnect.",
        { Status: "suspended" } => $"{device.Name} was suspended by an administrator.",
        { Expired: true } => $"The sign-in for {device.Name} has expired. Open claude-comptroller-connect and refresh.",
        _ => null,
    };
}
