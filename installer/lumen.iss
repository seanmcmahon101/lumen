#ifndef AppVersion
  #error Pass AppVersion to ISCC with /DAppVersion=<version>.
#endif

[Setup]
AppId=Lumen.Launcher
AppName=Lumen
AppVersion={#AppVersion}
DefaultDirName={localappdata}\Lumen
DisableDirPage=yes
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
MinVersion=10.0
AppMutex=Local\Lumen.Launcher
UninstallDisplayIcon={app}\lumen.exe
OutputDir=..\target\installer
OutputBaseFilename=Lumen-Setup-x64
WizardStyle=modern

[Files]
Source: "..\target\x86_64-pc-windows-msvc\release\lumen.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\Lumen"; Filename: "{app}\lumen.exe"; Comment: "Start Lumen, then press Win+Space to search"

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "Lumen"; Flags: uninsdeletevalue

[Run]
Filename: "{app}\lumen.exe"; Description: "Start Lumen (press Win+Space to search)"; Flags: nowait postinstall skipifsilent
