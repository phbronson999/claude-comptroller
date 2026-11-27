<#
.SYNOPSIS
  Local test environment: the mock upstream plus the service in this console.
.PARAMETER Real
  Forward to the real api.anthropic.com instead of the mock. Uses your seat.
#>
param([switch]$Real)

$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
$dotnet = (Get-Command dotnet -ErrorAction SilentlyContinue).Source
if (-not $dotnet) { $dotnet = "$env:ProgramFiles\dotnet\dotnet.exe" }

$config = if ($Real) { "$root\dev\comptroller.real.json" } else { "$root\dev\comptroller.json" }
$projects = @("$root\service\Comptroller.Service")
if (-not $Real) { $projects += "$root\tools\mock-anthropic" }
foreach ($project in $projects) {
    & $dotnet build $project --nologo -v quiet
    if ($LASTEXITCODE -ne 0) { throw "Build failed: $project" }
}

$mock = $null
try {
    if (-not $Real) {
        $mockDll = "$root\tools\mock-anthropic\bin\Debug\net10.0\mock-anthropic.dll"
        $mock = Start-Process $dotnet -ArgumentList "`"$mockDll`"" -PassThru -NoNewWindow
    }

    Write-Host ''
    Write-Host "Upstream: $(if ($Real) { 'api.anthropic.com (REAL, uses your seat)' } else { 'mock on http://localhost:5055' })"
    Write-Host 'To send Claude Code through it, in a NEW terminal:'
    Write-Host '  $env:ANTHROPIC_BASE_URL = "http://localhost:8080"'
    Write-Host '  $env:ANTHROPIC_CUSTOM_HEADERS = "X-Comptroller-Token: dev-test"'
    Write-Host '  claude'
    Write-Host 'Ctrl+C stops everything.'
    Write-Host ''

    & $dotnet "$root\service\Comptroller.Service\bin\Debug\net10.0\Comptroller.Service.dll" --config $config
}
finally {
    if ($mock -and -not $mock.HasExited) { Stop-Process -Id $mock.Id -Confirm:$false }
}
