using System.Net;
using System.Security.Cryptography;
using Comptroller.Service.Config;
using Comptroller.Service.Identity;
using Microsoft.Extensions.Options;

namespace Comptroller.Service;

/// <summary>
/// The setup page developers open in a browser, and the tray app it links to. The exe comes
/// from the <c>downloads</c> folder beside comptroller.json, where the admin terminal's Install
/// puts it. Neither carries user data, so unlike the other /_comptroller/ endpoints a browser
/// may load them without the X-Comptroller-Client header. The address allow-list still applies.
/// </summary>
public static class Downloads
{
    public const string ConnectExe = "claude-comptroller-connect.exe";

    public static void MapDownloads(this WebApplication app, string configPath)
    {
        var options = app.Services.GetRequiredService<IOptionsMonitor<ComptrollerConfig>>();
        var folder = Path.Combine(Path.GetDirectoryName(configPath)!, "downloads");
        var exe = Path.Combine(folder, ConnectExe);
        var hashes = new FileHashes();

        app.MapGet("/_comptroller/download", (HttpContext context) =>
        {
            var file = new FileInfo(exe);
            var server = $"{context.Request.Scheme}://{context.Request.Host}";
            var html = Page(options.CurrentValue, server, file.Exists ? (file.Length, hashes.Sha256(file)) : null);
            context.Response.Headers.CacheControl = "no-store";
            return Results.Content(html, "text/html; charset=utf-8");
        });

        app.MapGet($"/_comptroller/download/{ConnectExe}", () => File.Exists(exe)
            ? Results.File(exe, "application/octet-stream", ConnectExe)
            : ApiErrors.Result(404, "The tray app isn't on this server yet. Ask your administrator."));
    }

    /// <summary>The SHA-256 of a file, worked out again only when the file changes.</summary>
    private sealed class FileHashes
    {
        private (string Path, DateTime Written, long Length, string Hash)? _last;

        public string Sha256(FileInfo file)
        {
            lock (this)
            {
                if (_last is { } l && l.Path == file.FullName && l.Written == file.LastWriteTimeUtc && l.Length == file.Length) return l.Hash;
                using var stream = file.OpenRead();
                var hash = Convert.ToHexStringLower(SHA256.HashData(stream));
                _last = (file.FullName, file.LastWriteTimeUtc, file.Length, hash);
                return hash;
            }
        }
    }

    private static string Page(ComptrollerConfig config, string server, (long Size, string Sha256)? exe)
    {
        var e = (string s) => WebUtility.HtmlEncode(s);
        var org = config.General?.OrganizationName?.Trim() ?? "";
        var title = org.Length > 0 ? $"Claude Code at {e(org)}" : "Claude Code through claude-comptroller";
        var signIn = config.Identity.Mode == "manual"
            ? "Ask your administrator for a one-time code first. The tray app asks for it, together with the server address."
            : $"It signs you in with your Windows account, without a password prompt. Your account must be in the <b>{e(config.Identity.AllowedGroup)}</b> group; if it isn't, ask your administrator.";
        var download = exe is { } file
            ? $"""
               <p><a class="button" href="/_comptroller/download/{ConnectExe}">Download {ConnectExe}</a></p>
               <p class="small">{file.Size / 1024.0 / 1024.0:0.0} MB · SHA-256 <code>{file.Sha256}</code></p>
               """
            : "<p class=\"warn\">The tray app isn't on this server yet. Ask your administrator: installing the service with the admin terminal puts it here.</p>";

        return $$"""
            <!doctype html>
            <html lang="en">
            <head>
            <meta charset="utf-8">
            <meta name="viewport" content="width=device-width, initial-scale=1">
            <title>{{title}}</title>
            <style>
              :root { color-scheme: light dark; --bg: #f6f6f4; --fg: #1d1d1b; --muted: #5d5d58; --card: #ffffff; --line: #deded8; --accent: #0b6fa4; --warn: #a65400; }
              @media (prefers-color-scheme: dark) { :root { --bg: #161616; --fg: #ececea; --muted: #a2a29c; --card: #202020; --line: #333; --accent: #56c4ee; --warn: #ff9828; } }
              body { margin: 0; background: var(--bg); color: var(--fg); font: 16px/1.55 "Segoe UI", system-ui, sans-serif; }
              main { max-width: 720px; margin: 0 auto; padding: 32px 16px 48px; }
              h1 { font-size: 1.6rem; margin: 0 0 4px; }
              h2 { font-size: 1.1rem; margin: 28px 0 8px; }
              .lead, .small { color: var(--muted); }
              .small { font-size: .875rem; overflow-wrap: anywhere; }
              ol { padding-left: 1.3em; } li { margin: 6px 0; }
              code { font-family: "Cascadia Mono", Consolas, monospace; font-size: .9em; background: var(--card); border: 1px solid var(--line); border-radius: 4px; padding: 1px 5px; }
              .button { display: inline-block; background: var(--accent); color: #fff; text-decoration: none; padding: 10px 18px; border-radius: 6px; font-weight: 600; }
              @media (prefers-color-scheme: dark) { .button { color: #111; } }
              .warn { color: var(--warn); }
              .card { background: var(--card); border: 1px solid var(--line); border-radius: 8px; padding: 4px 18px; }
            </style>
            </head>
            <body>
            <main>
            <h1>{{title}}</h1>
            <p class="lead">Your Claude Code requests go through this company server. You keep your own Claude login and plan; the server never sees your password and never stores your prompts or replies.</p>

            <h2>1. Get the tray app</h2>
            {{download}}
            <p class="small">Windows may warn that the app is from an unknown publisher. Choose <b>More info</b>, then <b>Run anyway</b>, if your administrator says that's expected.</p>

            <h2>2. Connect</h2>
            <div class="card">
            <ol>
              <li>Run <code>{{ConnectExe}}</code>. It sits in the notification area, by the clock.</li>
              <li>When it asks for the server, enter <code>{{e(server)}}</code></li>
              <li>{{signIn}}</li>
              <li>Restart VS Code once, so the Claude Code extension picks up the change.</li>
            </ol>
            </div>

            <h2>What it changes</h2>
            <p>Two settings, in <code>%USERPROFILE%\.claude\settings.json</code> and in VS Code's user settings: <code>ANTHROPIC_BASE_URL</code>, pointing Claude Code at this server, and <code>ANTHROPIC_CUSTOM_HEADERS</code>, with this computer's token. Nothing else in those files changes, and the originals are backed up first.</p>
            <p>It never sets an API key, so you stay on your own Claude plan.</p>

            <h2>Disconnecting</h2>
            <p><b>Disconnect</b> in the tray app stops Claude Code on this computer until you reconnect. To go back to using Claude directly, right-click the tray icon and choose <b>Remove comptroller settings</b>; it puts your original settings back.</p>
            </main>
            </body>
            </html>
            """;
    }
}
