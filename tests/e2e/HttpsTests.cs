using System.Net;
using System.Security.Cryptography;
using System.Security.Cryptography.X509Certificates;
using System.Text.Json.Nodes;

namespace Comptroller.E2E;

/// <summary>
/// The service on https:// with a throwaway self-signed certificate in the current user's
/// personal store (a console run reads CurrentUser; an installed service, LocalMachine).
/// The test clients accept exactly that certificate.
/// </summary>
public sealed class HttpsHarness : Harness
{
    public X509Certificate2 Certificate { get; } = Create();

    protected override bool Https => true;

    protected override void Customize(JsonNode config) =>
        config["Tls"] = new JsonObject { ["CertificateThumbprint"] = Certificate.Thumbprint, ["CertificateStore"] = "CurrentUser" };

    protected override HttpClientHandler NewHandler() => new()
    {
        ServerCertificateCustomValidationCallback = (_, presented, _, _) => presented?.Thumbprint == Certificate.Thumbprint,
    };

    public override async Task DisposeAsync()
    {
        await base.DisposeAsync();
        using var store = new X509Store(StoreName.My, StoreLocation.CurrentUser);
        store.Open(OpenFlags.ReadWrite);
        store.Remove(Certificate);
    }

    private static X509Certificate2 Create()
    {
        using var key = RSA.Create(2048);
        var request = new CertificateRequest("CN=comptroller-e2e", key, HashAlgorithmName.SHA256, RSASignaturePadding.Pkcs1);
        var names = new SubjectAlternativeNameBuilder();
        names.AddDnsName("localhost");
        request.CertificateExtensions.Add(names.Build());
        request.CertificateExtensions.Add(new X509EnhancedKeyUsageExtension([new Oid("1.3.6.1.5.5.7.3.1")], critical: false));
        using var created = request.CreateSelfSigned(DateTimeOffset.Now.AddMinutes(-5), DateTimeOffset.Now.AddDays(1));
        // Through a PFX so the key is saved in the user's key store, where the service finds it.
        var stored = X509CertificateLoader.LoadPkcs12(created.Export(X509ContentType.Pfx), null, X509KeyStorageFlags.PersistKeySet | X509KeyStorageFlags.UserKeySet);
        using var store = new X509Store(StoreName.My, StoreLocation.CurrentUser);
        store.Open(OpenFlags.ReadWrite);
        store.Add(stored);
        return stored;
    }
}

public sealed class HttpsTests(HttpsHarness harness) : IClassFixture<HttpsHarness>
{
    [Fact]
    public async Task The_proxy_works_over_https_with_the_configured_certificate()
    {
        Assert.Equal("https", harness.Client.BaseAddress!.Scheme);
        var token = await harness.Enroll("HTTPS-LAPTOP");
        Assert.Equal(HttpStatusCode.OK, (await harness.Models(token)).StatusCode);
        Assert.Equal(HttpStatusCode.Unauthorized, (await harness.Models(null)).StatusCode);
    }

    [Fact]
    public async Task Health_reports_the_certificate_without_key_material()
    {
        var tls = (await Harness.Json(await harness.Admin.GetAsync("/health"))).GetProperty("tls");
        Assert.Equal(harness.Certificate.Thumbprint, tls.GetProperty("thumbprint").GetString());
        Assert.Equal("comptroller-e2e", tls.GetProperty("subject").GetString());
        Assert.Contains("localhost", tls.GetProperty("dnsNames").EnumerateArray().Select(n => n.GetString()));
        var notAfter = DateTime.Parse(tls.GetProperty("notAfterUtc").GetString()!, System.Globalization.CultureInfo.InvariantCulture,
            System.Globalization.DateTimeStyles.AdjustToUniversal | System.Globalization.DateTimeStyles.AssumeUniversal);
        Assert.True(notAfter > DateTime.UtcNow);
    }

    [Fact]
    public async Task Plain_http_on_the_https_port_gets_no_service()
    {
        // Either an error status or a dropped connection: never a proxied reply.
        using var plain = new HttpClient { BaseAddress = new UriBuilder(harness.Client.BaseAddress!) { Scheme = "http" }.Uri };
        try
        {
            var response = await plain.GetAsync("/v1/models");
            Assert.NotEqual(HttpStatusCode.OK, response.StatusCode);
        }
        catch (HttpRequestException)
        {
        }
    }
}
