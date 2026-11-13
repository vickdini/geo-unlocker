; Geo Unlocker — Windows installer (Inno Setup 6).
;   python packaging\fetch_tor.py windows-x86_64 dist\windows\tor
;   cargo build --release -p geo-unlocker
;   iscc packaging\windows\geo-unlocker.iss
; Output: dist\GeoUnlockerSetup-<version>-x64.exe

#ifndef AppVersion
  #define AppVersion "0.2.0"
#endif
#define Root "..\.."

[Setup]
AppId={{6F2B8E1C-4D7A-4B9E-9C3F-5A1D2E7B8C40}
AppName=Geo Unlocker
AppVersion={#AppVersion}
AppVerName=Geo Unlocker {#AppVersion}
AppPublisher=Geo Unlocker
DefaultDirName={autopf}\Geo Unlocker
DefaultGroupName=Geo Unlocker
DisableProgramGroupPage=yes
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir={#Root}\dist
OutputBaseFilename=GeoUnlockerSetup-{#AppVersion}-x64
SetupIconFile={#Root}\packaging\icons\geo-unlocker.ico
UninstallDisplayIcon={app}\geo-unlocker.exe
UninstallDisplayName=Geo Unlocker
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
CloseApplications=force
RestartApplications=no

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "{#Root}\target\release\geo-unlocker.exe"; DestDir: "{app}"; Flags: ignoreversion
; Tor Expert Bundle (unmodified, minus pluggable transports): tor\tor\tor.exe, tor\data\geoip, tor\docs
Source: "{#Root}\dist\windows\tor\*"; DestDir: "{app}\tor"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "{#Root}\crates\desktop\static\NOTICE.md"; DestDir: "{app}\licenses"; Flags: ignoreversion
Source: "{#Root}\crates\desktop\assets\wintun\LICENSE.txt"; DestDir: "{app}\licenses"; DestName: "wintun-LICENSE.txt"; Flags: ignoreversion
; Only used on the rare Windows 10 PCs without the WebView2 runtime (Windows 11 always has it).
Source: "{#Root}\dist\windows\MicrosoftEdgeWebview2Setup.exe"; DestDir: "{tmp}"; Flags: deleteafterinstall skipifsourcedoesntexist; Check: NeedsWebView2

[Icons]
Name: "{autoprograms}\Geo Unlocker"; Filename: "{app}\geo-unlocker.exe"
Name: "{autodesktop}\Geo Unlocker"; Filename: "{app}\geo-unlocker.exe"; Tasks: desktopicon

[Run]
Filename: "{tmp}\MicrosoftEdgeWebview2Setup.exe"; Parameters: "/silent /install"; StatusMsg: "Installing Microsoft Edge WebView2..."; Flags: waituntilterminated skipifdoesntexist; Check: NeedsWebView2
Filename: "{app}\geo-unlocker.exe"; Description: "Start Geo Unlocker"; Flags: nowait postinstall skipifsilent shellexec
; Automatic updates run the installer silently with /relaunch=1: start the new version afterwards.
Filename: "{app}\geo-unlocker.exe"; Flags: nowait shellexec; Check: ShouldRelaunch

[UninstallRun]
; Undo any network changes left by a crash (routes, DNS rule, proxy) before removing files.
Filename: "{app}\geo-unlocker.exe"; Parameters: "--cleanup"; Flags: runhidden waituntilterminated; RunOnceId: "GeoUnlockerCleanup"

[UninstallDelete]
Type: filesandordirs; Name: "{commonappdata}\GeoUnlocker"

[Code]
function WebView2Version(Root: Integer; Key: String): String;
begin
  if not RegQueryStringValue(Root, Key, 'pv', Result) then Result := '';
end;

function ShouldRelaunch(): Boolean;
begin
  Result := ExpandConstant('{param:relaunch|0}') = '1';
end;

function NeedsWebView2(): Boolean;
var v: String;
begin
  v := WebView2Version(HKLM, 'SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}');
  if (v = '') or (v = '0.0.0.0') then
    v := WebView2Version(HKCU, 'Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}');
  Result := (v = '') or (v = '0.0.0.0');
end;
