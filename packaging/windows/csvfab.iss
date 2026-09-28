; Inno Setup script: a per-user installer (no admin rights) for winget and direct download.
; Python is required and not bundled: the winget manifest declares it as a dependency.
; Built by .github/workflows/release.yml:  iscc /DAppVersion=1.2.3 packaging\windows\csvfab.iss
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
UninstallDisplayIcon={app}\icons\csvfab.ico
LicenseFile=..\..\LICENSE
Compression=lzma2
SolidCompression=yes
ChangesEnvironment=yes
ChangesAssociations=yes
WizardStyle=modern

[Files]
Source: "..\..\csvfab.py";         DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\csvfab.cmd";        DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\server.py";         DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\viewer.htm";        DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\papaparse.min.js";  DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE";           DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\icons\csvfab.svg";  DestDir: "{app}\icons"; Flags: ignoreversion
Source: "..\..\ui\*";             DestDir: "{app}\ui";    Flags: ignoreversion recursesubdirs
Source: "..\..\icons\csvfab.ico";  DestDir: "{app}\icons"; Flags: ignoreversion

[Icons]
; Minimised: csvfab.cmd hands over to pyw at once, so its console only flickers in the taskbar.
Name: "{userprograms}\csvfab"; Filename: "{app}\csvfab.cmd"; WorkingDir: "{app}"; IconFilename: "{app}\icons\csvfab.ico"; Flags: runminimized

[Registry]
; csvfab.cmd reachable from any new terminal.
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Check: NeedsAddPath(ExpandConstant('{app}'))
; "Open with" for .csv / .tsv, without taking over the default application.
Root: HKCU; Subkey: "Software\Classes\csvfab.table"; ValueType: string; ValueData: "CSV table (csvfab)"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\csvfab.table\DefaultIcon"; ValueType: string; ValueData: "{app}\icons\csvfab.ico"
Root: HKCU; Subkey: "Software\Classes\csvfab.table\shell\open\command"; ValueType: string; ValueData: """{app}\csvfab.cmd"" ""%1"""
Root: HKCU; Subkey: "Software\Classes\.csv\OpenWithProgids"; ValueType: string; ValueName: "csvfab.table"; ValueData: ""; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.tsv\OpenWithProgids"; ValueType: string; ValueName: "csvfab.table"; ValueData: ""; Flags: uninsdeletevalue

[Code]
function NeedsAddPath(Dir: string): Boolean;
var Path: string;
begin
  if not RegQueryStringValue(HKCU, 'Environment', 'Path', Path) then begin Result := True; exit; end;
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Path) + ';') = 0;
end;

function InitializeSetup(): Boolean;
begin
  Result := True;
  if not (RegKeyExists(HKCU, 'Software\Python\PythonCore') or RegKeyExists(HKLM, 'Software\Python\PythonCore')) then
    MsgBox('csvfab needs Python 3, which was not found.' + #13#10#13#10 +
           'Install it with "winget install Python.Python.3.12" or from python.org, then start csvfab.', mbInformation, MB_OK);
end;
