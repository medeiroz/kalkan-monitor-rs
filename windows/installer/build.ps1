<#
  Builds the Kalkan Monitor installer: target/installer/KalkanMonitor-Setup-<version>.exe

  Usage (PowerShell 7):  ./windows/installer/build.ps1 [-Version 0.1.0]

  Requires Rust, .NET 8 SDK and Inno Setup 6 (winget install JRSoftware.InnoSetup).
  This is the same script GitHub Actions runs (.github/workflows/release.yml).
#>
param([string]$Version = "0.0.0")

$ErrorActionPreference = 'Stop'
$root = Split-Path (Split-Path $PSScriptRoot)
Set-Location $root

# PawnIO: HVCI-compatible kernel driver used by LibreHardwareMonitor.
# Pinned version and verified SHA-256: the installer never bundles a different binary.
$PawnIOVersion = '2.2.0'
$PawnIOSha256 = '1f519a22e47187f70a1379a48ca604981c4fcf694f4e65b734aaa74a9fba3032'

cargo build --release -p kalkan-ui
if ($LASTEXITCODE) { throw "cargo build failed" }

dotnet publish sidecar/kalkan-sensord -c Release -r win-x64 --self-contained
if ($LASTEXITCODE) { throw "dotnet publish failed" }

$out = Join-Path $root 'target/installer'
New-Item -ItemType Directory -Force $out | Out-Null
$pawn = Join-Path $out 'PawnIO_setup.exe'
if (-not (Test-Path $pawn) -or (Get-FileHash $pawn -Algorithm SHA256).Hash -ne $PawnIOSha256) {
    Invoke-WebRequest "https://github.com/namazso/PawnIO.Setup/releases/download/$PawnIOVersion/PawnIO_setup.exe" -OutFile $pawn
}
$hash = (Get-FileHash $pawn -Algorithm SHA256).Hash
if ($hash -ne $PawnIOSha256) { throw "PawnIO SHA-256 mismatch: $hash" }

$iscc = @(
    (Get-Command iscc -ErrorAction SilentlyContinue)?.Source
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if (-not $iscc) { throw "Inno Setup 6 not found (winget install JRSoftware.InnoSetup)" }

& $iscc "/DAppVersion=$Version" "/DPawnIOSetup=$pawn" windows/installer/kalkan-monitor.iss
if ($LASTEXITCODE) { throw "ISCC failed" }
