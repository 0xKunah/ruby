; Ruby installer (Inno Setup 6).
;
; Build from the repository root after `cargo build --release -p ruby`:
;   ISCC.exe /DAppVersion=0.1.0 installer\ruby.iss
; The installer is written to dist\Ruby-Setup-<version>.exe.
;
; Installs for the current user by default (no administrator rights needed), with an option to
; install for all users. Creates a Start menu entry, a desktop shortcut (can be unchecked) and an
; uninstaller listed in Windows Settings > Apps. User settings in %APPDATA%\Ruby and the notes
; themselves are never touched by the uninstaller.

#ifndef AppVersion
#define AppVersion "0.0.0"
#endif

[Setup]
; Keep this AppId forever: it is how Windows recognizes upgrades of the same app.
AppId={{B15B3442-FAB4-499C-9193-7E2DC4244003}
AppName=Ruby
AppVersion={#AppVersion}
AppVerName=Ruby {#AppVersion}
AppPublisher=Ruby contributors
VersionInfoVersion={#AppVersion}
VersionInfoDescription=Ruby installer
DefaultDirName={autopf}\Ruby
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
SourceDir=..
OutputDir=dist
OutputBaseFilename=Ruby-Setup-{#AppVersion}
SetupIconFile=crates\app\assets\ruby.ico
UninstallDisplayIcon={app}\Ruby.exe
UninstallDisplayName=Ruby
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "french"; MessagesFile: "compiler:Languages\French.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "target\release\Ruby.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Ruby"; Filename: "{app}\Ruby.exe"
Name: "{autodesktop}\Ruby"; Filename: "{app}\Ruby.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\Ruby.exe"; Description: "{cm:LaunchProgram,Ruby}"; Flags: nowait postinstall skipifsilent
