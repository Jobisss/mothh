[Setup]
AppId={{C5E3BEE3-4036-4A6E-96FC-40B61FBC2DCF}
AppName=mothh
AppVersion=0.1.0
AppPublisher=mothh
DefaultDirName={localappdata}\Programs\mothh
DefaultGroupName=mothh
PrivilegesRequired=lowest
OutputDir=..\dist
OutputBaseFilename=mothh-setup-windows-x64
Compression=lzma
SolidCompression=yes
ArchitecturesAllowed=x64
ArchitecturesInstallIn64BitMode=x64
SetupIconFile=mothh.ico
UninstallDisplayIcon={app}\mothh.ico

[Files]
Source: "..\target\release\mothh.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "mothh.ico"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\mothh"; Filename: "{app}\mothh.exe"; IconFilename: "{app}\mothh.ico"

[Run]
Filename: "{app}\mothh.exe"; Description: "Abrir mothh"; Flags: nowait postinstall skipifsilent
