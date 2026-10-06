param([Parameter(Mandatory=$true)][int]$AppProcessId, [string]$ExecutablePath, [switch]$Cancel)
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
$projectRoot=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$process=Get-Process -Id $AppProcessId -ErrorAction Stop
if (!$process.Path.StartsWith((Join-Path $projectRoot 'src-tauri\target\'),[StringComparison]::OrdinalIgnoreCase) -or [IO.Path]::GetFileName($process.Path) -ne 'core-pulse.exe') { throw 'Only the workspace QA application can be controlled.' }
if (!$Cancel) {
    $fixture=[IO.Path]::GetFullPath($ExecutablePath)
    $fixtureRoot=(Join-Path $projectRoot 'artifacts\games\')
    if (!$fixture.StartsWith($fixtureRoot,[StringComparison]::OrdinalIgnoreCase) -or [IO.Path]::GetExtension($fixture) -ne '.exe' -or !(Test-Path -LiteralPath $fixture -PathType Leaf)) { throw 'Only existing executable fixtures under artifacts/games are allowed.' }
}
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$condition=New-Object System.Windows.Automation.AndCondition (
    (New-Object System.Windows.Automation.PropertyCondition ([System.Windows.Automation.AutomationElement]::ProcessIdProperty,$AppProcessId)),
    (New-Object System.Windows.Automation.PropertyCondition ([System.Windows.Automation.AutomationElement]::ClassNameProperty,'#32770'))
)
$condition=New-Object System.Windows.Automation.AndCondition ($condition,(New-Object System.Windows.Automation.PropertyCondition ([System.Windows.Automation.AutomationElement]::ControlTypeProperty,[System.Windows.Automation.ControlType]::Window)))
$dialog=$null
for ($attempt=0;$attempt -lt 60 -and !$dialog;$attempt++) {
    $dialog=[System.Windows.Automation.AutomationElement]::RootElement.FindFirst([System.Windows.Automation.TreeScope]::Children,$condition)
    if (!$dialog) {
        $owner=[System.Windows.Automation.AutomationElement]::RootElement.FindFirst([System.Windows.Automation.TreeScope]::Children,(New-Object System.Windows.Automation.PropertyCondition ([System.Windows.Automation.AutomationElement]::ProcessIdProperty,$AppProcessId)))
        if ($owner) { $dialog=$owner.FindFirst([System.Windows.Automation.TreeScope]::Descendants,$condition) }
    }
    if (!$dialog) { Start-Sleep -Milliseconds 100 }
}
if (!$dialog) { throw 'Owned native file-picker dialog was not found.' }
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class CorePulseFixtureDialog {
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr parent, int id);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr window, System.Text.StringBuilder buffer, int length);
    [DllImport("user32.dll", CharSet=CharSet.Unicode, EntryPoint="SendMessageTimeoutW")] public static extern IntPtr SendPointer(IntPtr window,uint message,IntPtr wParam,IntPtr lParam,uint flags,uint timeout,out IntPtr result);
    [DllImport("user32.dll", CharSet=CharSet.Unicode, EntryPoint="SendMessageTimeoutW")] public static extern IntPtr SendText(IntPtr window,uint message,IntPtr wParam,string lParam,uint flags,uint timeout,out IntPtr result);
}
"@
function Assert-OwnedControl([IntPtr]$Handle,[string]$ExpectedClass) {
    [uint32]$ownerId=0
    $null=[CorePulseFixtureDialog]::GetWindowThreadProcessId($Handle,[ref]$ownerId)
    $class=New-Object Text.StringBuilder 100
    $null=[CorePulseFixtureDialog]::GetClassName($Handle,$class,100)
    if ($Handle -eq [IntPtr]::Zero -or $ownerId -ne $AppProcessId -or $class.ToString() -ne $ExpectedClass) { throw 'Control ownership or native class did not match the fixture dialog.' }
}
$dialogHandle=[IntPtr]$dialog.Current.NativeWindowHandle
Assert-OwnedControl $dialogHandle '#32770'
if (!$Cancel) {
    $filename=[CorePulseFixtureDialog]::GetDlgItem($dialogHandle,1148)
    Assert-OwnedControl $filename 'ComboBoxEx32'
    [IntPtr]$edit=[IntPtr]::Zero
    if ([CorePulseFixtureDialog]::SendPointer($filename,0x0407,[IntPtr]::Zero,[IntPtr]::Zero,2,5000,[ref]$edit) -eq [IntPtr]::Zero) { throw 'Owned file-name control did not respond.' }
    Assert-OwnedControl $edit 'Edit'
    [IntPtr]$accepted=[IntPtr]::Zero
    if ([CorePulseFixtureDialog]::SendText($edit,0x000C,[IntPtr]::Zero,$fixture,2,5000,[ref]$accepted) -eq [IntPtr]::Zero -or $accepted -eq [IntPtr]::Zero) { throw 'Owned file-name control rejected the fixture.' }
}
$buttonId=if ($Cancel) {2} else {1}
$button=[CorePulseFixtureDialog]::GetDlgItem($dialogHandle,$buttonId)
Assert-OwnedControl $button 'Button'
[IntPtr]$result=[IntPtr]::Zero
if ([CorePulseFixtureDialog]::SendPointer($button,0x00F5,[IntPtr]::Zero,[IntPtr]::Zero,2,5000,[ref]$result) -eq [IntPtr]::Zero) { throw 'Owned picker action did not respond.' }
Write-Output 'Owned fixture picker action completed.'
