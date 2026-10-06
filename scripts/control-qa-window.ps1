param(
    [Parameter(Mandatory=$true)][int]$AppProcessId,
    [Parameter(Mandatory=$true)][ValidateSet('Minimize','Restore','Close')][string]$Action
)
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
$projectRoot=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$expected=[IO.Path]::GetFullPath((Join-Path $projectRoot 'src-tauri\target\debug\core-pulse.exe'))
$process=Get-Process -Id $AppProcessId -ErrorAction Stop
if (![string]::Equals($process.Path,$expected,[StringComparison]::OrdinalIgnoreCase)) { throw 'Only the workspace debug QA application can be controlled.' }
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$condition=New-Object System.Windows.Automation.PropertyCondition ([System.Windows.Automation.AutomationElement]::ProcessIdProperty,$AppProcessId)
$windows=[System.Windows.Automation.AutomationElement]::RootElement.FindAll([System.Windows.Automation.TreeScope]::Children,$condition)
$main=@($windows | Where-Object { $_.Current.ClassName -eq 'Tauri Window' -and $_.Current.Name -eq 'Core Pulse' })
if ($main.Count -ne 1) { throw 'Expected exactly one owned Tauri main window.' }
$handle=[IntPtr]$main[0].Current.NativeWindowHandle
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class CorePulseQaWindow {
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr window, int command);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr window);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr window, uint message, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);
}
'@
[uint32]$owner=0
[void][CorePulseQaWindow]::GetWindowThreadProcessId($handle,[ref]$owner)
if ($owner -ne $AppProcessId) { throw 'QA window ownership changed.' }
if ($Action -eq 'Close') {
    if (![CorePulseQaWindow]::PostMessage($handle,0x0010,[IntPtr]::Zero,[IntPtr]::Zero)) { throw 'The owned main window did not accept close.' }
    if (!$process.WaitForExit(15000)) { throw 'The QA application did not complete graceful shutdown.' }
} else {
    $command=if ($Action -eq 'Minimize') {6} else {9}
    [void][CorePulseQaWindow]::ShowWindow($handle,$command)
    if ([CorePulseQaWindow]::IsIconic($handle) -ne ($Action -eq 'Minimize')) { throw 'The QA window state did not change as requested.' }
}
Write-Output "Owned QA window action completed: $Action"
