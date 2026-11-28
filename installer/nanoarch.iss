; NanoArch installer (Inno Setup 6). Build:
;   ISCC.exe /DExeSrc="<path to nanoarch-app.exe>" installer\nanoarch.iss
; Installs per user (no administrator rights) unless the user chooses "for all users".
; The installer is NOT code-signed, so Windows SmartScreen will warn on first run.

#define AppName "NanoArch"
#define AppVersion "2.13.0"
#define AppPublisher "NanoNexus"
#ifndef ExeSrc
  #error Pass the built executable: /DExeSrc="C:\path\to\nanoarch-app.exe"
#endif

[Setup]
AppId={{B6E2C1F4-6A0B-4B6E-9B7A-3A1F0D9C5E21}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\dist
OutputBaseFilename=NanoArch-Setup-{#AppVersion}
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\icon.ico
UninstallDisplayName={#AppName}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
VersionInfoVersion={#AppVersion}
VersionInfoCompany={#AppPublisher}
VersionInfoDescription={#AppName} setup

[Languages]
Name: "arabic"; MessagesFile: "compiler:Languages\Arabic.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#ExeSrc}"; DestDir: "{app}"; DestName: "NanoArch.exe"; Flags: ignoreversion
Source: "..\assets\icon.ico"; DestDir: "{app}"; Flags: ignoreversion
; Empty templates to fill in: a code rule pack (every value null) and production rates (every value null).
Source: "..\nnx-core\packs\*.json"; DestDir: "{app}\templates"; Flags: ignoreversion
; Licences of the bundled third-party parts (the interface font, SIL OFL 1.1, and the PDF engine, Apache-2.0), beside the program.
Source: "..\assets\vendor\fonts\plex-sans-arabic-OFL-LICENSE.txt"; DestDir: "{app}\licenses"; Flags: ignoreversion
Source: "..\assets\vendor\pdfjs-LICENSE.txt"; DestDir: "{app}\licenses"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\NanoArch.exe"; IconFilename: "{app}\icon.ico"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\NanoArch.exe"; IconFilename: "{app}\icon.ico"; Tasks: desktopicon

[UninstallDelete]
; The app creates its WebView2 profile (cache, local storage) beside the executable on first run. It is not an installed file, so the
; uninstaller would leave it behind; remove it so the install folder goes away completely. Projects (.nnx) and the application data
; under %APPDATA% are not touched.
Type: filesandordirs; Name: "{app}\NanoArch.exe.WebView2"
; An install into a folder that already existed (an earlier version) is not registered for removal: take it away once it is empty.
Type: dirifempty; Name: "{app}"

[Run]
Filename: "{app}\NanoArch.exe"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

[Code]
// NanoArch shows its interface with the Microsoft Edge WebView2 runtime (installed with Windows 11
// and with Microsoft Edge). If it is missing, say so instead of installing something unasked.
function WebView2Installed: Boolean;
var
  v: String;
begin
  Result :=
    (RegQueryStringValue(HKLM32, 'SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}', 'pv', v) and (v <> '') and (v <> '0.0.0.0')) or
    (RegQueryStringValue(HKCU, 'Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}', 'pv', v) and (v <> '') and (v <> '0.0.0.0'));
end;

function InitializeSetup: Boolean;
begin
  Result := True;
  if not WebView2Installed then
    if MsgBox('The Microsoft Edge WebView2 runtime was not found. NanoArch needs it to display its interface.' + #13#10 +
              'Install it from https://developer.microsoft.com/microsoft-edge/webview2/ and then run NanoArch.' + #13#10#13#10 +
              'Continue installing anyway?', mbConfirmation, MB_YESNO) = IDNO then
      Result := False;
end;
