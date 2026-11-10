; Inno Setup script: per-user installer (no admin rights), Start-menu shortcut, uninstaller.
#define AppVersion GetEnv("INKVOICE_VERSION")
#if AppVersion == ""
  #define AppVersion "0.1.0"
#endif

[Setup]
AppId={{8C1F2A7E-6B9D-4E3A-9F21-3D5B7A1C4E90}
AppName=InkVoice
AppVersion={#AppVersion}
AppPublisher=InkVoice
AppPublisherURL=https://github.com/sabeel111/InkVoice
DefaultDirName={localappdata}\Programs\InkVoice
DisableProgramGroupPage=yes
DisableDirPage=yes
PrivilegesRequired=lowest
OutputDir=..\..\dist
OutputBaseFilename=InkVoice-Setup-x86_64
SetupIconFile=..\..\assets\inkvoice.ico
UninstallDisplayIcon={app}\InkVoice.exe
UninstallDisplayName=InkVoice
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked

[Files]
Source: "..\..\target\release\inkvoice.exe"; DestDir: "{app}"; DestName: "InkVoice.exe"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\InkVoice"; Filename: "{app}\InkVoice.exe"
Name: "{autodesktop}\InkVoice"; Filename: "{app}\InkVoice.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\InkVoice.exe"; Description: "Start InkVoice"; Flags: nowait postinstall skipifsilent
