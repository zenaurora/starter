param(
    [string]$Version = "0.1.0",
    [string]$OutputDirectory = "dist"
)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
Set-Location $root

if ([string]::IsNullOrWhiteSpace($Version)) {
    $cargoManifest = Get-Content "Cargo.toml" -Raw
    if ($cargoManifest -notmatch '(?m)^version\s*=\s*"([^"]+)"') {
        throw "Could not determine the package version from Cargo.toml"
    }
    $Version = $Matches[1]
}

cargo build --locked --release

$stage = Join-Path $env:TEMP "starter-$Version-windows-x86_64"
$archive = Join-Path $OutputDirectory "Starter-$Version-windows-x86_64.zip"
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
if (Test-Path $archive) { Remove-Item -Force $archive }
New-Item -ItemType Directory -Force -Path $stage, $OutputDirectory | Out-Null

Copy-Item "target\release\starter.exe" (Join-Path $stage "Starter.exe")
Copy-Item "README.md" (Join-Path $stage "README.md")
Compress-Archive -Path (Join-Path $stage "*") -DestinationPath $archive
Write-Host "Created $archive"
