# Build Geo Unlocker for testing and start it (Windows asks for admin rights).
#   powershell -ExecutionPolicy Bypass -File scripts\dev.ps1
# Quit the installed Geo Unlocker first: only one copy can run at a time.
$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)
if (Get-Process geo-unlocker -ErrorAction SilentlyContinue) {
    Write-Host "Geo Unlocker is running. Quit it first (power icon, top right), then run this again." -ForegroundColor Yellow
    exit 1
}
cargo build --profile fast -p geo-unlocker
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
Start-Process "target\fast\geo-unlocker.exe"
Write-Host "Started the test build. Log: C:\ProgramData\GeoUnlocker\geo-unlocker.log"
