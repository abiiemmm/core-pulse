param(
    [Parameter(Mandatory=$true)][int]$AppProcessId,
    [Parameter(Mandatory=$true)][ValidateSet('HideOnClose','Open','InspectMenu','SelectOpen','SelectQuit')][string]$Action,
    [ValidateSet('id','en','es')][string]$Language='id'
)
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
$projectRoot=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$expected=[IO.Path]::GetFullPath((Join-Path $projectRoot 'src-tauri\target\debug\core-pulse.exe'))
$process=Get-Process -Id $AppProcessId -ErrorAction Stop
if (![string]::Equals($process.Path,$expected,[StringComparison]::OrdinalIgnoreCase)) { throw 'Only the workspace debug QA application can be controlled.' }
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class CorePulseQaTray {
    public delegate bool EnumCallback(IntPtr window, IntPtr argument);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumCallback callback, IntPtr argument);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr window, StringBuilder name, int capacity);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetWindowText(IntPtr window, StringBuilder text, int capacity);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr window, uint message, IntPtr wParam, IntPtr lParam);
    [StructLayout(LayoutKind.Sequential)] public struct IconIdentity { public uint size; public IntPtr window; public uint id; public Guid guid; }
    [StructLayout(LayoutKind.Sequential)] public struct Rect { public int left, top, right, bottom; }
    [DllImport("shell32.dll")] static extern int Shell_NotifyIconGetRect(ref IconIdentity identity, out Rect rect);
    [StructLayout(LayoutKind.Sequential)] public struct MenuBarInfo { public uint size; public Rect rect; public IntPtr menu, menuWindow; public uint flags; }
    [DllImport("user32.dll")] static extern bool GetMenuBarInfo(IntPtr window, int objectId, int itemId, ref MenuBarInfo info);
    [DllImport("user32.dll")] static extern int GetMenuItemCount(IntPtr menu);
    [DllImport("user32.dll")] static extern uint GetMenuItemID(IntPtr menu, int position);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetMenuString(IntPtr menu, uint item, StringBuilder text, int capacity, uint flags);
    public static string[] MenuLabels(IntPtr window) {
        var info=new MenuBarInfo {size=(uint)Marshal.SizeOf(typeof(MenuBarInfo))};
        if(!GetMenuBarInfo(window,-4,0,ref info) || info.menu==IntPtr.Zero) throw new InvalidOperationException("Native popup menu is unavailable.");
        int count=GetMenuItemCount(info.menu);
        if(count!=2) throw new InvalidOperationException("Expected exactly two native menu items.");
        var labels=new List<string>();
        for(uint i=0;i<count;i++) {
            var text=new StringBuilder(256);
            if(GetMenuString(info.menu,i,text,text.Capacity,0x400)==0) throw new InvalidOperationException("Native menu label is unavailable.");
            labels.Add(text.ToString());
        }
        return labels.ToArray();
    }
    public static uint MenuCommand(IntPtr window, int position) {
        var info=new MenuBarInfo {size=(uint)Marshal.SizeOf(typeof(MenuBarInfo))};
        if(!GetMenuBarInfo(window,-4,0,ref info) || info.menu==IntPtr.Zero) throw new InvalidOperationException("Native popup menu is unavailable.");
        uint id=GetMenuItemID(info.menu,position);
        if(id==0 || id==UInt32.MaxValue || id>UInt16.MaxValue) throw new InvalidOperationException("Invalid native menu command.");
        return id;
    }
    public static IntPtr[] OwnedWindows(uint processId, string className, string title) {
        var windows=new List<IntPtr>();
        EnumWindows((window, argument) => {
            uint owner; GetWindowThreadProcessId(window, out owner);
            if(owner!=processId) return true;
            var name=new StringBuilder(256); GetClassName(window,name,name.Capacity);
            var text=new StringBuilder(256); GetWindowText(window,text,text.Capacity);
            if(name.ToString()==className && (title==null || text.ToString()==title)) windows.Add(window);
            return true;
        }, IntPtr.Zero);
        return windows.ToArray();
    }
    public static uint RegisteredIcon(IntPtr window) {
        // tray-icon 0.24.2 allocates small sequential IDs. Require an actual shell registration.
        uint found=0;
        for(uint id=1;id<=16;id++) {
            var identity=new IconIdentity {size=(uint)Marshal.SizeOf(typeof(IconIdentity)),window=window,id=id};
            Rect rect;
            if(Shell_NotifyIconGetRect(ref identity,out rect)==0) {
                if(found!=0) throw new InvalidOperationException("Multiple owned tray registrations.");
                found=id;
            }
        }
        if(found==0) throw new InvalidOperationException("No owned icon registered with the Windows shell.");
        return found;
    }
}
'@
$main=@([CorePulseQaTray]::OwnedWindows([uint32]$AppProcessId,'Tauri Window','Core Pulse'))
$tray=@([CorePulseQaTray]::OwnedWindows([uint32]$AppProcessId,'tray_icon_app',$null))
if ($main.Count -ne 1 -or $tray.Count -ne 1) { throw 'Expected one owned Tauri window and one owned tray window.' }
$iconId=[CorePulseQaTray]::RegisteredIcon($tray[0])
if ($Action -eq 'HideOnClose') {
    if (![CorePulseQaTray]::PostMessage($main[0],0x0010,[IntPtr]::Zero,[IntPtr]::Zero)) { throw 'Close request failed.' }
    $deadline=[DateTime]::UtcNow.AddSeconds(5)
    while ([CorePulseQaTray]::IsWindowVisible($main[0]) -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 50 }
    if ($process.HasExited -or [CorePulseQaTray]::IsWindowVisible($main[0])) { throw 'Close did not hide the live application.' }
} elseif ($Action -eq 'Open') {
    # Dispatch the native shell callback to the PID-verified icon window. No global mouse/keyboard input.
    if (![CorePulseQaTray]::PostMessage($tray[0],6002,[IntPtr]$iconId,[IntPtr]0x0202)) { throw 'Tray left-click callback failed.' }
    $deadline=[DateTime]::UtcNow.AddSeconds(5)
    while (![CorePulseQaTray]::IsWindowVisible($main[0]) -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 50 }
    if (![CorePulseQaTray]::IsWindowVisible($main[0])) { throw 'Tray callback did not show the main window.' }
} else {
    try {
    if (![CorePulseQaTray]::PostMessage($tray[0],6002,[IntPtr]$iconId,[IntPtr]0x0205)) { throw 'Tray menu callback failed.' }
    $deadline=[DateTime]::UtcNow.AddSeconds(5)
    do {
        $menus=@([CorePulseQaTray]::OwnedWindows([uint32]$AppProcessId,'#32768',$null) | Where-Object {[CorePulseQaTray]::IsWindowVisible($_)})
        if ($menus.Count -eq 0) { Start-Sleep -Milliseconds 50 }
    } while ($menus.Count -eq 0 -and [DateTime]::UtcNow -lt $deadline)
    if ($menus.Count -ne 1) { throw 'Expected exactly one PID-owned native tray menu.' }
    $actualLabels=[CorePulseQaTray]::MenuLabels($menus[0])
    $labels=switch($Language) { 'en' {@('Open Core Pulse','Quit Core Pulse')} 'es' {@('Abrir Core Pulse','Salir de Core Pulse')} default {@('Buka Core Pulse','Keluar dari Core Pulse')} }
    if ($actualLabels.Count -ne 2 -or $actualLabels[0] -ne $labels[0] -or $actualLabels[1] -ne $labels[1]) { throw ('Native tray menu labels do not match the selected language: '+($actualLabels -join ', ')) }
    # Read the real popup's command ID, dismiss that owner's menu, and dispatch its
    # native WM_COMMAND. This exercises menu routing without global keyboard input.
    $index=if ($Action -eq 'SelectQuit') {1} else {0}
    $command=[CorePulseQaTray]::MenuCommand($menus[0],$index)
    if (![CorePulseQaTray]::PostMessage($tray[0],0x001F,[IntPtr]::Zero,[IntPtr]::Zero)) {throw 'Owned menu dismissal failed.'}
    if (![CorePulseQaTray]::PostMessage($tray[0],0x0111,[IntPtr]$command,[IntPtr]::Zero)) {throw 'Native menu command failed.'}
    if ($Action -eq 'SelectQuit' -and !$process.WaitForExit(15000)) { throw 'Tray quit did not complete graceful shutdown.' }
    # InspectMenu selects Open to dismiss only this app's popup without global input.
    [pscustomobject]@{labels=$labels;action=$Action;iconRegistered=$true}|ConvertTo-Json -Compress
    } finally {
        [uint32]$dismissOwner=0
        [void][CorePulseQaTray]::GetWindowThreadProcessId($tray[0],[ref]$dismissOwner)
        if ($dismissOwner -eq $AppProcessId) { [void][CorePulseQaTray]::PostMessage($tray[0],0x001F,[IntPtr]::Zero,[IntPtr]::Zero) }
    }
}
Write-Output "Owned QA tray action completed: $Action"
