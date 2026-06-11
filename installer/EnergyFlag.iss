; Inno Setup script for EnergyFlag.
; Builds a single per-user EnergyFlag-Setup.exe that needs no admin rights.
; Compile with:  ISCC.exe installer\EnergyFlag.iss
; The EnergyFlag.exe must already be built at rs\target\release\EnergyFlag.exe
; (override with /DBuildDir=... and /DAppVersion=... on the ISCC command line).

#ifndef AppVersion
  #define AppVersion "0.3.0"
#endif
#ifndef BuildDir
  #define BuildDir "..\rs\target\release"
#endif

[Setup]
AppId={{EEFCF2D5-DACB-48EC-9C67-3526C2054DC4}
AppName=EnergyFlag
AppVersion={#AppVersion}
AppPublisher=gabrielchaves6
AppPublisherURL=https://github.com/gabrielchaves6/energyflag
DefaultDirName={autopf}\EnergyFlag
DefaultGroupName=EnergyFlag
DisableProgramGroupPage=yes
DisableDirPage=auto
; Per-user install: no administrator prompt.
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=dist
OutputBaseFilename=EnergyFlag-Setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayName=EnergyFlag
UninstallDisplayIcon={app}\EnergyFlag.exe
SetupIconFile=..\rs\assets\energyflag.ico
SetupLogging=yes

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "brazilianportuguese"; MessagesFile: "compiler:Languages\BrazilianPortuguese.isl"

[Tasks]
Name: "startup"; Description: "{cm:StartAtLogon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "{#BuildDir}\EnergyFlag.exe"; DestDir: "{app}"; Flags: ignoreversion
; Shipped next to the exe as a runtime icon fallback (and a stable icon for shortcuts).
Source: "..\rs\assets\energyflag.ico"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\EnergyFlag"; Filename: "{app}\EnergyFlag.exe"; IconFilename: "{app}\energyflag.ico"
Name: "{userstartup}\EnergyFlag"; Filename: "{app}\EnergyFlag.exe"; IconFilename: "{app}\energyflag.ico"; Tasks: startup

[Run]
; No skipifsilent: the in-app updater runs Setup with /VERYSILENT and relies on this
; entry to relaunch EnergyFlag once the new exe is in place (one-click hands-off update).
Filename: "{app}\EnergyFlag.exe"; Description: "{cm:LaunchProgram,EnergyFlag}"; Flags: nowait postinstall

[CustomMessages]
en.StartAtLogon=Start EnergyFlag when I sign in to Windows
brazilianportuguese.StartAtLogon=Iniciar o EnergyFlag ao entrar no Windows

[Code]
// A running EnergyFlag.exe locks its own file. Kill any instance before
// installing over it and before uninstalling.
procedure KillRunning;
var
  ResultCode: Integer;
begin
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/IM EnergyFlag.exe /F',
       '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssInstall then
    KillRunning;
end;

function InitializeUninstall(): Boolean;
begin
  KillRunning;
  Result := True;
end;
