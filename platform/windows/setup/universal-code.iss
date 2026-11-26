// Included inside [Code]. No registration or application shutdown before preflight.
var
  UniversalBackupReady, UniversalCommitted, UniversalFailed, UniversalRestored: Boolean;
  UniversalNames: TArrayOfString;
  UniversalExisted: array of Boolean;
  UniversalBackupDir, UniversalTargetDir, UniversalSourceDir, UniversalFailureMessage: String;

function IsArm64System(): Boolean;
begin
  Result := ProcessorArchitecture = paARM64;
end;

function IsX64System(): Boolean;
begin
  Result := not IsArm64System();
end;

function IsUninstallerFile(const Name: String): Boolean;
var
  Base, Ext: String;
  I: Integer;
begin
  Ext := Lowercase(ExtractFileExt(Name));
  Base := Copy(Name, 1, Length(Name) - Length(Ext));
  Result := (Copy(Lowercase(Base), 1, 5) = 'unins') and (Length(Base) > 5) and
    ((Ext = '.exe') or (Ext = '.dat') or (Ext = '.msg'));
  if Result then
    for I := 6 to Length(Base) do
      if (Base[I] < '0') or (Base[I] > '9') then Result := False;
end;

function UniversalKnowsFile(const Name: String): Boolean;
var
  I: Integer;
begin
  Result := False;
  for I := 0 to GetArrayLength(UniversalNames) - 1 do
    if CompareText(Name, UniversalNames[I]) = 0 then Result := True;
end;

// Files explicitly retired by [InstallDelete]/Prepare still belong to rollback.
procedure UniversalRememberRetiredFile(const Name: String);
var
  N: Integer;
