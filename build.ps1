<#
.SYNOPSIS
  Builds a release: the service, the tray app, and the admin terminal with both built in,
  then packages them as zips under dist\ with SHA-256 checksums.
.DESCRIPTION
  1. Publishes the service as one self-contained exe  -> publish\service\
  2. Builds the tray app (release)                     -> publish\connect\
  3. Builds the admin terminal (release) with both inside it
  4. Packages dist\claude-comptroller-<version>-win-x64.zip          (for the server)
             dist\claude-comptroller-connect-<version>-win-x64.zip  (for developer computers)
             dist\SHA256SUMS.txt
  Needs the .NET 10 SDK and Rust (stable, MSVC). The tray app also needs WebView2 to run,
  which Windows 10 and 11 include.
.PARAMETER Test
  Run every test suite first, and stop if one fails.
#>
param([switch]$Test)

$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
Set-Location $root

function Write-Step([string]$text) { Write-Host "`n== $text" -ForegroundColor Cyan }
function Invoke-Native {
    # Runs a native command and stops the build if it fails. A plain function, so every
    # argument (-o, -c and the like) is passed on rather than taken as its own.
    # Progress on stderr (cargo's "Compiling …") isn't an error; the exit code says.
    $exe, $rest = $args
    $saved = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try { & $exe @rest } finally { $ErrorActionPreference = $saved }
    if ($LASTEXITCODE -ne 0) { throw "Failed ($LASTEXITCODE): $($args -join ' ')" }
}

# ── one version everywhere ──────────────────────────────────────────────────
Write-Step 'Version'
$versions = [ordered]@{
    'service/Comptroller.Service/Comptroller.Service.csproj' = ([xml](Get-Content service/Comptroller.Service/Comptroller.Service.csproj -Raw)).Project.PropertyGroup.Version | Where-Object { $_ } | Select-Object -First 1
    'tui/Cargo.toml'            = (Select-String -Path tui/Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
    'connect/Cargo.toml'        = (Select-String -Path connect/Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
    'connect/tauri.conf.json'   = (Get-Content connect/tauri.conf.json -Raw | ConvertFrom-Json).version
}
$versions.GetEnumerator() | ForEach-Object { Write-Host ("  {0,-55} {1}" -f $_.Key, $_.Value) }
$version = @($versions.Values | Sort-Object -Unique)
if ($version.Count -ne 1) { throw 'The versions above differ. Set the same version in all four files.' }
$version = $version[0]

if ($Test) {
    Write-Step 'Tests'
    Invoke-Native dotnet test service/Comptroller.slnx
    Invoke-Native cargo test --manifest-path connect/Cargo.toml
    Invoke-Native cargo test --manifest-path tui/Cargo.toml
}

# ── build ───────────────────────────────────────────────────────────────────
$publish = Join-Path $root 'publish'
Write-Step 'Service'
Invoke-Native dotnet publish service/Comptroller.Service -c Release -r win-x64 --self-contained `
    -p:PublishSingleFile=true -p:IncludeNativeLibrariesForSelfExtract=true -o (Join-Path $publish 'service')

Write-Step 'Tray app'
Invoke-Native cargo build --release --manifest-path connect/Cargo.toml --features custom-protocol
New-Item -ItemType Directory -Force (Join-Path $publish 'connect') | Out-Null
Copy-Item connect/target/release/claude-comptroller-connect.exe (Join-Path $publish 'connect') -Force

Write-Step 'Admin terminal, with the service and tray app inside'
$env:COMPTROLLER_SERVICE_EXE = Join-Path $publish 'service\claude-comptroller-service.exe'
$env:COMPTROLLER_CONNECT_EXE = Join-Path $publish 'connect\claude-comptroller-connect.exe'
try {
    Invoke-Native cargo build --release --manifest-path tui/Cargo.toml
} finally {
    Remove-Item Env:COMPTROLLER_SERVICE_EXE, Env:COMPTROLLER_CONNECT_EXE -ErrorAction SilentlyContinue
}

# ── package ─────────────────────────────────────────────────────────────────
Write-Step 'Package'
$dist = Join-Path $root 'dist'
if (Test-Path $dist) { Remove-Item $dist -Recurse -Force }
New-Item -ItemType Directory $dist | Out-Null

function New-ReleaseZip([string]$name, [hashtable]$files) {
    $stage = Join-Path $dist $name
    New-Item -ItemType Directory $stage | Out-Null
    foreach ($target in $files.Keys) { Copy-Item $files[$target] (Join-Path $stage $target) }
    $zip = Join-Path $dist "$name.zip"
    Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip
    Remove-Item $stage -Recurse -Force
    $zip
}

$server = New-ReleaseZip "claude-comptroller-$version-win-x64" @{
    'claude-comptroller.exe'      = 'tui/target/release/claude-comptroller.exe'
    'comptroller.sample.json'     = 'contracts/comptroller.sample.json'
    'pricing.json'                = 'contracts/pricing.json'
    'README.md'                   = 'README.md'
    'LICENSE'                     = 'LICENSE'
}
$connect = New-ReleaseZip "claude-comptroller-connect-$version-win-x64" @{
    'claude-comptroller-connect.exe' = 'connect/target/release/claude-comptroller-connect.exe'
    'LICENSE'                        = 'LICENSE'
}

$sums = foreach ($zip in $server, $connect) {
    '{0}  {1}' -f (Get-FileHash $zip -Algorithm SHA256).Hash.ToLowerInvariant(), (Split-Path $zip -Leaf)
}
# Plain LF and no BOM, so `sha256sum -c` reads it too.
[IO.File]::WriteAllText((Join-Path $dist 'SHA256SUMS.txt'), ($sums -join "`n") + "`n")

Write-Step "Done: version $version"
Get-ChildItem $dist | ForEach-Object { Write-Host ("  {0,-50} {1,8:N1} MB" -f $_.Name, ($_.Length / 1MB)) }
