# Intended for a disposable Windows CI runner, where Starter is not installed.
param([Parameter(Mandatory = $true)][string]$MsiPath)
$ErrorActionPreference = "Stop"
$msi = (Resolve-Path $MsiPath).Path
$installDirectory = Join-Path $env:LOCALAPPDATA "Programs\Starter"
$shortcut = Join-Path ([Environment]::GetFolderPath("Programs")) "Starter\Starter.lnk"
if (Test-Path $installDirectory) { throw "Smoke test requires a clean runner" }

function Invoke-Installer([string]$Action, [string]$LogName) {
    $log = Join-Path $env:TEMP $LogName
    $process = Start-Process msiexec.exe -Wait -PassThru -ArgumentList `
        "$Action `"$msi`" /qn /norestart /L*v `"$log`""
    if ($process.ExitCode -notin @(0, 3010)) {
        Get-Content $log -Tail 60 | Write-Host
        throw "Windows Installer failed with exit code $($process.ExitCode)"
    }
}

try {
    Invoke-Installer "/i" "starter-msi-install.log"
    if (-not (Test-Path (Join-Path $installDirectory "Starter.exe"))) {
        throw "MSI did not install Starter.exe"
    }
    if (-not (Test-Path $shortcut)) { throw "MSI did not create the Start menu shortcut" }
} finally {
    Invoke-Installer "/x" "starter-msi-uninstall.log"
}
if (Test-Path (Join-Path $installDirectory "Starter.exe")) { throw "Uninstall left the executable" }
if (Test-Path $shortcut) { throw "Uninstall left the Start menu shortcut" }
Write-Host "MSI installation, Start menu shortcut, and uninstallation verified"
