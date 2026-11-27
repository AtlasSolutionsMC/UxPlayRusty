#define StageDir GetEnv("UXPLAY_STAGE_DIR")
#define OutputDirectory GetEnv("UXPLAY_OUTPUT_DIR")
#define AppVersion GetEnv("UXPLAY_VERSION")
#if StageDir == ""
  #error "UXPLAY_STAGE_DIR must point to the staged runtime"
#endif
#if OutputDirectory == ""
  #error "UXPLAY_OUTPUT_DIR must point to the installer output directory"
#endif
#if AppVersion == ""
  #error "UXPLAY_VERSION must match the Cargo package version"
#endif

[Setup]
AppId={{9AFA2E24-CA0D-44D7-B13C-22D3F5BA6935}
AppName=UxPlayRusty
AppVersion={#AppVersion}
AppPublisher=AtlasSolutionsMC
DefaultDirName={localappdata}\Programs\UxPlay
DefaultGroupName=UxPlay
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputDir={#OutputDirectory}
OutputBaseFilename=Setup
UninstallDisplayName=UxPlayRusty
CloseApplications=yes
CloseApplicationsFilter=uxplay.exe,uxplay-beacon.exe
RestartApplications=no
ArchitecturesInstallIn64BitMode=x64compatible
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
Uninstallable=yes
SetupLogging=yes

[Tasks]
Name: "autostart"; Description: "Start UxPlay when I sign in to Windows"; Flags: unchecked
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked

[Files]
Source: "{#StageDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\UxPlay"; Filename: "{app}\uxplay.exe"
Name: "{userdesktop}\UxPlay"; Filename: "{app}\uxplay.exe"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "UxPlay"; ValueData: """{app}\uxplay.exe"""; Tasks: autostart; Flags: uninsdeletevalue

[Run]
Filename: "{app}\uxplay.exe"; Description: "Launch UxPlay"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
Type: files; Name: "{userappdata}\UxPlay\uxplay.ble"

[Code]
function UpdateStartupSetting(ConfigFile: string; Enabled: Boolean): Boolean;
var
  Lines: TStringList;
  I: Integer;
  InStartup: Boolean;
  Found: Boolean;
  Value: string;
begin
  Result := False;
  Lines := TStringList.Create;
  try
    ForceDirectories(ExtractFileDir(ConfigFile));
    if FileExists(ConfigFile) then
      Lines.LoadFromFile(ConfigFile)
    else if not CopyFile(ExpandConstant('{app}\config.example.toml'), ConfigFile, False) then
      Exit;
    InStartup := False;
    Found := False;
    if Enabled then Value := 'true' else Value := 'false';
    for I := 0 to Lines.Count - 1 do begin
      if Trim(Lines[I]) = '[startup]' then begin
        InStartup := True;
        Continue;
      end;
      if (Length(Trim(Lines[I])) > 0) and (Copy(Trim(Lines[I]), 1, 1) = '[') then
        InStartup := False;
      if InStartup and (Copy(Trim(Lines[I]), 1, 10) = 'enabled = ') then begin
        Lines[I] := 'enabled = ' + Value;
        Found := True;
        Break;
      end;
    end;
    if not Found then begin
      if Lines.Count > 0 then Lines.Add('');
      Lines.Add('[startup]');
      Lines.Add('enabled = ' + Value);
    end;
    Lines.SaveToFile(ConfigFile);
    Result := True;
  finally
    Lines.Free;
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  ConfigFile: string;
begin
  if CurStep = ssPostInstall then begin
    ConfigFile := ExpandConstant('{userappdata}\UxPlay\config.toml');
    if not UpdateStartupSetting(ConfigFile, WizardIsTaskSelected('autostart')) then
      MsgBox('UxPlay was installed, but its startup preference could not be saved. Change startup.enabled in config.toml and restart UxPlay.', mbError, MB_OK);
  end;
end;
