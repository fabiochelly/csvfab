# csvfab installer for Windows — per user, no administrator rights needed.
#
#   irm https://raw.githubusercontent.com/fabiochelly/csvfab/main/install.ps1 | iex
#   .\install.ps1                 from an unpacked release, or a checkout (built with cargo)
#   .\install.ps1 -Uninstall      removes the app (your settings are kept)
#
# Environment: $env:CSVFAB_VERSION = "2.0.0" installs that release instead of the latest;
# $env:CSVFAB_UNINSTALL = 1 uninstalls when the script is piped into iex.
param([switch]$Uninstall)
$ErrorActionPreference = 'Stop'

$Repo    = if ($env:CSVFAB_REPO) { $env:CSVFAB_REPO } else { 'fabiochelly/csvfab' }
$Version = if ($env:CSVFAB_VERSION) { $env:CSVFAB_VERSION } else { 'latest' }
$Dest    = Join-Path $env:LOCALAPPDATA 'Programs\csvfab'
$Link    = Join-Path ([Environment]::GetFolderPath('Programs')) 'csvfab.lnk'
$Classes = 'HKCU:\Software\Classes'

function Say($m)  { Write-Host "==> $m" -ForegroundColor Cyan }

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
    Say 'Done. Settings stay in %LOCALAPPDATA%\csvfab.'
    return
}

# --- the program: this folder's, or the release's -----------------------------
$Src = $null; $Exe = $null
if ($PSScriptRoot -and (Test-Path (Join-Path $PSScriptRoot 'csvfab.exe'))) {
    $Src = $PSScriptRoot; $Exe = Join-Path $Src 'csvfab.exe'                  # an unpacked release
} elseif ($PSScriptRoot -and (Test-Path (Join-Path $PSScriptRoot 'Cargo.toml'))) {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { throw 'Building from the sources needs Rust (https://rustup.rs), or: winget install FabioChelly.csvfab' }
    Say 'Building csvfab (cargo build --release): a few minutes the first time'
    Push-Location $PSScriptRoot
    try { cargo build --release --locked -p csvfab; if ($LASTEXITCODE) { throw 'the build failed' } } finally { Pop-Location }
    $Src = $PSScriptRoot; $Exe = Join-Path $Src 'target\release\csvfab.exe'   # a checkout
}
if (-not $Src) {
    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("csvfab-" + [guid]::NewGuid())
    New-Item -ItemType Directory $tmp | Out-Null
    $url = if ($Version -eq 'latest') { "https://github.com/$Repo/releases/latest/download/csvfab-windows-x86_64.zip" }
           else { "https://github.com/$Repo/releases/download/v$Version/csvfab-$Version-windows-x86_64.zip" }
    Say "Downloading $url"
    Invoke-WebRequest $url -OutFile "$tmp\csvfab.zip" -UseBasicParsing
    Expand-Archive "$tmp\csvfab.zip" $tmp
    $Src = Join-Path $tmp 'csvfab'; $Exe = Join-Path $Src 'csvfab.exe'
}

# --- install ----------------------------------------------------------------
Say "Installing into $Dest"
Remove-Item -Recurse -Force $Dest -ErrorAction SilentlyContinue   # also what csvfab 1.x installed there
New-Item -ItemType Directory "$Dest\icons" -Force | Out-Null
Copy-Item $Exe "$Dest\csvfab.exe"
Copy-Item (Join-Path $Src 'LICENSE') $Dest
Copy-Item (Join-Path $Src 'icons\csvfab.ico') "$Dest\icons"

# Start menu shortcut.
$sh = (New-Object -ComObject WScript.Shell).CreateShortcut($Link)
$sh.TargetPath = "$Dest\csvfab.exe"
$sh.WorkingDirectory = $Dest
$sh.IconLocation = "$Dest\icons\csvfab.ico"
$sh.Description = 'csvfab — CSV editor'
$sh.Save()

# "Open with" for .csv and .tsv (it does not take over the default app).
$cmd = "`"$Dest\csvfab.exe`" `"%1`""
New-Item -Force "$Classes\csvfab.table\shell\open\command" | Out-Null
Set-Item "$Classes\csvfab.table" 'CSV table (csvfab)'
Set-Item "$Classes\csvfab.table\shell\open\command" $cmd
New-Item -Force "$Classes\csvfab.table\DefaultIcon" | Out-Null
Set-Item "$Classes\csvfab.table\DefaultIcon" "$Dest\icons\csvfab.ico"
foreach ($ext in '.csv', '.tsv') {
    New-Item -Force "$Classes\$ext\OpenWithProgids" | Out-Null
    New-ItemProperty -Force "$Classes\$ext\OpenWithProgids" -Name 'csvfab.table' -Value '' -PropertyType String | Out-Null
}

Set-UserPath        # csvfab from any new terminal
Say 'csvfab installed: Start menu > csvfab, or `csvfab file.csv` in a new terminal.'
