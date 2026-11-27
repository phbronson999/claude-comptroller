using System.Net;
using System.Security.Cryptography;

namespace Comptroller.E2E;

/// <summary>A server with the tray app in its downloads folder.</summary>
public sealed class DownloadHarness : Harness
{
    public static readonly byte[] Exe = "not really an exe, but bytes to serve"u8.ToArray();

    protected override void BeforeStart(string databasePath)
    {
        var folder = Path.Combine(Path.GetDirectoryName(Path.GetDirectoryName(databasePath)!)!, "downloads");
        Directory.CreateDirectory(folder);
        File.WriteAllBytes(Path.Combine(folder, "claude-comptroller-connect.exe"), Exe);
    }
}

public sealed class DownloadTests(DownloadHarness harness) : IClassFixture<DownloadHarness>
{
    [Fact]
    public async Task A_browser_gets_the_setup_page_and_the_tray_app_without_the_client_header()
    {
        // Plain GETs, as a browser sends them: no X-Comptroller-Client.
        var page = await harness.Client.GetAsync("/_comptroller/download");
        Assert.Equal(HttpStatusCode.OK, page.StatusCode);
        Assert.Equal("text/html", page.Content.Headers.ContentType?.MediaType);
        var html = await page.Content.ReadAsStringAsync();
        Assert.Contains("/_comptroller/download/claude-comptroller-connect.exe", html);
        Assert.Contains(Convert.ToHexStringLower(SHA256.HashData(DownloadHarness.Exe)), html);
        Assert.Contains("one-time code", html);
        Assert.Contains(harness.Client.BaseAddress!.Authority, html);

        var exe = await harness.Client.GetAsync("/_comptroller/download/claude-comptroller-connect.exe");
        Assert.Equal(HttpStatusCode.OK, exe.StatusCode);
        Assert.Equal("claude-comptroller-connect.exe", exe.Content.Headers.ContentDisposition?.FileNameStar ?? exe.Content.Headers.ContentDisposition?.FileName?.Trim('"'));
        Assert.Equal(DownloadHarness.Exe, await exe.Content.ReadAsByteArrayAsync());

        // The other own endpoints still want the header.
        Assert.Equal(HttpStatusCode.BadRequest, (await harness.Client.GetAsync("/_comptroller/info")).StatusCode);
    }
}

public sealed class NoDownloadTests(Harness harness) : IClassFixture<Harness>
{
    [Fact]
    public async Task Without_the_tray_app_the_page_says_so_and_the_file_is_not_found()
    {
        var html = await harness.Client.GetStringAsync("/_comptroller/download");
        Assert.Contains("isn't on this server yet", html);
        Assert.Equal(HttpStatusCode.NotFound, (await harness.Client.GetAsync("/_comptroller/download/claude-comptroller-connect.exe")).StatusCode);
    }
}
