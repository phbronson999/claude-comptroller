<#
.SYNOPSIS
  Installs the published service as a temporary Windows Service, checks it, then removes it.
  Uses the mock upstream, so no seat is used. The admin TUI will take over installing later.
.PARAMETER Console
  Run the same checks against the published exe as a plain process. Needs no admin rights,
  but can't check stopping with a request in flight.
.PARAMETER Keep
  Leave the service installed and running afterwards.
.PARAMETER Remove
  Only remove a service left behind by -Keep.
#>
param([switch]$Console, [switch]$Keep, [switch]$Remove)

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
$name = 'claude-comptroller-test'
$account = "NT SERVICE\$name"
$install = if ($Console) { Join-Path $env:TEMP $name } else { Join-Path $env:ProgramFiles $name }
$exe = Join-Path $install 'claude-comptroller-service.exe'
$base = 'http://localhost:8080'
$headers = @{ 'X-Comptroller-Client' = 'service-test' }

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole('Administrators')
if (-not $Console -and -not $isAdmin) { throw 'Run this in a terminal opened with "Run as administrator" (or use -Console).' }

function Invoke-Native([string]$What, [scriptblock]$Command) {
    $output = & $Command
    if ($LASTEXITCODE -ne 0) { throw "$What failed ($LASTEXITCODE): $output" }
}

function Remove-TestService {
    if (Get-Service $name -ErrorAction SilentlyContinue) {
        Stop-Service $name -Force -ErrorAction SilentlyContinue
        Invoke-Native 'sc delete' { sc.exe delete $name }
        Start-Sleep -Seconds 2
    }
    if (Test-Path $install) { Remove-Item $install -Recurse -Force }
}

if ($Remove) { Remove-TestService; 'Removed.'; return }

$script:failed = 0
function Test-That([string]$What, [scriptblock]$Check) {
    try {
        $detail = & $Check
        Write-Host "PASS  $What  $detail" -ForegroundColor Green
    }
    catch {
        $script:failed++
        Write-Host "FAIL  $What  $($_.Exception.Message)" -ForegroundColor Red
    }
}

$busy = Get-NetTCPConnection -State Listen -LocalPort 8080, 8081, 5055 -ErrorAction SilentlyContinue
if ($busy) { throw "Ports 8080, 8081 and 5055 must be free. Stop dev.ps1 first. In use: $($busy.LocalPort -join ', ')" }

$dotnet = (Get-Command dotnet -ErrorAction SilentlyContinue).Source
if (-not $dotnet) { $dotnet = "$env:ProgramFiles\dotnet\dotnet.exe" }

Write-Host 'Building...'
& $dotnet publish "$root\service\Comptroller.Service" -c Release -r win-x64 --self-contained -p:PublishSingleFile=true -p:IncludeNativeLibrariesForSelfExtract=true -o "$root\publish\service" --nologo -v quiet
if ($LASTEXITCODE -ne 0) { throw 'Publish failed.' }
& $dotnet build "$root\tools\mock-anthropic" --nologo -v quiet
if ($LASTEXITCODE -ne 0) { throw 'Mock build failed.' }

