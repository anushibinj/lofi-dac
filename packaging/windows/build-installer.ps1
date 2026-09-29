# Builds the release binaries and the Windows installer.
#   powershell -ExecutionPolicy Bypass -File packaging\windows\build-installer.ps1
# Requires Rust (stable) and Inno Setup 6 (iscc.exe on PATH or in the default install folder).
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..\..")

$env:RUSTFLAGS = "-C target-feature=+crt-static"
cargo build --release -p lofify-app -p lofify-cli
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$version = (cargo pkgid -p lofify-app) -replace '.*[#@]', ''
$iscc = (Get-Command iscc -ErrorAction SilentlyContinue).Source
if (-not $iscc) { $iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe" }
if (-not (Test-Path $iscc)) { throw "Inno Setup 6 not found. Install it from https://jrsoftware.org/isinfo.php" }

& $iscc "/DAppVersion=$version" "packaging\windows\lofify.iss"
if ($LASTEXITCODE -ne 0) { throw "iscc failed" }
Write-Host "Installer written to dist\Lofify-$version-setup.exe"
