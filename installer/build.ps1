<#
  Gera o instalador do Kalkan Monitor: target/installer/KalkanMonitor-Setup-<versão>.exe

  Uso (PowerShell 7):  ./installer/build.ps1 [-Version 0.1.0]

  Requer Rust, .NET 8 SDK e Inno Setup 6 (winget install JRSoftware.InnoSetup).
  É o mesmo script que o GitHub Actions roda (.github/workflows/release.yml).
#>
param([string]$Version = "0.0.0")

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
Set-Location $root

# PawnIO — driver de kernel compatível com HVCI usado pelo LibreHardwareMonitor.
# Versão fixa e SHA-256 conferido: o instalador nunca embute um binário diferente.
$PawnIOVersion = '2.2.0'
$PawnIOSha256 = '1f519a22e47187f70a1379a48ca604981c4fcf694f4e65b734aaa74a9fba3032'

cargo build --release -p kalkan-ui
if ($LASTEXITCODE) { throw "cargo build falhou" }

dotnet publish sidecar/kalkan-sensord -c Release -r win-x64 --self-contained
if ($LASTEXITCODE) { throw "dotnet publish falhou" }

$out = Join-Path $root 'target/installer'
New-Item -ItemType Directory -Force $out | Out-Null
$pawn = Join-Path $out 'PawnIO_setup.exe'
if (-not (Test-Path $pawn) -or (Get-FileHash $pawn -Algorithm SHA256).Hash -ne $PawnIOSha256) {
    Invoke-WebRequest "https://github.com/namazso/PawnIO.Setup/releases/download/$PawnIOVersion/PawnIO_setup.exe" -OutFile $pawn
}
$hash = (Get-FileHash $pawn -Algorithm SHA256).Hash
if ($hash -ne $PawnIOSha256) { throw "SHA-256 do PawnIO não confere: $hash" }

$iscc = @(
    (Get-Command iscc -ErrorAction SilentlyContinue)?.Source
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if (-not $iscc) { throw "Inno Setup 6 não encontrado (winget install JRSoftware.InnoSetup)" }

& $iscc "/DAppVersion=$Version" "/DPawnIOSetup=$pawn" installer/kalkan-monitor.iss
if ($LASTEXITCODE) { throw "ISCC falhou" }
