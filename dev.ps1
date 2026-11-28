<#
.SYNOPSIS
  Local test environment: the mock upstream plus the service in this console.
.PARAMETER Real
  Forward to the real api.anthropic.com instead of the mock. Uses your seat.
.PARAMETER Seed
  First fill the database with three weeks of traffic from four example people (EXAMPLE\maverick …),
  for trying the Dashboard, Radar, Cost and Users screens. Running it again replaces them.
.PARAMETER Https
  Listen on https://localhost:8443 with the .NET development certificate for localhost, which
  Windows (and so Claude Code and the tray app) trusts on this computer. Creates it on first
  use, after Windows asks you to trust it. Writes dev\comptroller.https.json from the config.
#>
param([switch]$Real, [switch]$Seed, [switch]$Https)

$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
$dotnet = (Get-Command dotnet -ErrorAction SilentlyContinue).Source
if (-not $dotnet) { $dotnet = "$env:ProgramFiles\dotnet\dotnet.exe" }

$config = if ($Real) { "$root\dev\comptroller.real.json" } else { "$root\dev\comptroller.json" }
$base = 'http://localhost:8080'

# Another copy (often an installed service from trying Install) would hold the ports.
$busy = Get-NetTCPConnection -LocalPort 8080, 8081, 8443 -State Listen -ErrorAction SilentlyContinue
if ($busy) {
    $owner = Get-Process -Id ($busy[0].OwningProcess) -ErrorAction SilentlyContinue
    $ports = ($busy.LocalPort | Sort-Object -Unique) -join ', '
    $hint = if ((Get-Service claude-comptroller -ErrorAction SilentlyContinue).Status -eq 'Running') {
        'The installed claude-comptroller service is running: stop it in an administrator terminal with "sc.exe stop claude-comptroller".'
    } else { 'Stop it and try again.' }
    throw "Port $ports is already in use by $($owner.ProcessName) (process $($owner.Id)). $hint"
}

if ($Https) {
    # The ASP.NET Core development certificate: CN=localhost, marked with this OID.
    $find = { Get-ChildItem Cert:\CurrentUser\My | Where-Object { $_.Extensions.Oid.Value -contains '1.3.6.1.4.1.311.84.1.1' -and $_.NotAfter -gt (Get-Date) } | Sort-Object NotAfter -Descending | Select-Object -First 1 }
    $cert = & $find
    # Made and trusted are separate: a certificate made earlier without --trust is refused by
    # Windows, so Claude Code, the tray app and the admin terminal's checks all refuse it too.
    $trusted = { param($c) [bool](Get-ChildItem Cert:\CurrentUser\Root | Where-Object Thumbprint -eq $c.Thumbprint) }
    if (-not $cert -or -not (& $trusted $cert)) {
        Write-Host 'Trusting the .NET development certificate for localhost (made if needed); Windows asks you to confirm.'
        & $dotnet dev-certs https --trust
        $cert = & $find
        if (-not $cert -or -not (& $trusted $cert)) { throw 'The development certificate isn''t trusted: run "dotnet dev-certs https --trust", answer Yes, and try again.' }
    }
    $settings = Get-Content $config -Raw | ConvertFrom-Json
    $settings.Proxy | Add-Member -Force ListenUrl 'https://localhost:8443'
    $settings | Add-Member -Force TLS ([pscustomobject]@{ CertificateThumbprint = $cert.Thumbprint; CertificateStore = 'CurrentUser' })
    $config = "$root\dev\comptroller.https.json"
    $settings | ConvertTo-Json -Depth 10 | Set-Content $config -Encoding utf8
    $base = 'https://localhost:8443'
}
$projects = @("$root\service\Comptroller.Service")
if (-not $Real) { $projects += "$root\tools\mock-anthropic" }
if ($Seed) { $projects += "$root\tools\seed-db" }
foreach ($project in $projects) {
    & $dotnet build $project --nologo -v quiet
    if ($LASTEXITCODE -ne 0) { throw "Build failed: $project" }
}

if ($Seed) {
    & $dotnet "$root\tools\seed-db\bin\Debug\net10.0\seed-db.dll" --config $config
    if ($LASTEXITCODE -ne 0) { throw 'Seeding failed' }
}

$mock = $null
try {
    if (-not $Real) {
        $mockDll = "$root\tools\mock-anthropic\bin\Debug\net10.0\mock-anthropic.dll"
        $mock = Start-Process $dotnet -ArgumentList "`"$mockDll`"" -PassThru -NoNewWindow
    }

    Write-Host ''
    Write-Host "Upstream: $(if ($Real) { 'api.anthropic.com (REAL, uses your seat)' } else { 'mock on http://localhost:5055' })"
    Write-Host 'To send Claude Code through it, in a NEW terminal (signs you in with Windows first):'
    Write-Host "  `$s = irm `"$base/_comptroller/connect?device=dev-laptop`" -UseDefaultCredentials -Headers @{`"X-Comptroller-Client`"=`"dev`"}"
    Write-Host "  `$env:ANTHROPIC_BASE_URL = `"$base`""
    Write-Host '  $env:ANTHROPIC_CUSTOM_HEADERS = "X-Comptroller-Token: $($s.token)"'
    Write-Host '  claude'
    Write-Host 'Health: http://127.0.0.1:8081/health'
    Write-Host 'Admin terminal against this console service:'
    Write-Host "  cargo run --manifest-path tui\Cargo.toml -- --config $config"
    Write-Host 'Tray app in sandbox mode (writes settings under dev\sandbox, never your real ones):'
    Write-Host "  cargo run --manifest-path connect\Cargo.toml -- --settings-root $root\dev\sandbox --server $base"
    Write-Host 'Ctrl+C stops everything.'
    Write-Host ''

    & $dotnet "$root\service\Comptroller.Service\bin\Debug\net10.0\claude-comptroller-service.dll" --config $config
}
finally {
    if ($mock -and -not $mock.HasExited) { Stop-Process -Id $mock.Id -Confirm:$false }
}
