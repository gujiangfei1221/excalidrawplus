[CmdletBinding()]
param(
  [switch]$Help,
  [switch]$NoProxy,
  [string]$HttpProxy = "http://127.0.0.1:10809",
  [string]$SocksProxy = "socks5://127.0.0.1:10808"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Write-Step {
  param([string]$Message)
  Write-Host ""
  Write-Host "==> $Message" -ForegroundColor Cyan
}

function Write-OutputFile {
  param([string]$Path)

  if (Test-Path -LiteralPath $Path) {
    $item = Get-Item -LiteralPath $Path
    $sizeMb = [Math]::Round($item.Length / 1MB, 2)
    Write-Host ("  {0} ({1} MB, {2})" -f $item.FullName, $sizeMb, $item.LastWriteTime)
  }
}

if ($Help) {
  @"
Build the Excalidraw Tauri desktop app for Windows.

Usage:
  build-desktop.bat
  build-desktop.bat -NoProxy
  build-desktop.bat -HttpProxy http://127.0.0.1:10809 -SocksProxy socks5://127.0.0.1:10808

Outputs:
  src-tauri\target\release\excalidraw-cloud-sync.exe
  src-tauri\target\release\bundle\nsis\Excalidraw_0.1.0_x64-setup.exe
  src-tauri\target\release\bundle\msi\Excalidraw_0.1.0_x64_en-US.msi
"@
  exit 0
}

$repoRoot = Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")
$tauriDir = Join-Path $repoRoot "src-tauri"
$cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"

if (-not (Test-Path -LiteralPath $tauriDir)) {
  throw "Cannot find src-tauri directory: $tauriDir"
}

if (Test-Path -LiteralPath $cargoBin) {
  $env:Path = "$cargoBin;$env:Path"
}

if (-not $NoProxy) {
  $env:HTTP_PROXY = $HttpProxy
  $env:HTTPS_PROXY = $HttpProxy
  $env:ALL_PROXY = $SocksProxy
  Write-Step "Proxy enabled"
  Write-Host "  HTTP_PROXY=$env:HTTP_PROXY"
  Write-Host "  HTTPS_PROXY=$env:HTTPS_PROXY"
  Write-Host "  ALL_PROXY=$env:ALL_PROXY"
} else {
  Write-Step "Proxy disabled"
  Remove-Item Env:\HTTP_PROXY -ErrorAction SilentlyContinue
  Remove-Item Env:\HTTPS_PROXY -ErrorAction SilentlyContinue
  Remove-Item Env:\ALL_PROXY -ErrorAction SilentlyContinue
}

Write-Step "Checking toolchain"
$cargoVersion = (& cargo --version) -join "`n"
$rustcVersion = (& rustc --version) -join "`n"
Write-Host "  $cargoVersion"
Write-Host "  $rustcVersion"

try {
  $tauriVersion = (& cargo tauri --version) -join "`n"
  Write-Host "  $tauriVersion"
} catch {
  Write-Host "  Tauri CLI not found. Installing tauri-cli v2..." -ForegroundColor Yellow
  & cargo install tauri-cli --version "^2" --locked
}

Write-Step "Building desktop package"
Push-Location $tauriDir
try {
  & cargo tauri build
  if ($LASTEXITCODE -ne 0) {
    throw "cargo tauri build failed with exit code $LASTEXITCODE"
  }
} finally {
  Pop-Location
}

Write-Step "Build outputs"
Write-OutputFile (Join-Path $tauriDir "target\release\excalidraw-cloud-sync.exe")
Write-OutputFile (Join-Path $tauriDir "target\release\bundle\nsis\Excalidraw_0.1.0_x64-setup.exe")
Write-OutputFile (Join-Path $tauriDir "target\release\bundle\msi\Excalidraw_0.1.0_x64_en-US.msi")

Write-Host ""
Write-Host "Done." -ForegroundColor Green
