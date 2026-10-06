param(
    [string]$Version = "",
    [string]$OutputDirectory = "dist"
)

$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
Set-Location $root

if ([string]::IsNullOrWhiteSpace($Version)) {
    $cargoManifest = Get-Content "Cargo.toml" -Raw
    if ($cargoManifest -notmatch '(?m)^version\s*=\s*"([^"]+)"') {
        throw "Could not determine the package version from Cargo.toml"
    }
    $Version = $Matches[1]
}

$cargoManifest = Get-Content "Cargo.toml" -Raw
if ($cargoManifest -notmatch '(?m)^version\s*=\s*"([^"]+)"' -or $Matches[1] -ne $Version) {
    throw "Release version must match Cargo.toml so update checks report the correct version"
}
if ($Version -notmatch '^\d+\.\d+\.\d+$') {
    throw "MSI releases require a stable major.minor.patch version"
}
$parts = $Version.Split('.')
if ([int]$parts[0] -gt 255 -or [int]$parts[1] -gt 255 -or [int]$parts[2] -gt 65535) {
    throw "Version exceeds Windows Installer version limits"
}

cargo build --locked --release
if ($LASTEXITCODE -ne 0) { throw "Cargo build failed" }

# Inspect the real PE header: GUI executables use subsystem 2, consoles use 3.
$exeBytes = [System.IO.File]::ReadAllBytes((Join-Path $root "target\release\starter.exe"))
$peOffset = [BitConverter]::ToInt32($exeBytes, 0x3c)
$subsystem = [BitConverter]::ToUInt16($exeBytes, $peOffset + 24 + 68)
if ($subsystem -ne 2) { throw "Starter.exe still uses the console subsystem" }

$stage = Join-Path $env:TEMP "starter-$Version-windows-x86_64"
$archive = Join-Path $OutputDirectory "Starter-$Version-windows-x86_64.zip"
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
if (Test-Path $archive) { Remove-Item -Force $archive }
New-Item -ItemType Directory -Force -Path $stage, $OutputDirectory | Out-Null

Copy-Item "target\release\starter.exe" (Join-Path $stage "Starter.exe")
Copy-Item "README.md" (Join-Path $stage "README.md")
Compress-Archive -Path (Join-Path $stage "*") -DestinationPath $archive
Write-Host "Created $archive"

# Install WiX from NuGet into a temporary directory; no global tools are changed.
$wixTools = Join-Path $env:TEMP "starter-wix-6.0.2"
$wix = Join-Path $wixTools "wix.exe"
if (-not (Test-Path $wix)) {
    dotnet tool install wix --version 6.0.2 --tool-path $wixTools
    if ($LASTEXITCODE -ne 0) { throw "WiX installation failed" }
}
$msi = Join-Path $OutputDirectory "Starter-$Version-windows-x86_64.msi"
& $wix build "resources\windows\installer.wxs" -arch x64 `
    -d "Version=$Version" -d "SourceDir=$stage" `
    -d "IconPath=$root\resources\icons\Starter.ico" -o $msi
if ($LASTEXITCODE -ne 0) { throw "MSI build failed" }
Write-Host "Created $msi"
