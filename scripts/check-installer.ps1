param([Parameter(Mandatory=$true)][string]$Installer, [string]$ReportDirectory = 'artifacts/stability')
$ErrorActionPreference = 'Stop'
$workspace = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$installerPath = (Resolve-Path -LiteralPath $Installer).Path
$reportPath = [IO.Path]::GetFullPath((Join-Path $workspace "$ReportDirectory/installer-verification.json"))
if (!$reportPath.StartsWith($workspace + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Installer report must stay inside the workspace.' }
$installDir = [IO.Path]::GetFullPath((Join-Path $workspace 'artifacts/installer-fixture'))
if (-not $installerPath.StartsWith($workspace + '\', [StringComparison]::OrdinalIgnoreCase) -or -not $installDir.StartsWith($workspace + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Installer test paths must be inside this workspace.' }
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Core Pulse'
$preferenceKey = 'HKCU:\Software\corepulse\Core Pulse'
if ((Test-Path -LiteralPath $uninstallKey) -or (Test-Path -LiteralPath $preferenceKey)) { throw 'An existing Core Pulse installation or installer preferences must be preserved; use a clean test account.' }
if (Get-Process -Name core-pulse -ErrorAction SilentlyContinue) { throw 'Close Core Pulse before installer verification.' }
if (Test-Path -LiteralPath $installDir) { throw 'The installer fixture directory already exists; inspect it before reusing.' }
$dataDirs = @((Join-Path $env:APPDATA 'com.corepulse.desktop'), (Join-Path $env:LOCALAPPDATA 'com.corepulse.desktop'))
function DataHashes {
    $hashes = @{}
    foreach ($dataDir in $dataDirs) {
        foreach ($name in @('performance.db','performance.db-wal','performance.db-shm')) {
            $path = Join-Path $dataDir $name
            if (Test-Path -LiteralPath $path) { $hashes[$path] = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash }
        }
    }
    return $hashes
}
function AssertDataPreserved($before) {
    $after = DataHashes
    if ($before.Count -ne $after.Count) { throw 'Installer changed the application database files.' }
    foreach ($name in $before.Keys) { if ($before[$name] -ne $after[$name]) { throw 'Installer changed application database content.' } }
}
function AssertSensorResources {
    $source = (Resolve-Path -LiteralPath (Join-Path $workspace 'sidecar/publish')).Path
    $destination = Join-Path $installDir 'sensors'
    $files = @(Get-ChildItem -LiteralPath $source -Recurse -File)
    if ($files.Count -lt 10) { throw 'Prepared sensor bundle is incomplete.' }
    foreach ($file in $files) {
        $relative = $file.FullName.Substring($source.Length + 1)
        $installedFile = Join-Path $destination $relative
        if (!(Test-Path -LiteralPath $installedFile) -or (Get-FileHash -LiteralPath $installedFile -Algorithm SHA256).Hash -ne (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash) { throw "Installed sensor resource differs: $relative" }
    }
    return $files.Count
}
$before = DataHashes
if ($before.Count -eq 0) { throw 'Database preservation verification requires an existing QA database; open Core Pulse once first.' }
$installed = $false
$checks = @()
try {
    $setup = Start-Process -FilePath $installerPath -ArgumentList "/S /NS /D=$installDir" -WindowStyle Hidden -PassThru -Wait
    if ($setup.ExitCode -ne 0) { throw "Installation failed ($($setup.ExitCode))." }
    $installed = $true
    $binary = Join-Path $installDir 'core-pulse.exe'
    if (-not (Test-Path -LiteralPath $binary)) { throw 'Installed executable missing.' }
    $registered = (Get-ItemProperty -LiteralPath $uninstallKey).InstallLocation.Trim('"')
    if ($registered -ne $installDir) { throw 'Unexpected registered installation directory.' }
    AssertDataPreserved $before
    $sensorFilesCompared = AssertSensorResources
    $sensorTest = Start-Process -FilePath (Join-Path $installDir 'sensors/CorePulse.SensorHost.exe') -ArgumentList '--self-test' -WindowStyle Hidden -PassThru -Wait
    if ($sensorTest.ExitCode -ne 0) { throw 'Installed sensor runtime self-test failed.' }
    $checks += @{ freshInstall = $true; databasePreserved = $true; sensorSelfTest = $true; exitCode = $setup.ExitCode }
    $upgrade = Start-Process -FilePath $installerPath -ArgumentList "/S /NS /D=$installDir" -WindowStyle Hidden -PassThru -Wait
    if ($upgrade.ExitCode -ne 0) { throw "Reinstall failed ($($upgrade.ExitCode))." }
    AssertDataPreserved $before
    AssertSensorResources | Out-Null
    $checks += @{ sameVersionReinstall = $true; databasePreserved = $true; exitCode = $upgrade.ExitCode }
} finally {
    if ($installed) {
        # The generated current-user NSIS uninstaller preserves app data unless
        # its explicit deletion checkbox is selected. Silent mode leaves it off.
        $uninstaller = Join-Path $installDir 'uninstall.exe'
        $result = Start-Process -FilePath $uninstaller -ArgumentList "/S _?=$installDir" -WindowStyle Hidden -PassThru -Wait
        if ($result.ExitCode -ne 0) { throw "Uninstall failed ($($result.ExitCode))." }
        if ((Test-Path -LiteralPath (Join-Path $installDir 'core-pulse.exe')) -or (Test-Path -LiteralPath $uninstallKey)) { throw 'Uninstall did not remove application registration/binary.' }
        AssertDataPreserved $before
        if (Test-Path -LiteralPath (Join-Path $installDir 'sensors')) { throw 'Uninstall left sensor resources behind.' }
        $checks += @{ uninstall = $true; databasePreserved = $true; exitCode = $result.ExitCode }
        # Restore absent pre-test installer preferences, only if still ours.
        if (Test-Path -LiteralPath $preferenceKey) {
            $registered = (Get-Item -LiteralPath $preferenceKey).GetValue('')
            if ($registered -ne $installDir) { throw 'Installer preferences changed externally; do not remove them.' }
            Remove-Item -LiteralPath $preferenceKey
        }
        # _?= runs the uninstaller in place; remove only its verified leftover.
        if (Test-Path -LiteralPath $uninstaller) { Remove-Item -LiteralPath $uninstaller }
        if ((Test-Path -LiteralPath $installDir) -and @(Get-ChildItem -LiteralPath $installDir -Force).Count -eq 0) { Remove-Item -LiteralPath $installDir }
    }
}
$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
$report = @{ checks = $checks; installerSha256 = (Get-FileHash -LiteralPath $installerPath -Algorithm SHA256).Hash; databaseFilesCompared = $before.Count; sensorFilesCompared = $sensorFilesCompared; elevatedToken = $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator); scope = 'current-user fresh install, same-version reinstall, uninstall; sensor bundle hashes and existing database byte hashes compared'; crossVersionUpgradeTested = $false }
$report | ConvertTo-Json -Depth 5 | Set-Content -Encoding UTF8 $reportPath
$report | ConvertTo-Json -Depth 5
