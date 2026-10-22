; CleanCore setup wizard (Inno Setup 6), published with every release as
; cleancore-<version>-windows-x64-setup.exe.
;
;   iscc /DAppVersion=0.2.3 /DBinDir=..\..\target\release /DOutDir=..\..\out packaging\windows\cleancore.iss
;
; The self-update (src/update.rs) runs a newer copy of this setup silently over an install
; made with it: it recognizes such an install by the uninstaller (unins000.exe) next to the program.

#ifndef AppVersion
  #error Pass the version: /DAppVersion=x.y.z
#endif
#ifndef BinDir
  #define BinDir "..\..\target\release"
#endif
#ifndef OutDir
  #define OutDir "..\..\out"
#endif

[Setup]
; Never change AppId: Windows tells installs apart by it, and upgrades find the old one with it.
AppId={{47E654CF-7D00-41BE-867A-7712BE990BAB}
AppName=CleanCore
AppVersion={#AppVersion}
AppVerName=CleanCore {#AppVersion}
AppPublisher=CleanCore
AppPublisherURL=https://github.com/sudo-py-dev/CleanCore
AppSupportURL=https://github.com/sudo-py-dev/CleanCore/issues
AppUpdatesURL=https://github.com/sudo-py-dev/CleanCore/releases
VersionInfoVersion={#AppVersion}
DefaultDirName={autopf}\CleanCore
DefaultGroupName=CleanCore
DisableProgramGroupPage=yes
; CleanCore works as administrator (uninstallers, the registry, services), so it installs for
; all users by default; the wizard still offers an install for the current user only.
PrivilegesRequired=admin
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
LicenseFile=..\..\LICENSE
SetupIconFile=cleancore.ico
UninstallDisplayIcon={app}\cleancore.ico
UninstallDisplayName=CleanCore
WizardStyle=modern
; A running CleanCore is closed for the upgrade and started again afterwards.
CloseApplications=yes
RestartApplications=yes
Compression=lzma2/max
SolidCompression=yes
OutputDir={#OutDir}
OutputBaseFilename=cleancore-{#AppVersion}-windows-x64-setup
ShowLanguageDialog=auto

[Languages]
; The languages CleanCore itself has; the wizard follows the Windows language.
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "he"; MessagesFile: "compiler:Languages\Hebrew.isl"
Name: "es"; MessagesFile: "compiler:Languages\Spanish.isl"
Name: "fr"; MessagesFile: "compiler:Languages\French.isl"
Name: "de"; MessagesFile: "compiler:Languages\German.isl"
Name: "ru"; MessagesFile: "compiler:Languages\Russian.isl"
Name: "uk"; MessagesFile: "compiler:Languages\Ukrainian.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#BinDir}\cleancore-gui.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BinDir}\cleancore.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "cleancore.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\CleanCore"; Filename: "{app}\cleancore-gui.exe"; IconFilename: "{app}\cleancore.ico"; Comment: "Uninstaller and system cleaner"
Name: "{autodesktop}\CleanCore"; Filename: "{app}\cleancore-gui.exe"; IconFilename: "{app}\cleancore.ico"; Tasks: desktopicon

[Run]
Filename: "{app}\cleancore-gui.exe"; Description: "{cm:LaunchProgram,CleanCore}"; Flags: nowait postinstall skipifsilent
; An update CleanCore started (silent): CleanCore closed for it, so start it again.
Filename: "{app}\cleancore-gui.exe"; Flags: nowait runasoriginaluser; Check: WizardSilent

[UninstallDelete]
; Copies an in-place update left behind.
Type: files; Name: "{app}\cleancore.old"
Type: files; Name: "{app}\cleancore-gui.old"
