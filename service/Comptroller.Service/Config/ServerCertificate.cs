using System.Security.Cryptography;
using System.Security.Cryptography.X509Certificates;

namespace Comptroller.Service.Config;

/// <summary>
/// The certificate for an https:// listen URL, from the Windows certificate store, so the
/// private key never sits in a file beside the config.
/// </summary>
public static class ServerCertificate
{
    /// <summary>Finds the certificate and checks this process can use its private key.</summary>
    /// <exception cref="InvalidOperationException">Why it can't be used, in words for the log.</exception>
    public static X509Certificate2 Load(TlsConfig tls)
    {
        var thumbprint = TlsConfig.Normalize(tls.CertificateThumbprint);
        var location = tls.CertificateStore == "CurrentUser" ? StoreLocation.CurrentUser : StoreLocation.LocalMachine;
        using var store = new X509Store(StoreName.My, location);
        store.Open(OpenFlags.ReadOnly | OpenFlags.OpenExistingOnly);
        var found = store.Certificates.Find(X509FindType.FindByThumbprint, thumbprint, validOnly: false);
        if (found.Count == 0)
            throw new InvalidOperationException($"No certificate with thumbprint {thumbprint} in the {location} personal store (Cert:\\{location}\\My).");

        var certificate = found[0];
        if (!certificate.HasPrivateKey)
            throw new InvalidOperationException($"The certificate {thumbprint} ({certificate.GetNameInfo(X509NameType.SimpleName, false)}) has no private key in the store. Import it from a .pfx with its key.");

        try
        {
            using var key = (AsymmetricAlgorithm?)certificate.GetRSAPrivateKey() ?? certificate.GetECDsaPrivateKey();
            if (key is null) throw new InvalidOperationException($"The certificate {thumbprint} has neither an RSA nor an ECDSA key.");
        }
        catch (CryptographicException ex)
        {
            throw new InvalidOperationException(
                $"This service's account can't read the private key of certificate {thumbprint} ({ex.Message.Trim()}). " +
                "Choose the certificate again in the admin terminal (Settings → TLS): saving it lets the service's account read the key.", ex);
        }

        if (certificate.NotAfter < DateTime.Now)
            Serilog.Log.Warning("The certificate {Thumbprint} expired on {NotAfter:u}: clients will refuse it", thumbprint, certificate.NotAfter.ToUniversalTime());
        return certificate;
    }

    /// <summary>What /health reports about the certificate: no key material.</summary>
    public static object Describe(X509Certificate2 certificate) => new
    {
        thumbprint = certificate.Thumbprint,
        subject = certificate.GetNameInfo(X509NameType.SimpleName, false),
        issuer = certificate.GetNameInfo(X509NameType.SimpleName, true),
        dnsNames = DnsNames(certificate),
        notAfterUtc = Identity.IdentityStore.Format(certificate.NotAfter.ToUniversalTime()),
    };

    public static IReadOnlyList<string> DnsNames(X509Certificate2 certificate) =>
        certificate.Extensions.OfType<X509SubjectAlternativeNameExtension>().SelectMany(e => e.EnumerateDnsNames()).ToList();
}
