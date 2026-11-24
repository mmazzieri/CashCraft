# Builds the Windows release files:
#   dist\cashcraft-<ver>-windows-x64.msi            installer (cargo-packager + WiX)
#   dist\cashcraft-<ver>-windows-x64-portable.zip   both programs, no installation
#
# Usage: pwsh packaging/windows/package.ps1     (needs: cargo install cargo-packager --locked)
# Signing: with WINDOWS_CERTIFICATE (base64 .pfx) and WINDOWS_CERTIFICATE_PASSWORD set, the
# programs and the installer are signed with signtool; without them they are unsigned.
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..\..')

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches[0].Groups[1].Value
$arch = 'x64'
$name = "cashcraft-$version-windows-$arch"
New-Item -ItemType Directory -Force dist | Out-Null

Write-Host "-> Building CashCraft $version ($arch)"
cargo build --release --bins
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

$pfx = $null
if ($env:WINDOWS_CERTIFICATE) {
    $pfx = Join-Path $env:RUNNER_TEMP 'cashcraft-signing.pfx'
    [IO.File]::WriteAllBytes($pfx, [Convert]::FromBase64String($env:WINDOWS_CERTIFICATE))
}
function Sign-File($path) {
    if ($pfx) {
        signtool sign /f $pfx /p $env:WINDOWS_CERTIFICATE_PASSWORD /fd SHA256 /tr http://timestamp.digicert.com /td SHA256 $path
        if ($LASTEXITCODE -ne 0) { throw "signing $path failed" }
    }
}
Sign-File target\release\cashcraft.exe
Sign-File target\release\cashcraft-cli.exe

Write-Host "-> Creating dist\$name-portable.zip"
$stage = "dist\$name-portable"
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item target\release\cashcraft.exe, target\release\cashcraft-cli.exe, LICENSE-MIT, LICENSE-APACHE, NOTICE, README.md $stage
Compress-Archive -Path "$stage\*" -DestinationPath "dist\$name-portable.zip" -Force
Remove-Item -Recurse -Force $stage

Write-Host "-> Creating dist\$name.msi"
if (Test-Path target\packager) { Remove-Item -Recurse -Force target\packager }
cargo packager --release --formats wix
if ($LASTEXITCODE -ne 0) { throw 'cargo packager failed' }
$msi = Get-ChildItem target\packager -Recurse -Filter *.msi | Select-Object -First 1
Copy-Item $msi.FullName "dist\$name.msi"
Sign-File "dist\$name.msi"
if (-not $pfx) { Write-Warning 'unsigned build (no WINDOWS_CERTIFICATE): Windows SmartScreen may warn' }

Get-ChildItem "dist\$name*"
