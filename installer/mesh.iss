; Mesh's Windows installer (Inno Setup 6). Build with installer\build.ps1.
; Per-user: no admin prompt, installs to %LOCALAPPDATA%\Programs\Mesh, and
; leaves your accounts and chats (%APPDATA%\mesh) alone on uninstall.

#define Root ".."
; What build.ps1 passes: the Mesh-Node checkout, the Rust target the
; binaries were built for, and the two commits they were built from.
#ifndef Node
  #define Node Root + "\..\Mesh-Node"
#endif
#ifndef Target
  #define Target "x86_64-pc-windows-msvc"
#endif
#ifndef AppCommit
  #define AppCommit "unknown"
#endif
#ifndef NodeCommit
  #define NodeCommit "unknown"
#endif
#define Bin "\target\" + Target + "\release\"
#define AppExe Root + Bin + "mesh-app.exe"
#define AppVersion GetVersionNumbersString(AppExe)
#define Built AppVersion + " (app " + AppCommit + ", engine " + NodeCommit + ")"

[Setup]
AppId={{D5884734-20BD-4787-83B8-C0C7491957AD}
AppName=Mesh
AppVersion={#AppVersion}
AppVerName=Mesh {#AppVersion}
AppPublisher=Mesh
AppPublisherURL=https://github.com/GloriousBrendon/Mesh-Voice
VersionInfoDescription=Mesh Setup
VersionInfoVersion={#AppVersion}
VersionInfoProductTextVersion={#Built}
VersionInfoComments=Mesh {#Built}
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
Source: "{#Root}\mesh-wa{#Bin}mesh-wa.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\mesh-dc{#Bin}mesh-dc.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Node}{#Bin}mesh-node.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\crates\mesh-app\assets\mesh.png"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\Mesh"; Filename: "{app}\Mesh.exe"; Comment: "Matrix, Discord and WhatsApp in one place"
Name: "{userdesktop}\Mesh"; Filename: "{app}\Mesh.exe"; Tasks: desktopicon

[Registry]
; Mesh's name and icon on its notifications (see `notify` in main.rs), instead of PowerShell's.
Root: HKCU; Subkey: "Software\Classes\AppUserModelId\Mesh.Mesh"; ValueType: string; ValueName: "DisplayName"; ValueData: "Mesh"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\AppUserModelId\Mesh.Mesh"; ValueType: string; ValueName: "IconUri"; ValueData: "{app}\mesh.png"
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Mesh"; ValueData: """{app}\Mesh.exe"""; Tasks: autostart; Flags: uninsdeletevalue

[Run]
Filename: "{app}\Mesh.exe"; Description: "Start Mesh"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/F /IM Mesh.exe /IM mesh-wa.exe /IM mesh-dc.exe /IM mesh-node.exe"; Flags: runhidden; RunOnceId: "StopMesh"
