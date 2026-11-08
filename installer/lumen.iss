#ifndef AppVersion
  #error Pass AppVersion to ISCC with /DAppVersion=<version>.
#endif
#ifndef AppBinary
  #define AppBinary "..\target\x86_64-pc-windows-msvc\release\lumen.exe"
#endif
#ifndef BuildOutputDir
  #define BuildOutputDir "..\target\installer"
#endif

[Setup]
AppId=Lumen.Launcher
AppName=Lumen
AppVersion={#AppVersion}
AppPublisher=Sean McMahon
VersionInfoProductName=Lumen
VersionInfoProductVersion={#AppVersion}.0
VersionInfoProductTextVersion={#AppVersion}.0
DefaultDirName={localappdata}\Lumen
DisableDirPage=yes
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
MinVersion=10.0
UninstallDisplayIcon={app}\lumen.exe
OutputDir={#BuildOutputDir}
OutputBaseFilename=Lumen-Setup-{#AppVersion}-x64
WizardStyle=modern
#ifdef SignedBuild
SignedUninstaller=yes
SignedUninstallerDir={#UninstallerDirectory}
#endif

[Files]
Source: "{#AppBinary}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\Lumen"; Filename: "{app}\lumen.exe"; Comment: "Start Lumen, then press Win+Space to search"

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "Lumen"; Flags: uninsdeletevalue

[Run]
Filename: "{app}\lumen.exe"; Description: "Start Lumen (press Win+Space to search)"; Flags: nowait postinstall skipifsilent

[Code]
// Lumen runs in the background, so stop it rather than asking the user to.
procedure StopLumen;
var
  ResultCode: Integer;
begin
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM lumen.exe', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  Sleep(500);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  StopLumen;
  Result := '';
end;

function InitializeUninstall(): Boolean;
begin
  StopLumen;
  Result := True;
end;
