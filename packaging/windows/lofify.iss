; Inno Setup script for Lofify (Windows).
;
; Build:  cargo build --release -p lofify-app -p lofify-cli
;         iscc /DAppVersion=0.1.0 packaging\windows\lofify.iss
; Output: dist\Lofify-<version>-setup.exe
;
; This installer deliberately does NOT bundle or install a virtual audio cable driver
; (VB-Cable's licence does not allow redistribution). See docs\SETUP_WINDOWS.md.
; UNTESTED: this script has not been run.

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#define AppName "Lofify"
#define AppExe "lofify-app.exe"

[Setup]
AppId={{B8A1F0C2-6E1B-4E52-9D0A-4C0F1F5A7A11}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher=Lofify contributors
AppPublisherURL=https://github.com/anushibinj/lofi-dac
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
OutputDir=..\..\dist
OutputBaseFilename=Lofify-{#AppVersion}-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Per-user install by default (no administrator prompt); the dialog lets people choose all users.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
InfoBeforeFile=installer-readme.txt
UninstallDisplayName={#AppName}
UninstallDisplayIcon={app}\{#AppExe}

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; Flags: unchecked

[Files]
Source: "..\..\target\release\lofify-app.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\target\release\lofify.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE-MIT"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE-APACHE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\docs\SETUP_WINDOWS.md"; DestDir: "{app}\docs"; Flags: ignoreversion
Source: "..\..\docs\LISTENING_TESTS.md"; DestDir: "{app}\docs"; Flags: ignoreversion

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{group}\Setup guide"; Filename: "{app}\docs\SETUP_WINDOWS.md"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExe}"; Description: "Launch {#AppName}"; Flags: nowait postinstall skipifsilent

[Code]
{ The app registers itself for "launch at login" in the current user's Run key. Remove that entry
  when uninstalling so nothing points at a deleted program. }
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RegDeleteValue(HKEY_CURRENT_USER, 'Software\Microsoft\Windows\CurrentVersion\Run', 'Lofify');
end;
