param([Parameter(Mandatory=$true)][int]$AppPid)
$ErrorActionPreference = 'Stop'
$workspace = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$appPath = Join-Path $workspace 'src-tauri\target\release\core-pulse.exe'
$sensorPath = Join-Path $workspace 'src-tauri\target\release\sensors\CorePulse.SensorHost.exe'
$app = Get-CimInstance Win32_Process -Filter "ProcessId=$AppPid"
if (!$app -or $app.ExecutablePath -ne $appPath) { throw 'Expected QA application process was not found.' }
$children = @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$AppPid AND Name='CorePulse.SensorHost.exe'")
if ($children.Count -ne 1 -or $children[0].ExecutablePath -ne $sensorPath) { throw 'Expected bundled QA sensor child was not found.' }
$sensorPid = $children[0].ProcessId
Stop-Process -Id $sensorPid -Force
@{ appPid=$AppPid; stoppedSensorPid=$sensorPid; verifiedPath=$sensorPath } | ConvertTo-Json
