<#
.SYNOPSIS
  Local test environment: the mock upstream plus the service in this console.
.PARAMETER Real
  Forward to the real api.anthropic.com instead of the mock. Uses your seat.
.PARAMETER Seed
  First fill the database with three weeks of traffic from four example people (EXAMPLE\maverick …),
  for trying the Dashboard, Radar, Cost and Users screens. Running it again replaces them.
#>
param([switch]$Real, [switch]$Seed)

$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
$dotnet = (Get-Command dotnet -ErrorAction SilentlyContinue).Source
if (-not $dotnet) { $dotnet = "$env:ProgramFiles\dotnet\dotnet.exe" }

$config = if ($Real) { "$root\dev\comptroller.real.json" } else { "$root\dev\comptroller.json" }
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
    Write-Host '  $s = irm "http://localhost:8080/_comptroller/connect?device=dev-laptop" -UseDefaultCredentials -Headers @{"X-Comptroller-Client"="dev"}'
    Write-Host '  $env:ANTHROPIC_BASE_URL = "http://localhost:8080"'
    Write-Host '  $env:ANTHROPIC_CUSTOM_HEADERS = "X-Comptroller-Token: $($s.token)"'
    Write-Host '  claude'
    Write-Host 'Health: http://127.0.0.1:8081/health'
    Write-Host 'Admin terminal against this console service:'
    Write-Host "  cargo run --manifest-path tui\Cargo.toml -- --config $config"
    Write-Host 'Tray app in sandbox mode (writes settings under dev\sandbox, never your real ones):'
    Write-Host "  cargo run --manifest-path connect\Cargo.toml -- --settings-root $root\dev\sandbox --server http://localhost:8080"
    Write-Host 'Ctrl+C stops everything.'
    Write-Host ''

    & $dotnet "$root\service\Comptroller.Service\bin\Debug\net10.0\claude-comptroller-service.dll" --config $config
}
finally {
    if ($mock -and -not $mock.HasExited) { Stop-Process -Id $mock.Id -Confirm:$false }
}
