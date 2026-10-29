; Inno Setup script: a per-user installer (no admin rights) for winget and direct download.
; csvfab.exe is the whole app (one program, nothing else to install); it comes from build\,
; filled by .github/workflows/release.yml:
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
; csvfab.exe is replaced while csvfab may be open: closed first, as any app being updated.
CloseApplications=yes

[InstallDelete]
; Left by csvfab 1.x (Python and its page, run in a browser window).
Type: files;          Name: "{app}\csvfab.cmd"
Type: files;          Name: "{app}\csvfab.py"
Type: files;          Name: "{app}\server.py"
Type: files;          Name: "{app}\viewer.htm"
Type: files;          Name: "{app}\papaparse.min.js"
Type: filesandordirs; Name: "{app}\python"
Type: filesandordirs; Name: "{app}\bridge"
Type: filesandordirs; Name: "{app}\ui"

[Files]
Source: "..\..\build\csvfab.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE";          DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\icons\csvfab.ico"; DestDir: "{app}\icons"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\csvfab"; Filename: "{app}\csvfab.exe"; WorkingDir: "{app}"; IconFilename: "{app}\icons\csvfab.ico"

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
