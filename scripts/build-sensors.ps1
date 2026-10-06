$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
Set-Location -LiteralPath $projectRoot
$localSdk = Join-Path $projectRoot '.tools/dotnet/dotnet.exe'
$dotnet = if (Test-Path -LiteralPath $localSdk) { $localSdk } else { (Get-Command dotnet -ErrorAction Stop).Source }
$env:DOTNET_ROOT = Split-Path -Parent $dotnet
$env:DOTNET_CLI_HOME = Join-Path $projectRoot '.tools/dotnet-home'
$env:NUGET_PACKAGES = Join-Path $projectRoot '.tools/nuget'
$env:DOTNET_CLI_TELEMETRY_OPTOUT = '1'
$env:DOTNET_GENERATE_ASPNET_CERTIFICATE = 'false'
& $dotnet --version
if ($LASTEXITCODE -ne 0) { throw 'Install the exact SDK specified in global.json.' }
& $dotnet restore sidecar/CorePulse.SensorHost.csproj --locked-mode
if ($LASTEXITCODE -ne 0) { throw 'Locked sensor dependency restore failed.' }
$publish = [IO.Path]::GetFullPath((Join-Path $projectRoot 'sidecar/publish'))
if ($publish -ne (Join-Path $projectRoot 'sidecar\publish')) { throw 'Unexpected sensor publish directory.' }
if (Test-Path -LiteralPath $publish) { Remove-Item -LiteralPath $publish -Recurse -Force }
& $dotnet publish sidecar/CorePulse.SensorHost.csproj -c Release -o $publish --no-restore
if ($LASTEXITCODE -ne 0) { throw 'Sensor host publish failed.' }
& (Join-Path $publish 'CorePulse.SensorHost.exe') --self-test
if ($LASTEXITCODE -ne 0) { throw 'Sensor host protocol self-test failed.' }

$sources = Get-Content -LiteralPath sidecar/source-manifest.json -Raw | ConvertFrom-Json
$sourceCache = Join-Path $projectRoot 'sidecar/third-party/source'
New-Item -ItemType Directory -Path $sourceCache -Force | Out-Null
foreach ($entry in $sources) {
    if ([IO.Path]::GetFileName($entry.Name) -ne $entry.Name) { throw 'Invalid source archive name.' }
    $target = Join-Path $sourceCache $entry.Name
    if (!(Test-Path -LiteralPath $target)) { Invoke-WebRequest -UseBasicParsing -Uri $entry.Url -OutFile $target -TimeoutSec 60 }
    if ((Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash -ne $entry.Sha256) { throw "Source hash mismatch: $($entry.Name)" }
}
$notices = Join-Path $publish 'third-party'
New-Item -ItemType Directory -Path $notices -Force | Out-Null
Copy-Item -LiteralPath sidecar/third-party/source -Destination $notices -Recurse
Get-ChildItem -LiteralPath sidecar/third-party -File | ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $notices }
Copy-Item -LiteralPath sidecar/source-manifest.json -Destination $notices

# Preserve package notices for the bundled DLLs, and the self-contained runtime.
$packages = (Get-Content -LiteralPath sidecar/packages.lock.json -Raw | ConvertFrom-Json).dependencies.'net10.0-windows7.0'
foreach ($package in $packages.PSObject.Properties) {
    if (!(Test-Path -LiteralPath (Join-Path $publish "$($package.Name).dll"))) { continue }
    $folder = Join-Path $env:NUGET_PACKAGES "$($package.Name.ToLowerInvariant())/$($package.Value.resolved)"
    $destination = Join-Path $notices "$($package.Name)-$($package.Value.resolved)"
    New-Item -ItemType Directory -Path $destination -Force | Out-Null
    Get-ChildItem -LiteralPath $folder -File | Where-Object { $_.Extension -eq '.nuspec' -or $_.Name -match '(?i)license|notice|copying' } | ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $destination }
}
$runtimeFolder = Get-ChildItem -LiteralPath (Join-Path $env:NUGET_PACKAGES 'microsoft.netcore.app.runtime.win-x64') -Directory | Sort-Object { [version]$_.Name } -Descending | Select-Object -First 1
foreach ($name in @('LICENSE.TXT', 'THIRD-PARTY-NOTICES.TXT')) { Copy-Item -LiteralPath (Join-Path $runtimeFolder.FullName $name) -Destination (Join-Path $notices "dotnet-$name") }
$hostSource = Join-Path $notices 'CorePulse.SensorHost-source'
New-Item -ItemType Directory -Path $hostSource -Force | Out-Null
foreach ($file in @('Program.cs','CorePulse.SensorHost.csproj','packages.lock.json')) { Copy-Item -LiteralPath (Join-Path $projectRoot "sidecar/$file") -Destination $hostSource }
Copy-Item -LiteralPath global.json -Destination $hostSource
Write-Output 'Sensor host, runtime, notices, and corresponding dependency sources prepared.'
