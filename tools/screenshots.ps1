<#
.SYNOPSIS
  Makes the README pictures in docs\images from example data.
.DESCRIPTION
  In C:\comptroller-example (removed afterwards): writes a config, seeds three weeks of
  traffic from the example people (EXAMPLE\maverick, goose, iceman and viper, all on an
  example seat plan), starts the service in the background on the default ports, then draws
  each admin terminal screen to a PNG. The Preflight picture sends one request without
  credentials to api.anthropic.com, as the preflight test does. Nothing of yours is used, and
  a picture that would show this computer's user or computer name is not written. Stop
  dev.ps1 first, and run it again after changing a screen.
#>
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
# Outside your profile, so no picture shows your user name in a path.
$work = Join-Path $env:SystemDrive 'comptroller-example'
$config = Join-Path $work 'comptroller.json'
# The defaults, so the pictures look like a real install.
$adminPort = 8081
try {
    Invoke-RestMethod "http://127.0.0.1:$adminPort/health" -TimeoutSec 2 | Out-Null
    throw "A service already answers on port $adminPort (dev.ps1?). Stop it first."
} catch [System.Net.WebException] { }

function Invoke-Native {
    $exe, $rest = $args
    $saved = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try { & $exe @rest } finally { $ErrorActionPreference = $saved }
    if ($LASTEXITCODE -ne 0) { throw "Failed ($LASTEXITCODE): $($args -join ' ')" }
}

if (Test-Path $work) { Remove-Item $work -Recurse -Force }
New-Item -ItemType Directory $work | Out-Null

# Renews in a few months, so the seat year is partly used.
$renewal = (Get-Date).AddMonths(4).ToString('yyyy-MM-dd')
@"
{
  "General": { "OrganizationName": "Example Co" },
  "Proxy": { "ListenUrl": "http://localhost:8080", "AdminPort": $adminPort },
  "Access": { "AllowedCidrs": [ "127.0.0.1/32", "::1/128", "10.0.0.0/8" ] },
  "Identity": { "Mode": "windows", "AllowedGroup": "Claude Code Users" },
  "Data": { "Folder": "data", "RetentionDays": 21 },
  "Logging": { "Folder": "logs" },
  "Cost": { "SeatPlans": [ { "Name": "Team standard", "Billing": "annual", "Price": 300, "RenewalDate": "$renewal" } ] }
}
"@ | Set-Content $config -Encoding ascii

Write-Host 'Building the service and the seed tool'
Invoke-Native dotnet build "$root\service\Comptroller.slnx" -v quiet -nologo
Write-Host 'Seeding example traffic'
Invoke-Native dotnet run --no-build --project "$root\tools\seed-db" '--' --config $config --days 21 --seat-plan 'Team standard'

$service = Start-Process dotnet -ArgumentList "`"$root\service\Comptroller.Service\bin\Debug\net10.0\claude-comptroller-service.dll`" --config `"$config`"" -WindowStyle Hidden -PassThru
try {
    for ($i = 0; $i -lt 60; $i++) {
        try { Invoke-RestMethod "http://127.0.0.1:$adminPort/health" | Out-Null; break } catch { Start-Sleep -Milliseconds 500 }
    }
    Write-Host 'Drawing the screens'
    $env:COMPTROLLER_SHOTS_CONFIG = $config
    $env:COMPTROLLER_SHOTS_OUT = Join-Path $root 'docs\images'
    Invoke-Native cargo test --manifest-path "$root\tui\Cargo.toml" --bin claude-comptroller '--' --ignored screenshots
} finally {
    Remove-Item Env:COMPTROLLER_SHOTS_CONFIG, Env:COMPTROLLER_SHOTS_OUT -ErrorAction SilentlyContinue
    Stop-Process -Id $service.Id -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 500
    Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue
}
Get-ChildItem (Join-Path $root 'docs\images') -Filter *.png | ForEach-Object { Write-Host "  $($_.Name)" }
