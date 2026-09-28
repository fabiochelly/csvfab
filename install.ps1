# csvfab installer for Windows — per user, no administrator rights needed.
#
#   irm https://raw.githubusercontent.com/fabiochelly/csvfab/main/install.ps1 | iex
#   .\install.ps1                 from a checkout or an unpacked release
#   .\install.ps1 -Uninstall      removes the app (your settings are kept)
#
# Environment: $env:CSVFAB_VERSION = "1.2.3" installs that release instead of the latest;
# $env:CSVFAB_UNINSTALL = 1 uninstalls when the script is piped into iex.
param([switch]$Uninstall)
$ErrorActionPreference = 'Stop'

$Repo    = if ($env:CSVFAB_REPO) { $env:CSVFAB_REPO } else { 'fabiochelly/csvfab' }
$Version = if ($env:CSVFAB_VERSION) { $env:CSVFAB_VERSION } else { 'latest' }
$Dest    = Join-Path $env:LOCALAPPDATA 'Programs\csvfab'
$Link    = Join-Path ([Environment]::GetFolderPath('Programs')) 'csvfab.lnk'
$Classes = 'HKCU:\Software\Classes'

function Say($m)  { Write-Host "==> $m" -ForegroundColor Cyan }
function Warn($m) { Write-Host "warning: $m" -ForegroundColor Yellow }

function Set-UserPath([switch]$Remove) {
    $p = [Environment]::GetEnvironmentVariable('Path', 'User'); if (-not $p) { $p = '' }
    $parts = $p.Split(';') | Where-Object { $_ -and ($_ -ne $Dest) }
    if (-not $Remove) { $parts += $Dest }
    [Environment]::SetEnvironmentVariable('Path', ($parts -join ';'), 'User')
}

if ($Uninstall -or $env:CSVFAB_UNINSTALL) {
    Say 'Removing csvfab'
    Remove-Item -Recurse -Force $Dest, $Link -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force "$Classes\csvfab.table" -ErrorAction SilentlyContinue
    foreach ($ext in '.csv', '.tsv') { Remove-ItemProperty "$Classes\$ext\OpenWithProgids" -Name 'csvfab.table' -ErrorAction SilentlyContinue }
    Set-UserPath -Remove
    Say 'Done. Settings and the browser profile stay in %LOCALAPPDATA%\csvfab.'
    return
}

# --- requirements -----------------------------------------------------------
$pyw = (Get-Command pyw.exe -ErrorAction SilentlyContinue).Source
if (-not $pyw) { $pyw = (Get-Command pythonw.exe -ErrorAction SilentlyContinue).Source }
if (-not $pyw) { throw 'Python 3 is required: winget install Python.Python.3.12 (then open a new terminal).' }

# --- sources: this folder, or the release archive ---------------------------
$Src = $null
if ($PSScriptRoot -and (Test-Path (Join-Path $PSScriptRoot 'csvfab.py')) -and (Test-Path (Join-Path $PSScriptRoot 'viewer.htm'))) { $Src = $PSScriptRoot }
if (-not $Src) {
    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("csvfab-" + [guid]::NewGuid())
    New-Item -ItemType Directory $tmp | Out-Null
    $url = if ($Version -eq 'latest') { "https://github.com/$Repo/releases/latest/download/csvfab.zip" }
           else { "https://github.com/$Repo/releases/download/v$Version/csvfab-$Version.zip" }
    Say "Downloading $url"
    Invoke-WebRequest $url -OutFile "$tmp\csvfab.zip" -UseBasicParsing
    Expand-Archive "$tmp\csvfab.zip" $tmp
    $Src = Join-Path $tmp 'csvfab'
}

# --- install ----------------------------------------------------------------
Say "Installing into $Dest"
Remove-Item -Recurse -Force $Dest -ErrorAction SilentlyContinue
New-Item -ItemType Directory "$Dest\icons" -Force | Out-Null
foreach ($f in 'csvfab.py', 'csvfab.cmd', 'server.py', 'viewer.htm', 'papaparse.min.js', 'LICENSE') { Copy-Item (Join-Path $Src $f) $Dest }
Copy-Item (Join-Path $Src 'icons\csvfab.svg'), (Join-Path $Src 'icons\csvfab.ico') "$Dest\icons"

# Start menu shortcut: pythonw, so no console window flashes.
$sh = (New-Object -ComObject WScript.Shell).CreateShortcut($Link)
$sh.TargetPath = $pyw
$sh.Arguments = "`"$Dest\csvfab.py`""
$sh.WorkingDirectory = $Dest
$sh.IconLocation = "$Dest\icons\csvfab.ico"
$sh.Description = 'csvfab — CSV editor'
$sh.Save()

# "Open with" for .csv and .tsv (it does not take over the default app).
$cmd = "`"$pyw`" `"$Dest\csvfab.py`" `"%1`""
New-Item -Force "$Classes\csvfab.table\shell\open\command" | Out-Null
Set-Item "$Classes\csvfab.table" 'CSV table (csvfab)'
Set-Item "$Classes\csvfab.table\shell\open\command" $cmd
New-Item -Force "$Classes\csvfab.table\DefaultIcon" | Out-Null
Set-Item "$Classes\csvfab.table\DefaultIcon" "$Dest\icons\csvfab.ico"
foreach ($ext in '.csv', '.tsv') {
    New-Item -Force "$Classes\$ext\OpenWithProgids" | Out-Null
    New-ItemProperty -Force "$Classes\$ext\OpenWithProgids" -Name 'csvfab.table' -Value '' -PropertyType String | Out-Null
}

Set-UserPath        # csvfab.cmd from any new terminal
$ok = @('chrome.exe', 'msedge.exe', 'brave.exe') | Where-Object { Get-Command $_ -ErrorAction SilentlyContinue }
if (-not $ok -and -not (Test-Path "${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe")) { Warn 'No Chromium-based browser found: csvfab needs Chrome, Edge, Brave or Chromium.' }
Say 'csvfab installed: Start menu > csvfab, or `csvfab file.csv` in a new terminal.'
