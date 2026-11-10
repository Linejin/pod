; Pod installer (Inno Setup 6, per-user, no admin). Built by tools/installer/build.ps1:
;   ISCC /DAppVersion=0.1.0[-dev.<sha>] /DFileVersion=0.1.0 /DStageDir=<staged files> /DOutputDir=<dist> tools\installer\pod.iss
; Design and upgrade rules: docs/14-install.md.
;
; This file must stay UTF-8 *with BOM* (Korean strings).

#ifndef AppVersion
  #error AppVersion is required (/DAppVersion=x.y.z)
#endif
; Numeric x.y.z for the file version resource (AppVersion may carry -dev.<sha>).
#ifndef FileVersion
  #define FileVersion AppVersion
#endif
#ifndef StageDir
  #error StageDir is required (/DStageDir=...)
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif

[Setup]
AppId={{8C7541A9-1FDB-43FC-B2EB-44131622A0CF}
AppName=Pod
AppVersion={#AppVersion}
AppVerName=Pod {#AppVersion}
AppPublisher=SJin-Choi
AppComments=에이전트 코딩 IDE (Claude Code · Codex)
VersionInfoVersion={#FileVersion}
VersionInfoProductName=Pod
VersionInfoDescription=Pod 설치 프로그램
; Per-user: %LOCALAPPDATA%\Programs\Pod, HKCU only, never elevates.
PrivilegesRequired=lowest
DefaultDirName={userpf}\Pod
DisableDirPage=auto
DisableProgramGroupPage=yes
DisableWelcomePage=no
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; ConPTY needs Windows 10 1809+.
MinVersion=10.0.17763
OutputDir={#OutputDir}
OutputBaseFilename=Pod-Setup-{#AppVersion}-x64
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
SetupIconFile=..\..\assets\icon\pod.ico
WizardImageFile=..\..\assets\icon\installer-welcome.bmp
WizardSmallImageFile=..\..\assets\icon\installer-small.bmp
ChangesEnvironment=yes
; Running files are handled in [Code] (pod-app closed, podd stopped, holder files
; moved aside). The Restart Manager would offer to close pod-holder/OpenConsole,
; i.e. kill the user's agent terminals, so it stays off.
CloseApplications=no
RestartApplications=no
UninstallDisplayIcon={app}\pod-app.exe
UninstallDisplayName=Pod
UsePreviousTasks=yes
SetupLogging=yes

[Languages]
Name: "korean"; MessagesFile: "compiler:Languages\Korean.isl"

[Tasks]
Name: "addtopath"; Description: "명령줄에서 pod 명령을 쓸 수 있게 PATH에 추가 (현재 사용자)"; GroupDescription: "명령줄:"
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
; Executables and the ConPTY DLL can be in use (pod-holder, OpenConsole, pod attach):
; MoveAsideIfLocked renames a locked file to <name>.old-<stamp> first.
Source: "{#StageDir}\*.exe"; DestDir: "{app}"; Flags: ignoreversion; BeforeInstall: MoveAsideIfLocked
Source: "{#StageDir}\*.dll"; DestDir: "{app}"; Flags: ignoreversion; BeforeInstall: MoveAsideIfLocked
Source: "{#StageDir}\x64\*"; DestDir: "{app}\x64"; Flags: ignoreversion; BeforeInstall: MoveAsideIfLocked
Source: "{#StageDir}\*.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\licenses\*"; DestDir: "{app}\licenses"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\Pod"; Filename: "{app}\pod-app.exe"; WorkingDir: "{%USERPROFILE}"; IconFilename: "{app}\pod-app.exe"; IconIndex: 0; Comment: "Pod 에이전트 IDE"; AppUserModelID: "Pod.App"
Name: "{userdesktop}\Pod"; Filename: "{app}\pod-app.exe"; WorkingDir: "{%USERPROFILE}"; IconFilename: "{app}\pod-app.exe"; IconIndex: 0; Comment: "Pod 에이전트 IDE"; AppUserModelID: "Pod.App"; Tasks: desktopicon

[Registry]
; AppUserModelID for Windows toasts (mod notices, docs/16 §8.2). Same id as the
; shortcuts above and pod-app's process id (os_notify.rs). Without this key the
; app keeps to in-app notices.
Root: HKCU; Subkey: "Software\Classes\AppUserModelId\Pod.App"; ValueType: string; ValueName: "DisplayName"; ValueData: "Pod"; Flags: uninsdeletekey

[Run]
; Offered (unchecked) only when `pod hooks status` says the hooks are missing or
; point to a different pod-hook build. It edits the user's agent configuration,
; so it is never run silently.
Filename: "{cmd}"; Parameters: "/C """"{app}\pod.exe"" hooks install & echo. & pause"""; Description: "에이전트 상태 훅 설치·업데이트"; Flags: postinstall unchecked skipifsilent; Check: HooksNeedInstall
Filename: "{app}\pod-app.exe"; WorkingDir: "{%USERPROFILE}"; Description: "Pod 실행"; Flags: postinstall nowait skipifsilent

[UninstallDelete]
Type: files; Name: "{app}\*.old-*"
Type: files; Name: "{app}\x64\*.old-*"
Type: dirifempty; Name: "{app}\x64"
Type: dirifempty; Name: "{app}\licenses"
Type: dirifempty; Name: "{app}"

[Code]
type
  TPidArray = array of Integer;

var
  HooksNeeded: Boolean;
  HooksReason: String;

{ ---------- helpers ---------- }

function HasParam(const Name: String): Boolean;
var
  I: Integer;
begin
  Result := False;
  for I := 1 to ParamCount do
    if CompareText(ParamStr(I), Name) = 0 then
      Result := True;
end;

function IsSilentRun(): Boolean;
begin
  if IsUninstaller then
    Result := UninstallSilent
  else
    Result := WizardSilent;
end;

{ Message box that returns Default without showing anything in silent runs. }
function Ask(const Text: String; Typ: TMsgBoxType; Buttons, Default: Integer): Integer;
begin
  if IsSilentRun() then begin
    Log('silent: "' + Text + '" -> ' + IntToStr(Default));
    Result := Default;
  end else
    Result := MsgBox(Text, Typ, Buttons);
end;

{ Processes named ExeName whose image lives under Dir (WMI). }
function FindAppProcesses(const ExeName, Dir: String; var Pids: TPidArray): Integer;
var
  Locator, Svc, Items, Item: Variant;
  I: Integer;
  P, Prefix: String;
begin
  Result := 0;
  SetArrayLength(Pids, 0);
  Prefix := Lowercase(AddBackslash(Dir));
  try
    Locator := CreateOleObject('WbemScripting.SWbemLocator');
    Svc := Locator.ConnectServer('.', 'root\CIMV2');
    Items := Svc.ExecQuery('SELECT ProcessId, ExecutablePath FROM Win32_Process WHERE Name = ''' + ExeName + '''');
    for I := 0 to Items.Count - 1 do begin
      Item := Items.ItemIndex(I);
      if not (VarIsNull(Item.ExecutablePath) or VarIsEmpty(Item.ExecutablePath)) then begin
        P := Item.ExecutablePath;
        P := Lowercase(P);
        if Pos(Prefix, P) = 1 then begin
          SetArrayLength(Pids, Result + 1);
          Pids[Result] := Item.ProcessId;
          Result := Result + 1;
        end;
      end;
    end;
  except
    Log('WMI process query failed: ' + GetExceptionMessage);
  end;
end;

function CountAppProcesses(const ExeName, Dir: String): Integer;
var
  Pids: TPidArray;
begin
  Result := FindAppProcesses(ExeName, Dir, Pids);
end;

function WaitGone(const ExeName, Dir: String; Seconds: Integer): Boolean;
var
  I: Integer;
begin
  for I := 1 to Seconds * 4 do begin
    if CountAppProcesses(ExeName, Dir) = 0 then begin
      Result := True;
      exit;
    end;
    Sleep(250);
  end;
  Result := CountAppProcesses(ExeName, Dir) = 0;
end;

procedure TaskKill(const ExeName, Dir: String; Force: Boolean);
var
  Pids: TPidArray;
  I, N, Code: Integer;
  Args: String;
begin
  N := FindAppProcesses(ExeName, Dir, Pids);
  for I := 0 to N - 1 do begin
    Args := '/PID ' + IntToStr(Pids[I]);
    if Force then
      Args := '/F ' + Args;
    Exec(ExpandConstant('{sys}\taskkill.exe'), Args, '', SW_HIDE, ewWaitUntilTerminated, Code);
    Log('taskkill ' + Args + ' (' + ExeName + ') -> ' + IntToStr(Code));
  end;
end;

{ Runs <Dir>\pod.exe Args with output captured to OutFile. True on exit code 0. }
function RunPod(const Dir, Args, OutFile: String): Boolean;
var
  Code: Integer;
  Exe: String;
begin
  Result := False;
  Exe := AddBackslash(Dir) + 'pod.exe';
  if not FileExists(Exe) then begin
    Log('no ' + Exe);
    exit;
  end;
  if Exec(ExpandConstant('{cmd}'), '/C ""' + Exe + '" ' + Args + ' > "' + OutFile + '" 2>&1"',
          Dir, SW_HIDE, ewWaitUntilTerminated, Code) then begin
    Log('pod ' + Args + ' -> exit ' + IntToStr(Code));
    Result := Code = 0;
  end else
    Log('could not run pod ' + Args);
end;

function RunPodText(const Dir, Args: String; var Text: String): Boolean;
var
  OutFile: String;
  S: AnsiString;
begin
  Text := '';
  OutFile := GenerateUniqueName(GetTempDir, '.txt');
  Result := RunPod(Dir, Args, OutFile);
  if LoadStringFromFile(OutFile, S) then
    Text := String(S);
  DeleteFile(OutFile);
  Log('pod ' + Args + ' output:'#13#10 + Text);
end;

{ Closes pod-app windows of this install (WM_CLOSE), then forces after 5 s. }
procedure CloseApp(const Dir: String);
begin
  if CountAppProcesses('pod-app.exe', Dir) = 0 then
    exit;
  TaskKill('pod-app.exe', Dir, False);
  if not WaitGone('pod-app.exe', Dir, 5) then
    TaskKill('pod-app.exe', Dir, True);
  WaitGone('pod-app.exe', Dir, 3);
end;

{ `pod server stop` (holders keep running), then force if needed. Killing podd
  is also safe for terminals: holders survive and a new podd re-attaches. }
procedure StopDaemon(const Dir: String);
var
  T: String;
begin
  if CountAppProcesses('podd.exe', Dir) = 0 then
    exit;
  RunPodText(Dir, 'server stop', T);
  if not WaitGone('podd.exe', Dir, 8) then begin
    Log('podd still running after server stop; terminating it');
    TaskKill('podd.exe', Dir, True);
    WaitGone('podd.exe', Dir, 3);
  end;
end;

{ Ends every terminal of this install: `pod server stop --kill` through the
  daemon (starting it with `pod ls` if needed, so it re-attaches the holders),
  then force-kills holders that are still there (Job Objects end their trees). }
procedure StopTerminals(const Dir: String);
var
  T: String;
begin
  if not RunPodText(Dir, 'server stop --kill', T) then begin
    RunPodText(Dir, 'ls', T);
    RunPodText(Dir, 'server stop --kill', T);
  end;
  if not WaitGone('pod-holder.exe', Dir, 10) then begin
    Log('holders still running; terminating them');
    TaskKill('pod-holder.exe', Dir, True);
    WaitGone('pod-holder.exe', Dir, 5);
  end;
  TaskKill('OpenConsole.exe', AddBackslash(Dir) + 'x64', True);
  WaitGone('podd.exe', Dir, 5);
end;

{ Removes <Dir>\*.old-* files that are no longer in use. }
procedure DeleteAsideFiles(const Dir: String);
var
  R: TFindRec;
begin
  if FindFirst(AddBackslash(Dir) + '*.old-*', R) then begin
    try
      repeat
        if DeleteFile(AddBackslash(Dir) + R.Name) then
          Log('deleted ' + R.Name)
        else
          Log('still in use: ' + R.Name);
      until not FindNext(R);
    finally
      FindClose(R);
    end;
  end;
end;

{ ---------- PATH (HKCU\Environment) ---------- }

function SamePath(A, B: String): Boolean;
begin
  A := RemoveBackslashUnlessRoot(Trim(A));
  B := RemoveBackslashUnlessRoot(Trim(B));
  Result := CompareText(A, B) = 0;
end;

function PathEntries(const Paths: String; var Parts: TArrayOfString): Integer;
var
  S: String;
  P: Integer;
begin
  Result := 0;
  SetArrayLength(Parts, 0);
  S := Paths;
  while S <> '' do begin
    P := Pos(';', S);
    if P = 0 then P := Length(S) + 1;
    SetArrayLength(Parts, Result + 1);
    Parts[Result] := Copy(S, 1, P - 1);
    Result := Result + 1;
    Delete(S, 1, P);
  end;
end;

procedure AddToPath(const Dir: String);
var
  Paths: String;
  Parts: TArrayOfString;
  I, N: Integer;
begin
  if not RegQueryStringValue(HKCU, 'Environment', 'Path', Paths) then
    Paths := '';
  N := PathEntries(Paths, Parts);
  for I := 0 to N - 1 do
    if SamePath(Parts[I], Dir) then
      exit;
  { keep the user's trailing-';' style so RemoveFromPath restores the exact value }
  if Paths = '' then
    Paths := Dir
  else if Paths[Length(Paths)] = ';' then
    Paths := Paths + Dir + ';'
  else
    Paths := Paths + ';' + Dir;
  if RegWriteExpandStringValue(HKCU, 'Environment', 'Path', Paths) then
    Log('PATH += ' + Dir);
end;

procedure RemoveFromPath(const Dir: String);
var
  Paths, NewPaths: String;
  Parts: TArrayOfString;
  I, N: Integer;
  Changed: Boolean;
begin
  if not RegQueryStringValue(HKCU, 'Environment', 'Path', Paths) then
    exit;
  N := PathEntries(Paths, Parts);
  NewPaths := '';
  Changed := False;
  for I := 0 to N - 1 do begin
    if SamePath(Parts[I], Dir) then
      Changed := True
    else begin
      if NewPaths <> '' then NewPaths := NewPaths + ';';
      NewPaths := NewPaths + Parts[I];
    end;
  end;
  if Changed then begin
    { keep a trailing ';' if the user had one }
    if (Paths <> '') and (Paths[Length(Paths)] = ';') then
      NewPaths := NewPaths + ';';
    if RegWriteExpandStringValue(HKCU, 'Environment', 'Path', NewPaths) then
      Log('PATH -= ' + Dir);
  end;
end;

{ ---------- install / upgrade ---------- }

procedure MoveAsideIfLocked();
var
  F, Aside: String;
begin
  F := ExpandConstant(CurrentFileName);
  if FileExists(F) and not DeleteFile(F) then begin
    Aside := F + '.old-' + GetDateTimeString('yyyymmddhhnnss', #0, #0);
    if RenameFile(F, Aside) then
      Log('in use, moved aside: ' + F + ' -> ' + Aside)
    else
      Log('in use and could not be moved aside: ' + F);
  end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Dir: String;
  Holders: Integer;
begin
  Result := '';
  Dir := ExpandConstant('{app}');
  Holders := CountAppProcesses('pod-holder.exe', Dir);

  if CountAppProcesses('pod-app.exe', Dir) > 0 then begin
    if Ask('Pod 앱이 실행 중입니다. 업데이트하려면 앱을 닫아야 합니다.' + #13#10#13#10 +
           '터미널과 에이전트는 꺼지지 않습니다. 앱은 화면일 뿐이고 터미널은 데몬과 holder가 따로 들고 있습니다.' + #13#10#13#10 +
           '앱을 닫고 계속할까요?', mbConfirmation, MB_YESNO, IDYES) <> IDYES then begin
      Result := 'Pod 앱이 실행 중이라 설치를 멈췄습니다. 앱을 닫은 뒤 다시 실행하세요.';
      exit;
    end;
    CloseApp(Dir);
  end;

  if CountAppProcesses('podd.exe', Dir) > 0 then begin
    if Ask('Pod 데몬(podd)이 실행 중입니다. 새 버전으로 바꾸려면 데몬을 잠시 멈춰야 합니다.' + #13#10#13#10 +
           '"pod server stop"으로 데몬만 멈춥니다. 실행 중인 터미널 ' + IntToStr(Holders) +
           '개와 그 안의 에이전트는 계속 돌고, 다음에 Pod를 열면 새 데몬이 다시 연결합니다.' + #13#10#13#10 +
           '데몬을 멈추고 계속할까요?', mbConfirmation, MB_YESNO, IDYES) <> IDYES then begin
      Result := 'Pod 데몬이 실행 중이라 설치를 멈췄습니다.';
      exit;
    end;
    StopDaemon(Dir);
  end;

  if Holders > 0 then
    Ask('실행 중인 터미널 ' + IntToStr(Holders) + '개는 그대로 유지됩니다.' + #13#10#13#10 +
        '이 터미널들은 닫을 때까지 이전 버전의 pod-holder로 돌아갑니다. 사용 중인 이전 파일은 ' +
        '"<이름>.old-<시각>"으로 옮겨 두고, 다음 설치나 제거 때 지웁니다.', mbInformation, MB_OK, IDOK);
end;

procedure CheckHooks();
var
  T, L: String;
begin
  HooksNeeded := False;
  HooksReason := '';
  if not RunPodText(ExpandConstant('{app}'), 'hooks status', T) then
    exit;
  L := Lowercase(T);
  if Pos('differs from this build', L) > 0 then
    HooksReason := '설치된 pod-hook이 이 버전과 다릅니다'
  else if (Pos('(missing', L) > 0) or (Pos(' 0 pod event', L) > 0) then
    HooksReason := '에이전트 훅이 없거나 일부만 설치되어 있습니다';
  HooksNeeded := HooksReason <> '';
end;

function HooksNeedInstall(): Boolean;
begin
  Result := HooksNeeded;
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  Dir: String;
begin
  if CurStep = ssPostInstall then begin
    Dir := ExpandConstant('{app}');
    DeleteAsideFiles(Dir);
    DeleteAsideFiles(AddBackslash(Dir) + 'x64');
    if WizardIsTaskSelected('addtopath') then
      AddToPath(Dir)
    else
      RemoveFromPath(Dir);
    CheckHooks();
  end;
end;

procedure CurPageChanged(CurPageID: Integer);
var
  Delta: Integer;
begin
  if (CurPageID = wpFinished) and HooksNeeded then begin
    Log('hooks offered on the finished page: ' + HooksReason);
    WizardForm.FinishedLabel.Caption := WizardForm.FinishedLabel.Caption + #13#10#13#10 +
      HooksReason + '. 아래 항목을 고르면 pod hooks install이 ~/.claude, ~/.codex 설정을 백업한 뒤 Pod 훅을 넣습니다.';
    Delta := WizardForm.AdjustLabelHeight(WizardForm.FinishedLabel);
    WizardForm.RunList.Top := WizardForm.RunList.Top + Delta;
    WizardForm.RunList.Height := WizardForm.RunList.Height - Delta;
  end;
end;

{ ---------- uninstall ---------- }

var
  HooksRemoved: Boolean;

function InitializeUninstall(): Boolean;
var
  Dir: String;
  Holders: Integer;
begin
  Result := True;
  Dir := ExpandConstant('{app}');
  Holders := CountAppProcesses('pod-holder.exe', Dir);
  if Holders > 0 then begin
    if HasParam('/STOPTERMINALS') or
       (Ask('실행 중인 Pod 터미널이 ' + IntToStr(Holders) + '개 있습니다(그 안의 에이전트 포함).' + #13#10#13#10 +
            'Pod를 제거하려면 이 터미널을 모두 종료해야 합니다. 종료하면 저장하지 않은 작업이 사라질 수 있습니다.' + #13#10#13#10 +
            '터미널을 모두 종료하고 제거할까요? ("아니요"를 누르면 제거를 취소합니다)',
            mbConfirmation, MB_YESNO or MB_DEFBUTTON2, IDNO) = IDYES) then
      StopTerminals(Dir)
    else begin
      Log('uninstall cancelled: ' + IntToStr(Holders) + ' terminal(s) running');
      Result := False;
      exit;
    end;
  end;
  CloseApp(Dir);
  StopDaemon(Dir);
end;

function DataDir(): String;
begin
  Result := AddBackslash(GetEnv('LOCALAPPDATA')) + 'pod';
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Dir, T: String;
  Purge: Boolean;
begin
  Dir := ExpandConstant('{app}');
  if CurUninstallStep = usUninstall then begin
    RemoveFromPath(Dir);
    { hooks: pod.exe is still there at this point }
    if RunPodText(Dir, 'hooks status', T) and (Pos('  runs ', Lowercase(T)) > 0) then begin
      if Ask('Claude Code·Codex 전역 설정(~/.claude/settings.json, ~/.codex/hooks.json)에 Pod 훅이 있습니다.' + #13#10#13#10 +
             '훅도 제거할까요? ("pod hooks uninstall": 파일을 백업한 뒤 Pod 항목만 지웁니다)' + #13#10#13#10 +
             '"아니요"를 고르면 훅과 ' + DataDir() + '\bin\pod-hook.exe가 남습니다. 이 훅은 Pod 밖에서는 아무 일도 하지 않습니다.',
             mbConfirmation, MB_YESNO or MB_DEFBUTTON2, IDNO) = IDYES then begin
        HooksRemoved := RunPodText(Dir, 'hooks uninstall', T);
        if not HooksRemoved then
          Ask('훅 제거에 실패했습니다:' + #13#10#13#10 + T, mbError, MB_OK, IDOK);
      end;
    end;
  end;

  if CurUninstallStep = usPostUninstall then begin
    if HooksRemoved then
      DelTree(DataDir() + '\bin', True, True, True);
    Purge := HasParam('/PURGEDATA');
    if not Purge then
      Purge := Ask('Pod 사용자 데이터도 지울까요?' + #13#10#13#10 +
                   '  - 터미널 기록·상태: ' + DataDir() + '\state' + #13#10 +
                   '  - 데몬 로그: ' + DataDir() + '\podd.log' + #13#10 +
                   '  - 앱 설정·레이아웃: ' + AddBackslash(GetEnv('APPDATA')) + 'pod' + #13#10#13#10 +
                   '작업 worktree(%USERPROFILE%\pod\worktrees)와 에이전트 훅 파일은 지우지 않습니다.' + #13#10 +
                   '"아니요"를 고르면 다시 설치했을 때 그대로 이어서 씁니다.',
                   mbConfirmation, MB_YESNO or MB_DEFBUTTON2, IDNO) = IDYES;
    if Purge then begin
      DelTree(DataDir() + '\state', True, True, True);
      DeleteFile(DataDir() + '\podd.log');
      DelTree(AddBackslash(GetEnv('APPDATA')) + 'pod', True, True, True);
      Log('user data removed');
    end;
    RemoveDir(DataDir());
  end;
end;
