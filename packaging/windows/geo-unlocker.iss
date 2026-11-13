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

[Icons]
Name: "{autoprograms}\Geo Unlocker"; Filename: "{app}\geo-unlocker.exe"
Name: "{autodesktop}\Geo Unlocker"; Filename: "{app}\geo-unlocker.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\geo-unlocker.exe"; Description: "Start Geo Unlocker"; Flags: nowait postinstall skipifsilent shellexec
; Automatic updates run the installer silently with /relaunch=1: start the new version afterwards.
Filename: "{app}\geo-unlocker.exe"; Flags: nowait shellexec; Check: ShouldRelaunch

[UninstallRun]
; Undo any network changes left by a crash (routes, DNS rule, proxy) before removing files.
Filename: "{app}\geo-unlocker.exe"; Parameters: "--cleanup"; Flags: runhidden waituntilterminated; RunOnceId: "GeoUnlockerCleanup"

[UninstallDelete]
Type: filesandordirs; Name: "{commonappdata}\GeoUnlocker"
; The app window's preferences (selected tab, home location).
Type: filesandordirs; Name: "{localappdata}\GeoUnlocker"

[Code]
function ShouldRelaunch(): Boolean;
begin
  Result := ExpandConstant('{param:relaunch|0}') = '1';
end;
