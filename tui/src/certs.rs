//! Certificates for the service's https:// listener: the ones in a Windows store that can
//! serve a website, and letting the service's account read the one chosen. Both go through
//! PowerShell's certificate drive, so the Commands panel shows what an admin would type.

use std::process::Command;

use serde::Deserialize;

use crate::activity;

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Certificate {
    pub thumbprint: String,
    pub subject: String,
    pub issuer: String,
    /// `yyyy-mm-dd`, UTC.
    pub not_after: String,
    pub has_private_key: bool,
    /// Usable for a web server: the Server Authentication purpose, or no stated purpose.
    pub server_auth: bool,
    #[serde(default)]
    pub dns_names: Vec<String>,
    /// Issued to itself: fine for a test, but every client would have to trust it by hand.
    pub self_signed: bool,
}

impl Certificate {
    /// Whether the service could use it at all.
    pub fn usable(&self) -> Option<&'static str> {
        if !self.has_private_key {
            Some("no private key")
        } else if !self.server_auth {
            Some("not for web servers")
        } else {
            None
        }
    }
}

fn powershell(script: &str) -> Result<String, String> {
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script])
        .output()
        .map_err(|e| format!("Couldn't run PowerShell: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let error = String::from_utf8_lossy(&output.stderr);
        // The first line of PowerShell's error is the message; the rest is where it happened.
        Err(error.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("PowerShell failed").to_string())
    }
}

fn store_path(store: &str) -> &'static str {
    if store == "CurrentUser" { r"Cert:\CurrentUser\My" } else { r"Cert:\LocalMachine\My" }
}

/// The certificates in a store's personal folder (`LocalMachine` or `CurrentUser`), soonest to
/// expire last.
pub fn list(store: &str) -> Result<Vec<Certificate>, String> {
    let path = store_path(store);
    let script = format!(
        r#"$c = @(Get-ChildItem '{path}' | ForEach-Object {{
  $eku = @($_.EnhancedKeyUsageList | ForEach-Object {{ $_.ObjectId }})
  [pscustomobject]@{{
    Thumbprint = $_.Thumbprint; Subject = $_.GetNameInfo('SimpleName', $false); Issuer = $_.GetNameInfo('SimpleName', $true)
    NotAfter = $_.NotAfter.ToUniversalTime().ToString('yyyy-MM-dd'); HasPrivateKey = $_.HasPrivateKey
    ServerAuth = ($eku.Count -eq 0 -or $eku -contains '1.3.6.1.5.5.7.3.1')
    DnsNames = @($_.DnsNameList | ForEach-Object {{ $_.Unicode }}); SelfSigned = ($_.Subject -eq $_.Issuer)
  }} }})
ConvertTo-Json -InputObject $c -Compress -Depth 3"#
    );
    let json = activity::track(format!("Get-ChildItem {path}"), || powershell(&script), |_| "listed".into())?;
    let mut certs: Vec<Certificate> = if json.is_empty() { Vec::new() } else { serde_json::from_str(&json).map_err(|e| format!("Unexpected certificate list: {e}"))? };
    certs.sort_by(|a, b| b.not_after.cmp(&a.not_after).then_with(|| a.subject.cmp(&b.subject)));
    Ok(certs)
}

/// Lets `account` (such as `NT SERVICE\claude-comptroller`) read the private key of the
/// certificate `thumbprint` in the LocalMachine store, as a service running under that account
/// must. Returns the key file it changed. Needs an administrator terminal.
pub fn grant_key_access(thumbprint: &str, account: &str) -> Result<String, String> {
    let thumbprint: String = thumbprint.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    let account = account.replace('\'', "''");
    // The key lives in a file named by its unique name: under Keys (CNG) or RSA\MachineKeys (older CSP keys).
    let script = format!(
        r#"$c = Get-Item 'Cert:\LocalMachine\My\{thumbprint}' -ErrorAction Stop
if (-not $c.HasPrivateKey) {{ throw 'The certificate has no private key.' }}
$k = [System.Security.Cryptography.X509Certificates.RSACertificateExtensions]::GetRSAPrivateKey($c)
if (-not $k) {{ $k = [System.Security.Cryptography.X509Certificates.ECDsaCertificateExtensions]::GetECDsaPrivateKey($c) }}
if (-not $k) {{ throw 'The private key could not be opened.' }}
$crypto = Join-Path $env:ProgramData 'Microsoft\Crypto'
if ($k.PSObject.Properties['Key']) {{ $name = $k.Key.UniqueName; $dirs = 'Keys', 'SystemKeys' }}
else {{ $name = $k.CspKeyContainerInfo.UniqueKeyContainerName; $dirs = 'RSA\MachineKeys' }}
$file = $dirs | ForEach-Object {{ Join-Path (Join-Path $crypto $_) $name }} | Where-Object {{ Test-Path $_ }} | Select-Object -First 1
if (-not $file) {{ throw "The key file for $name wasn't found under $crypto." }}
$out = icacls.exe $file /grant '{account}:R' 2>&1
if ($LASTEXITCODE -ne 0) {{ throw ($out -join ' ') }}
$file"#
    );
    activity::track(
        format!(r#"icacls.exe "<private key of {thumbprint}>" /grant "{account}:R""#),
        || powershell(&script),
        |file| format!("granted on {file}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_the_current_users_certificates() {
        // Whatever is there, the listing parses; a fresh profile may have none.
        let certs = list("CurrentUser").unwrap();
        for c in &certs {
            assert_eq!(c.thumbprint.len(), 40);
            assert_eq!(c.not_after.len(), 10);
        }
    }
}
