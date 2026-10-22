; Mesh's Windows installer (Inno Setup 6). Build with installer\build.ps1.
; Per-user: no admin prompt, installs to %LOCALAPPDATA%\Programs\Mesh, and
; leaves your accounts and chats (%APPDATA%\mesh) alone on uninstall.

#define Root ".."
#define AppExe Root + "\target\release\mesh-app.exe"
#define AppVersion GetVersionNumbersString(AppExe)

[Setup]
AppId={{D5884734-20BD-4787-83B8-C0C7491957AD}
AppName=Mesh
AppVersion={#AppVersion}
AppVerName=Mesh {#AppVersion}
AppPublisher=Mesh
AppPublisherURL=https://github.com/GloriousBrendon/Mesh-Voice
VersionInfoDescription=Mesh Setup
PrivilegesRequired=lowest
DefaultDirName={localappdata}\Programs\Mesh
DisableProgramGroupPage=yes
DisableDirPage=yes
SetupIconFile={#Root}\crates\mesh-app\assets\mesh.ico
UninstallDisplayIcon={app}\Mesh.exe
UninstallDisplayName=Mesh
OutputDir={#Root}\dist
OutputBaseFilename=Mesh-Setup-{#AppVersion}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Mesh and its helpers may be running: close them (Restart Manager) first.
CloseApplications=force
RestartApplications=no

[Tasks]
Name: desktopicon; Description: "Create a desktop shortcut"; Flags: unchecked
Name: autostart; Description: "Start Mesh when I sign in to Windows"; Flags: unchecked

[Files]
Source: "{#AppExe}"; DestDir: "{app}"; DestName: "Mesh.exe"; Flags: ignoreversion
Source: "{#Root}\mesh-wa\target\release\mesh-wa.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\mesh-dc\target\release\mesh-dc.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\Mesh"; Filename: "{app}\Mesh.exe"; Comment: "Matrix, Discord and WhatsApp in one place"
Name: "{userdesktop}\Mesh"; Filename: "{app}\Mesh.exe"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Mesh"; ValueData: """{app}\Mesh.exe"""; Tasks: autostart; Flags: uninsdeletevalue

[Run]
Filename: "{app}\Mesh.exe"; Description: "Start Mesh"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/F /IM Mesh.exe /IM mesh-wa.exe /IM mesh-dc.exe"; Flags: runhidden; RunOnceId: "StopMesh"
