# Update rehearsal helpers (s115). Dot-sourced by update-rehearsal.yml.
# Pure ASCII on purpose (Windows PowerShell 5 reads files as ANSI).

$Served = Join-Path $env:RUNNER_TEMP 'served'
$Requests = Join-Path $env:RUNNER_TEMP 'requests.log'
$Install = 'C:\Program Files\Zaaheen'
$LogDir = Join-Path $env:LOCALAPPDATA 'com.zaaheen.app\logs'
$Pending = Join-Path $env:LOCALAPPDATA 'com.zaaheen.app\updates\pending.update'

function Write-Manifest([string]$Version, [string]$Url, [string]$SigFile) {
    $sig = (Get-Content $SigFile -Raw).Trim()
    $m = [ordered]@{
        version  = $Version
        notes    = 'rehearsal'
        pub_date = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
        platforms = @{ 'windows-x86_64' = @{ url = $Url; signature = $sig } }
    }
    New-Item -ItemType Directory -Force (Join-Path $Served 'updates') | Out-Null
    $m | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $Served 'updates\latest.json') -Encoding ascii
    Write-Host "manifest: $Version -> $Url"
}

function Stop-Zaaheen {
    foreach ($n in 'zaaheen-desktop', 'zaaheen', 'zaaheen-maintenance') {
        Get-Process -Name $n -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    }
    Start-Sleep -Seconds 3
}

function Start-Zaaheen {
    $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9222'
    $p = Start-Process -FilePath (Join-Path $Install 'zaaheen-desktop.exe') -PassThru
    Write-Host "started zaaheen-desktop pid $($p.Id)"
    $p
}

function Get-AppLog {
    if (-not (Test-Path $LogDir)) { return '' }
    (Get-ChildItem $LogDir -File | ForEach-Object { Get-Content $_.FullName -Raw }) -join "`n"
}

# Waits for any of the patterns in the app's log; returns the one seen or $null.
function Wait-AppLog([string[]]$Patterns, [int]$Seconds) {
    $until = (Get-Date).AddSeconds($Seconds)
    while ((Get-Date) -lt $until) {
        $log = Get-AppLog
        foreach ($p in $Patterns) { if ($log -match [regex]::Escape($p)) { return $p } }
        Start-Sleep -Seconds 5
    }
    $null
}

function Show-UpdateLog {
    Write-Host '--- update lines in the app log ---'
    (Get-AppLog) -split "`n" | Where-Object { $_ -match 'update' } | Select-Object -Last 20 | ForEach-Object { Write-Host $_ }
    Write-Host '--- requests to the stand-in server ---'
    if (Test-Path $Requests) { Get-Content $Requests | Select-Object -Last 20 | ForEach-Object { Write-Host $_ } }
}

function Get-InstalledVersion {
    $keys = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*',
            'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*'
    (Get-ItemProperty $keys -ErrorAction SilentlyContinue | Where-Object { $_.DisplayName -eq 'Zaaheen' } | Select-Object -First 1).DisplayVersion
}

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class Elevation {
    [DllImport("advapi32.dll", SetLastError = true)]
    static extern bool OpenProcessToken(IntPtr h, uint access, out IntPtr token);
    [DllImport("advapi32.dll", SetLastError = true)]
    static extern bool GetTokenInformation(IntPtr t, int cls, out int info, int len, out int ret);
    [DllImport("kernel32.dll")] static extern IntPtr OpenProcess(uint access, bool inherit, int pid);
    public static string Of(int pid) {
        IntPtr p = OpenProcess(0x1000, false, pid);
        if (p == IntPtr.Zero) return "unknown (cannot open process)";
        IntPtr t;
        if (!OpenProcessToken(p, 0x0008, out t)) return "unknown (cannot open token)";
        int elevated, ret;
        if (!GetTokenInformation(t, 20, out elevated, 4, out ret)) return "unknown";
        int kind;
        GetTokenInformation(t, 18, out kind, 4, out ret);
        return (elevated != 0 ? "elevated" : "not elevated") + " (elevation type " + kind + ")";
    }
}
'@
