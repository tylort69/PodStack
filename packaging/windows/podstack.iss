; Inno Setup script for the Windows installer: a per-user install (no administrator prompt) into
; %LOCALAPPDATA%\Programs\PodStack Native, a Start menu shortcut and an uninstaller.
;
;   iscc /DAppVersion=0.4.0 packaging\windows\podstack.iss      (after bundle.sh has filled dist\PodStack)
;
; "PodStack Native" everywhere Windows shows a name, so it can sit beside the original Electron PodStack without
; either replacing the other's shortcut or uninstall entry.

#ifndef AppVersion
  #error Pass the version: iscc /DAppVersion=x.y.z podstack.iss
#endif

[Setup]
; Never change AppId: Windows uses it to recognise an upgrade of an existing install.
AppId={{AA190A3E-4711-40CD-A2A1-D1B53E7869BE}
AppName=PodStack Native
AppVersion={#AppVersion}
AppVerName=PodStack Native {#AppVersion}
AppPublisher=PodStack
AppPublisherURL=https://github.com/tylort69/PodStack
AppSupportURL=https://github.com/tylort69/PodStack/issues
AppUpdatesURL=https://github.com/tylort69/PodStack/releases
DefaultDirName={autopf}\PodStack Native
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir=..\..\dist
OutputBaseFilename=PodStack-Native-Setup-{#AppVersion}
SetupIconFile=podstack.ico
UninstallDisplayIcon={app}\bin\podstack.exe
UninstallDisplayName=PodStack Native
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; Closes a running PodStack before upgrading its files, and starts it again afterwards.
CloseApplications=yes
RestartApplications=yes

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[InstallDelete]
; An upgrade replaces the whole runtime, so DLLs a newer GTK no longer ships don't linger.
Type: filesandordirs; Name: "{app}\bin"
Type: filesandordirs; Name: "{app}\lib"
Type: filesandordirs; Name: "{app}\share"

[Files]
Source: "..\..\dist\PodStack\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
; The app id matches the one PodStack gives its process, so Windows notifications show as PodStack.
Name: "{autoprograms}\PodStack Native"; Filename: "{app}\bin\podstack.exe"; AppUserModelID: "io.github.tylort69.PodStack"
Name: "{autodesktop}\PodStack Native"; Filename: "{app}\bin\podstack.exe"; AppUserModelID: "io.github.tylort69.PodStack"; Tasks: desktopicon

[Run]
Filename: "{app}\bin\podstack.exe"; Description: "{cm:LaunchProgram,PodStack}"; Flags: nowait postinstall skipifsilent