$mock = $null
$process = $null
try {
    Remove-TestService
    New-Item -ItemType Directory -Force $install, "$install\data", "$install\logs" | Out-Null
    Copy-Item "$root\publish\service\claude-comptroller-service.exe" $exe
    Copy-Item "$root\dev\comptroller.json" "$install\comptroller.json"

    # A slow stream (about 7 s) so the service can be stopped in the middle of it.
    $mock = Start-Process $dotnet -ArgumentList "`"$root\tools\mock-anthropic\bin\Debug\net10.0\mock-anthropic.dll`"", '--delay-ms', '350' -PassThru -WindowStyle Hidden

    if ($Console) {
        $process = Start-Process $exe -PassThru -WindowStyle Hidden
    }
    else {
        # A virtual account: no password, few rights, and the computer's identity on the network.
        # New-Service, because Windows PowerShell mangles the quoted path on the way to sc.exe.
        New-Service -Name $name -BinaryPathName "`"$exe`" --service" -StartupType Manual | Out-Null
        Invoke-Native 'sc config' { sc.exe config $name obj= $account }
        Invoke-Native 'icacls logs' { icacls.exe "$install\logs" /grant "${account}:(OI)(CI)M" }
        # The database holds users and token hashes: administrators and the service only.
        Invoke-Native 'icacls data' { icacls.exe "$install\data" /inheritance:r /grant '*S-1-5-32-544:(OI)(CI)F' '*S-1-5-18:(OI)(CI)F' "${account}:(OI)(CI)M" }
        Start-Service $name
    }

    $health = $null
    foreach ($i in 1..30) {
        try { $health = Invoke-RestMethod 'http://127.0.0.1:8081/health' -TimeoutSec 2; break } catch { Start-Sleep 1 }
    }

    Test-That 'starts and answers /health' {
        if (-not $health) { throw 'No answer.' }
        if ($health.windowsService -eq [bool]$Console) { throw "windowsService is $($health.windowsService)" }
        "version $($health.version), config $($health.config.status), service $($health.windowsService)"
    }

    $session = $null
    Test-That 'Windows sign-in and group check' {
        $script:session = Invoke-RestMethod "$base/_comptroller/connect?device=service-test" -UseDefaultCredentials -Headers $headers
        "signed in as $($script:session.user)"
    }

    Test-That 'a request goes through with the token' {
        $models = Invoke-RestMethod "$base/v1/models" -Headers @{ 'X-Comptroller-Token' = $script:session.token }
        "got $($models.data.Count) model"
    }

    if (-not $Console) {
        Test-That 'stopping lets a reply in flight finish' {
            $body = "$install\body.json"
            $reply = "$install\reply.txt"
            Set-Content $body '{"model":"claude-mock-1","stream":true}' -Encoding ascii
            $curl = Start-Process curl.exe -ArgumentList '-sN', "$base/v1/messages", '-H', 'content-type:application/json', '-H', "X-Comptroller-Token:$($script:session.token)", '--data-binary', "`"@$body`"", '-o', "`"$reply`"" -PassThru -WindowStyle Hidden
            Start-Sleep -Seconds 1
            if ($curl.HasExited) { throw 'The reply ended before the stop.' }
            $clock = [Diagnostics.Stopwatch]::StartNew()
            Stop-Service $name
            (Get-Service $name).WaitForStatus('Stopped', '00:01:30')
            $curl.WaitForExit(30000) | Out-Null
            if (-not (Select-String -Path $reply -Pattern 'message_stop' -Quiet)) { throw 'The reply was cut off.' }
            "stop took $([int]$clock.Elapsed.TotalSeconds) s, reply complete"
        }

        Test-That 'starts again with the same database' {
            Start-Service $name
            foreach ($i in 1..30) {
                try { Invoke-RestMethod "$base/v1/models" -Headers @{ 'X-Comptroller-Token' = $script:session.token } | Out-Null; return 'old token still works' } catch { Start-Sleep 1 }
            }
            throw 'The token no longer works.'
        }
    }
}
finally {
    if ($script:failed -gt 0 -or -not $health) {
        Write-Host "`n--- last log lines" -ForegroundColor Yellow
        Get-ChildItem "$install\logs" -ErrorAction SilentlyContinue | Select-Object -Last 1 | Get-Content -Tail 15
        Write-Host '--- Application event log' -ForegroundColor Yellow
        Get-WinEvent -FilterHashtable @{ LogName = 'Application'; ProviderName = '.NET Runtime', $name; StartTime = (Get-Date).AddMinutes(-3) } -ErrorAction SilentlyContinue |
            Select-Object -First 3 -ExpandProperty Message
    }

    if ($process -and -not $process.HasExited) { Stop-Process -Id $process.Id -Confirm:$false; $process.WaitForExit() }
    if (-not $Keep -or $Console) { Remove-TestService }
    if ($mock -and -not $mock.HasExited) { Stop-Process -Id $mock.Id -Confirm:$false }
}

if ($script:failed -gt 0) { Write-Host "`n$script:failed check(s) failed." -ForegroundColor Red; exit 1 }
Write-Host "`nAll checks passed." -ForegroundColor Green
if ($Keep -and -not $Console) { "Still installed in $install. Remove with: .\dev\service-test.ps1 -Remove" }
