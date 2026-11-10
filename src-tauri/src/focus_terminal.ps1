$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class CamTerminalFocus {
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool ShowWindowAsync(IntPtr hwnd, int cmd);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
}
"@
$element = [Windows.Automation.AutomationElement]
$scope = [Windows.Automation.TreeScope]
$marker = $env:CAM_FOCUS_TITLE
if ($marker -notmatch '^CAM-[0-9a-f-]{36}$') { exit 1 }
$tabType = [Windows.Automation.PropertyCondition]::new($element::ControlTypeProperty, [Windows.Automation.ControlType]::TabItem)
$tabName = [Windows.Automation.PropertyCondition]::new($element::NameProperty, $marker)
$tabMatch = [Windows.Automation.AndCondition]::new($tabType, $tabName)
$matchesFound = @()
foreach ($app in @(Get-Process -Name WindowsTerminal -ErrorAction SilentlyContinue)) {
    $forPid = [Windows.Automation.PropertyCondition]::new($element::ProcessIdProperty, [int]$app.Id)
    foreach ($frame in $element::RootElement.FindAll($scope::Children, $forPid)) {
        foreach ($tab in $frame.FindAll($scope::Descendants, $tabMatch)) {
            $matchesFound += [pscustomobject]@{Frame=$frame; Tab=$tab}
        }
    }
}
if ($matchesFound.Count -ne 1) { exit 2 }
$match = $matchesFound[0]
$hwnd = [IntPtr]$match.Frame.Current.NativeWindowHandle
$selection = $match.Tab.GetCurrentPattern([Windows.Automation.SelectionItemPattern]::Pattern)
if ($match.Tab.Current.Name -cne $marker -or $hwnd -eq [IntPtr]::Zero) { exit 3 }
if ([CamTerminalFocus]::IsIconic($hwnd)) { [void][CamTerminalFocus]::ShowWindowAsync($hwnd, 9) }
$selection.Select()
[void][CamTerminalFocus]::SetForegroundWindow($hwnd)
$deadline = [DateTime]::UtcNow.AddMilliseconds(750)
do {
    if ($selection.Current.IsSelected -and [CamTerminalFocus]::GetForegroundWindow() -eq $hwnd -and ![CamTerminalFocus]::IsIconic($hwnd)) { exit 0 }
    Start-Sleep -Milliseconds 25
} while ([DateTime]::UtcNow -lt $deadline)
exit 4