begin
  if FileExists(UniversalTargetDir + '\' + Name) and (not UniversalKnowsFile(Name)) then begin
    N := GetArrayLength(UniversalNames);
    SetArrayLength(UniversalNames, N + 1);
    UniversalNames[N] := Name;
  end;
end;

function UniversalPrepare(): String;
var
  Arch, SourceDir, BackupFile, Target: String;
  Code, I, N: Integer;
  SR: TFindRec;
begin
  Result := '';
  if UniversalBackupReady then begin
    if CompareText(UniversalTargetDir, WizardDirValue) <> 0 then
      Result := '旧程序已备份。若要更改安装目录，请退出后重新打开安装器。';
    exit;
  end;
  UniversalTargetDir := WizardDirValue;
  if IsArm64System() then Arch := 'arm64' else Arch := 'x64';
  ExtractTemporaryFiles('{tmp}\hufu-preflight\' + Arch + '\*');
  ExtractTemporaryFile('program-files.lst');
  SourceDir := ExpandConstant('{tmp}\hufu-preflight\') + Arch;
  UniversalSourceDir := SourceDir;
  Code := -1;
  if not Exec(SourceDir + '\hufu-setup-engine.exe',
      'preflight --src "' + SourceDir + '" --log "' + ExpandConstant('{tmp}\hufu-preflight.log') + '"',
      SourceDir, SW_HIDE, ewWaitUntilTerminated, Code) then Code := -1;
  if Code <> 0 then begin
    Result := '安装包校验或 TSF 加载检查失败，现有安装尚未更改。' + #13#10 +
      ExpandConstant('{tmp}\hufu-preflight.log');
    exit;
  end;
  if not LoadStringsFromFile(ExpandConstant('{tmp}\program-files.lst'), UniversalNames) then begin
    Result := '无法读取程序文件清单。';
    exit;
  end;
  UniversalRememberRetiredFile('install.ps1');
  UniversalRememberRetiredFile('uninstall.ps1');
  UniversalRememberRetiredFile('hufu-setup-tools.ps1');
  UniversalRememberRetiredFile('hufu-postboot.ps1');
  UniversalRememberRetiredFile('卸载.bat');
  UniversalRememberRetiredFile('设置.bat');
  UniversalRememberRetiredFile('安装.bat');
  UniversalBackupDir := UniversalTargetDir + '\安装备份\program-' +
    GetDateTimeString('yyyymmdd-hhnnss-zzz', '/', ':');
  // Inno writes these files and its ARP entry before ssPostInstall.
  if FindFirst(UniversalTargetDir + '\unins*', SR) then begin
    try
      repeat
        if IsUninstallerFile(SR.Name) and (not UniversalKnowsFile(SR.Name)) then begin
          N := GetArrayLength(UniversalNames);
          SetArrayLength(UniversalNames, N + 1);
          UniversalNames[N] := SR.Name;
        end;
      until not FindNext(SR);
    finally
      FindClose(SR);
    end;
  end;
  if not ForceDirectories(UniversalBackupDir) then begin
    Result := '无法创建安装备份目录。';
    exit;
  end;
  Code := -1;
  if not Exec(SourceDir + '\hufu-setup-engine.exe',
      'snapshot-inno --snapshot "' + UniversalBackupDir + '\inno-registry.json"',
      SourceDir, SW_HIDE, ewWaitUntilTerminated, Code) then Code := -1;
  if Code <> 0 then begin
    Result := '无法备份本产品卸载登记，已停止安装。';
    exit;
  end;
  SetArrayLength(UniversalExisted, GetArrayLength(UniversalNames));
  for I := 0 to GetArrayLength(UniversalNames) - 1 do begin
    Target := UniversalTargetDir + '\' + UniversalNames[I];
    UniversalExisted[I] := FileExists(Target);
    if UniversalExisted[I] then begin
      BackupFile := UniversalBackupDir + '\' + UniversalNames[I];
      if (not ForceDirectories(ExtractFileDir(BackupFile))) or
         (not CopyFile(Target, BackupFile, False)) then begin
        Result := '无法备份旧程序，已停止安装：' + Target;
        exit;
      end;
    end;
  end;
  UniversalBackupReady := True;
end;

procedure UniversalRestorePrograms();
var
  I, Code: Integer;
  SR: TFindRec;
  Target, BackupFile, FailedName: String;
  Restored: Boolean;
begin
  if (not UniversalBackupReady) or UniversalCommitted or UniversalRestored then exit;
  Restored := True;
  for I := 0 to GetArrayLength(UniversalNames) - 1 do begin
    Target := UniversalTargetDir + '\' + UniversalNames[I];
    BackupFile := UniversalBackupDir + '\' + UniversalNames[I];
    if FileExists(Target) and (not DeleteFile(Target)) then begin
      FailedName := Target + '.failed-' + GetDateTimeString('yyyymmddhhnnsszzz', '/', ':');
      if not RenameFile(Target, FailedName) then begin
        Restored := False;
        Log('程序回退无法腾位：' + Target);
        continue;
      end;
    end;
    if UniversalExisted[I] then begin
      ForceDirectories(ExtractFileDir(Target));
      if not CopyFile(BackupFile, Target, False) then begin
        Restored := False;
        Log('程序回退失败，备份保留：' + BackupFile);
      end;
    end;
  end;
  if FindFirst(UniversalTargetDir + '\unins*', SR) then begin
    try
      repeat
        if IsUninstallerFile(SR.Name) and (not UniversalKnowsFile(SR.Name)) then
          if not DeleteFile(UniversalTargetDir + '\' + SR.Name) then Restored := False;
      until not FindNext(SR);
    finally
      FindClose(SR);
    end;
  end;
  Code := -1;
  if not Exec(UniversalSourceDir + '\hufu-setup-engine.exe',
      'restore-inno --snapshot "' + UniversalBackupDir + '\inno-registry.json"',
      UniversalSourceDir, SW_HIDE, ewWaitUntilTerminated, Code) then Code := -1;
  if Code <> 0 then begin
    Restored := False;
    Log('本产品卸载登记恢复失败，快照保留：' + UniversalBackupDir);
  end;
  UniversalRestored := Restored;
  if Restored then Log('已恢复本次覆盖的程序文件；系统注册恢复结果见 setup-engine.log。')
  else Log('部分程序尚未恢复，保留备份：' + UniversalBackupDir);
  // Backups are retained on failure, including when a second UAC rollback is denied.
end;

procedure DeinitializeSetup();
begin
  UniversalRestorePrograms();
end;

function GetCustomSetupExitCode(): Integer;
begin
  if UniversalFailed then Result := 1 else Result := 0;
end;
