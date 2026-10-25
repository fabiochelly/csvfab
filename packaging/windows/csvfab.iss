; Inno Setup script: a per-user installer (no admin rights) for winget and direct download.
; Self-contained: Python's embeddable distribution ships in {app}\python, and csvfab.exe
; (csvfab.c) runs csvfab.py with it — winget refuses a package whose entry point is a
; script, which csvfab.cmd was. Both come from build\, filled by .github/workflows/release.yml:
;   iscc /DAppVersion=1.2.3 packaging\windows\csvfab.iss
#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{6C3A0F52-7E0B-4C1D-9B8A-C5F0B1A2D3E4}
AppName=csvfab
AppVersion={#AppVersion}
AppPublisher=Fabio Chelly
AppPublisherURL=https://github.com/fabiochelly/csvfab
DefaultDirName={localappdata}\Programs\csvfab
DefaultGroupName=csvfab
DisableProgramGroupPage=yes
DisableDirPage=yes
PrivilegesRequired=lowest
OutputDir=..\..\dist
OutputBaseFilename=csvfab-setup-{#AppVersion}
SetupIconFile=..\..\icons\csvfab.ico
UninstallDisplayIcon={app}\csvfab.exe
LicenseFile=..\..\LICENSE
Compression=lzma2
SolidCompression=yes
ChangesEnvironment=yes
ChangesAssociations=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[InstallDelete]
; Left by the versions that needed a system Python; and a bundled Python of another
; version would leave its pythonXY.dll and .zip behind.
Type: files;          Name: "{app}\csvfab.cmd"
Type: filesandordirs; Name: "{app}\python"

[UninstallDelete]
; The __pycache__ folders Python writes there at run time.
Type: filesandordirs; Name: "{app}\bridge"

[Files]
Source: "..\..\build\csvfab.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\build\python\*";  DestDir: "{app}\python"; Flags: ignoreversion recursesubdirs
Source: "..\..\csvfab.py";         DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\server.py";         DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\viewer.htm";        DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\papaparse.min.js";  DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE";           DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\icons\csvfab.svg";  DestDir: "{app}\icons"; Flags: ignoreversion
Source: "..\..\ui\*";             DestDir: "{app}\ui";    Flags: ignoreversion recursesubdirs
Source: "..\..\bridge\*";         DestDir: "{app}\bridge"; Flags: ignoreversion recursesubdirs; Excludes: "__pycache__"
Source: "..\..\icons\csvfab.ico";  DestDir: "{app}\icons"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\csvfab"; Filename: "{app}\csvfab.exe"; WorkingDir: "{app}"

[Registry]
; csvfab.exe reachable from any new terminal.
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Check: NeedsAddPath(ExpandConstant('{app}'))
; "Open with" for .csv / .tsv, without taking over the default application.
Root: HKCU; Subkey: "Software\Classes\csvfab.table"; ValueType: string; ValueData: "CSV table (csvfab)"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\csvfab.table\DefaultIcon"; ValueType: string; ValueData: "{app}\icons\csvfab.ico"
Root: HKCU; Subkey: "Software\Classes\csvfab.table\shell\open\command"; ValueType: string; ValueData: """{app}\csvfab.exe"" ""%1"""
Root: HKCU; Subkey: "Software\Classes\.csv\OpenWithProgids"; ValueType: string; ValueName: "csvfab.table"; ValueData: ""; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.tsv\OpenWithProgids"; ValueType: string; ValueName: "csvfab.table"; ValueData: ""; Flags: uninsdeletevalue

[Code]
function NeedsAddPath(Dir: string): Boolean;
var Path: string;
begin
  if not RegQueryStringValue(HKCU, 'Environment', 'Path', Path) then begin Result := True; exit; end;
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Path) + ';') = 0;
end;
