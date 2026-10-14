; ============================================================================
; HuFu 虎符输入法 · 单 exe 安装包（Inno Setup 6）
; ----------------------------------------------------------------------------
; 架构铁律（与 install.ps1 双阶段一一对齐，勿"标准化"改动）：
;   1. PrivilegesRequired=lowest——全程普通权限；HKCU/文件/server 永远在
;      非提权上下文；HKLM+SystemIME+msctf 交给复用的 install.ps1 内部
;      提权子进程（一次 UAC）。server 绝不由提权进程直接启动。
;   2. 升级 = 无痕覆盖：绝不注销 TIP、不动语言列表、跳过 ctfmon 刷新
;      （install.ps1 L41-46 升级语义，虎爪保护）。
;   3. 数据安全红线：永不目录级删除数据区；程序文件走 [Files]（进卸载
;      日志）；数据\ 经 -BackupDiff 只补缺（缺失才拷，已存在一律不覆盖；不进
;      卸载日志），码表\ 覆盖安装时整目录零写入（不覆盖/不补缺/不复活已删），
;      全新安装才全量落盘——双清单 hufu-files.lst / hufu-files-upgrade.lst）
;      卸载天然保留）；模型\码表导出\用户调整.txt 不在包中，永不触碰。
;   4. 被占用 PE 一律"改名腾位 .oldN"（同卷改名，加载中亦可），新文件
;      立即就位；.oldN 由 RunOnce(HuFuPostBoot) 与下次安装清理。
;   5. 卸载顺序：码表备份确认 → 清默认输入法覆盖 → 复用 uninstall.ps1
;      全部语义（提权一次；UAC 拒绝则回退 -NoHKLM 每用户清理）→
;      Inno 删程序白名单 → 数据目录保留并告知位置（/PURGEDATA 全清）。
;
; 编译：iscc /DSetupVersion=x.y.z /DDistDir=<输出目录> hufu-setup.iss
;   staging\ 与 staging-files.inc 由 build-setup.ps1 生成。
; ============================================================================

#ifndef SetupVersion
  #define SetupVersion "0.0.0"
#endif
#ifndef SetupVersion4
  #define SetupVersion4 SetupVersion + ".0"
#endif
#ifndef DistDir
  #define DistDir "dist"
#endif

[Setup]
AppId={{2F8A5D41-7B63-4C3E-9E15-A8D4B0C6F293}
AppName=HuFu 虎符输入法
AppVersion={#SetupVersion}
AppVerName=HuFu 虎符输入法 {#SetupVersion}
AppPublisher=HuFu Project
AppComments=以虎码为核心的输入法平台
DefaultDirName={code:GetDefaultDir}
; 目录逻辑自管（zip 存量 + 野残留），禁止 Inno 用自家键自作主张
UsePreviousAppDir=no
AppendDefaultDirName=no
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
; 64 位安装模式：setup 进程为 x64 → {sys}=真 System32，
; 派生的 PowerShell 为 64 位（避免 HKLM 写入被 WOW64 重定向进 32 位视图）
; 仅 x64：x64os = 拒绝 ARM64 仿真（装载层硬门禁，R47；比 [Code] 判断更早）
ArchitecturesAllowed=x64os
ArchitecturesInstallIn64BitMode=x64os
MinVersion=10.0.14393
OutputDir={#DistDir}
OutputBaseFilename=HuFuSetup-{#SetupVersion}
SetupIconFile=staging\图标.ico
UninstallDisplayName=HuFu 虎符输入法
UninstallDisplayIcon={app}\图标.ico
Uninstallable=yes
Compression=lzma2/max
SolidCompression=yes
; 【清场加固 2026-10-14】每次安装自动留全量日志 %TEMP%\Setup Log*.txt
; （排查"拒绝访问/重试跳过中止"类文件占用弹窗的定案证据）
SetupLogging=yes
WizardStyle=modern
; 不用 Restart Manager 硬关用户程序（weasel/CorvusSKK 共识）；腾位自管
CloseApplications=no
RestartApplications=no
VersionInfoVersion={#SetupVersion4}
VersionInfoDescription=HuFu 虎符输入法安装程序

[Languages]
Name: "chinesesimplified"; MessagesFile: "lang\ChineseSimplified.isl"

[CustomMessages]
chinesesimplified.MutexRunning=另一个 HuFu 安装程序正在运行，请先完成或关闭它。
chinesesimplified.WildRemnant=检测到本机有虎符的残留注册信息（原目录已不存在）。%n%n本次将按全新安装处理，残留注册会被自动覆盖；如遇异常请把日志发给开发者。
chinesesimplified.EngineFailed=系统注册引擎执行失败（退出码 %s）。详细日志：%s。输入法文件已就位，但注册可能不完整；请重新运行本安装程序，或把日志发给开发者。
chinesesimplified.DataCopyFailed=数据文件安装出现问题（退出码 %s）。程序与注册仍会继续，但码表可能不完整；请重新运行安装程序。详见：%s
chinesesimplified.DirChanged=检测到旧版安装在其它目录，本次已按你的选择安装到新目录。%n%n旧目录未做任何改动；如需迁移数据，请手动把旧目录中的 码表\、数据\、模型\、码表导出\ 复制到新目录后重跑安装。
chinesesimplified.UninstBackupAsk=卸载前请确认数据已备份（码表\、码表导出\、数据\、模型\ 都在安装目录，建议复制到别处保存）。%n%n【是】= 已备份，开始卸载%n【否】= 先不卸了，我先去备份（将打开文件夹并退出卸载）
chinesesimplified.UninstAborted=已退出卸载（未做任何更改）。备份完成后可再次运行卸载。
chinesesimplified.UpgradeFound=已找到老版虎符，将升级安装，不会影响到码表和个人设置、皮肤等。
chinesesimplified.UninstDoneKeep=卸载完成。
chinesesimplified.UninstOldLeft=另有少量被占用的文件（*.old*）重启后自动清理，未清掉的可手动删除。
chinesesimplified.UninstRebootAsk=是否现在重启？
chinesesimplified.PurgeDoneInfo=已按你的选择彻底删除全部用户数据。

[Messages]
; 内置消息覆盖（必须在此段；写在 [CustomMessages] 无效——2026-10-13 实锤）
chinesesimplified.FinishedRestartLabel=为确保更新的内容生效最好重启一次。
chinesesimplified.FinishedRestartMessage=为确保更新的内容生效最好重启一次。%n%n要立即重启吗？

[Files]
; ---------- 程序文件（进卸载日志：卸载时删除） ----------
Source: "staging\hufu-server.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "staging\hufu_tsf.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "staging\hufu_tsf32.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "staging\hufu-tsf-smoke.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "staging\TigerClaw.Sentence.Native.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "staging\install.ps1"; DestDir: "{app}"; Flags: ignoreversion
Source: "staging\uninstall.ps1"; DestDir: "{app}"; Flags: ignoreversion
#ifdef RUST_ENGINE
Source: "staging\hufu-setup-engine.exe"; DestDir: "{app}"; Flags: ignoreversion
#endif
Source: "staging\hufu-setup-tools.ps1"; DestDir: "{app}"; Flags: ignoreversion
Source: "staging\hufu-postboot.ps1"; DestDir: "{app}"; Flags: ignoreversion
; zip 时代 卸载.bat 由构建替换为指向安装器体系的 stub（GBK）；exe 体系不
; 安装 安装.bat（升级时清除目标目录里的遗留，见 PrepareToInstall）
Source: "staging\卸载.bat"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
Source: "staging\设置.bat"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
Source: "staging\使用说明.txt"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
Source: "staging\更新日志.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "staging\图标.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "staging\version.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "staging\licenses\*"; DestDir: "{app}\licenses"; Flags: ignoreversion recursesubdirs createallsubdirs skipifsourcedoesntexist
; ---------- 数据文件（解压到 {tmp}，由 [Code] 差异备份后拷入；不进卸载日志） ----------
Source: "staging\hufu-files.lst"; DestDir: "{tmp}"; Flags: deleteafterinstall skipifsourcedoesntexist
Source: "staging\hufu-files-upgrade.lst"; DestDir: "{tmp}"; Flags: deleteafterinstall skipifsourcedoesntexist
#include "staging-files.inc"

[Code]
const
  CLSID = '{8F5C2A10-3E77-4B9C-A1D4-9E0B7C2F5A88}';
  PROFILEGUID = '{8F5C2A11-3E77-4B9C-A1D4-9E0B7C2F5A88}';
  PEFiles = 'hufu-server.exe,hufu_tsf.dll,hufu_tsf32.dll,hufu-tsf-smoke.exe,TigerClaw.Sentence.Native.dll';

var
  UpgradeMode: Boolean;
  PrevDir: String;
  PrevVersion: String;
  WildRemnant: Boolean;
  SelectDirMode: Boolean;
  NoHKLMMode: Boolean;
  RenamedOldCount: Integer;
  PurgeDataFlag: Boolean;
  UninstallReallyStarted: Boolean;
  EngineExitCode: Integer;
  MutexHandle: Longword;

function CreateMutexW(lpMutexAttributes: Longword; bInitialOwner: Boolean;
  lpName: String): Longword;
  external 'CreateMutexW@kernel32.dll stdcall';
function GetLastError: Longword;
  external 'GetLastError@kernel32.dll stdcall';

// 参数解析：安装器/卸载器统一走 GetCmdTail（卸载侧 ParamStr 不可用）
function CmdLineHas(const ASwitch: String): Boolean;
var
  Tail, Q: String;
  I, P: Integer;
begin
  Result := False;
  Tail := GetCmdTail();
  I := 1;
  while I <= Length(Tail) do begin
    P := I;
    Q := '';
    while (P <= Length(Tail)) and (Tail[P] <> ' ') do begin
      Q := Q + Tail[P];
      P := P + 1;
    end;
    if UpperCase(Q) = UpperCase(ASwitch) then begin
      Result := True;
      exit;
    end;
    I := P + 1;
  end;
end;

procedure LogLine(const S: String);
begin
  Log(S);
end;

// ============================ 版本读取 ============================
function ReadVersionFromDir(const ADir: String): String;
var
  S, Line: String;
  Buf: AnsiString;
  Lines: TArrayOfString;
  I, P: Integer;
  B: Boolean;
begin
  Result := '';
  // 优先 version.txt（安装器体系随包写入）
  // 注意：Inno 6.7 的 LoadStringFromFile 出参为 AnsiString
  B := LoadStringFromFile(ADir + '\version.txt', Buf);
  if B then begin
    S := String(Buf);
    S := Trim(S);
    if Pos(#$EF#$BB#$BF, S) = 1 then Delete(S, 1, 3);
    while (Length(S) > 0) and ((Copy(S, Length(S), 1) = #13) or (Copy(S, Length(S), 1) = #10)) do
      Delete(S, Length(S), 1);
    if S <> '' then begin
      Result := S;
      exit;
    end;
  end;
  // 回退：更新日志.txt 首行 "2026-10-12  v1.8.0 ..." → 1.8.0
  if LoadStringsFromFile(ADir + '\更新日志.txt', Lines) then
    for I := 0 to GetArrayLength(Lines) - 1 do begin
      Line := Lines[I];
      P := Pos('v', Line);
      if P > 0 then begin
        S := '';
        while P <= Length(Line) do begin
          if (Pos(Line[P], '0123456789.') > 0) or ((Line[P] = 'v') and (S = '')) then
            S := S + Line[P]
          else if S <> '' then
            break;
          P := P + 1;
        end;
        if (Length(S) > 3) and (Copy(S, 1, 1) = 'v') and (Pos('.', S) > 0) then begin
          Result := Copy(S, 2, Length(S) - 1);
          exit;
        end;
      end;
    end;
end;

// ============================ 旧版检测 ============================
// 信号：HKCU\Software\HuFu\InstallDir（权威，install.ps1 L264 写入）
//       HKCU Run\HuFu 值 = "…\hufu-server.exe"（install.ps1 L497 写入）
//       TIP 键存在性（HKCU/HKLM）仅用于"野残留"判定
// 注：HKLM IconFile 指向 SystemIME 副本，不能当安装目录信号
function ValidInstallDir(const ADir: String): Boolean;
begin
  Result := (ADir <> '') and DirExists(ADir) and FileExists(ADir + '\hufu-server.exe');
end;

procedure DetectExistingInstall();
var
  TipHKCU, TipHKLM: Boolean;
  DirFromReg, DirFromRun, S: String;
begin
  UpgradeMode := False;
  WildRemnant := False;
  PrevDir := '';
  PrevVersion := '';

  DirFromReg := '';
  RegQueryStringValue(HKCU, 'Software\HuFu', 'InstallDir', DirFromReg);
  DirFromRun := '';
  if RegQueryStringValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Run',
    'HuFu', DirFromRun) then begin
    // 值形如 "E:\dir\hufu-server.exe"（带引号）→ 去引号去文件名
    S := DirFromRun;
    StringChangeEx(S, '"', '', True);
    if Pos('\hufu-server.exe', LowerCase(S)) > 0 then
      DirFromRun := ExtractFileDir(S)
    else
      DirFromRun := '';
  end;

  if ValidInstallDir(DirFromReg) then
    PrevDir := DirFromReg
  else if ValidInstallDir(DirFromRun) then
    PrevDir := DirFromRun;

  if PrevDir <> '' then begin
    UpgradeMode := True;
    PrevVersion := ReadVersionFromDir(PrevDir);
    if PrevVersion = '' then PrevVersion := '未知版本';
    LogLine('检测到已安装：' + PrevDir + ' (v' + PrevVersion + ')');
  end else begin
    TipHKCU := RegKeyExists(HKCU, 'Software\Microsoft\CTF\TIP\' + CLSID);
    TipHKLM := RegKeyExists(HKLM, 'SOFTWARE\Microsoft\CTF\TIP\' + CLSID);
    if TipHKCU or TipHKLM or (ValidInstallDir(DirFromReg) = False) and
       ((DirFromReg <> '') or (DirFromRun <> '')) then
      // 注册在而目录亡（或目录只剩残壳）
      WildRemnant := TipHKCU or TipHKLM or (DirFromReg <> '') or (DirFromRun <> '');
    if WildRemnant then
      LogLine('野残留：注册在而目录亡（reg=' + DirFromReg + ' run=' + DirFromRun + '）');
  end;
end;

// DefaultDirName 求值：升级锁定旧目录；全新默认用户程序目录
function GetDefaultDir(Param: String): String;
begin
  if UpgradeMode and (PrevDir <> '') then
    Result := PrevDir
  else
    Result := ExpandConstant('{localappdata}') + '\Programs\HuFu';
end;

// ============================ 事件：初始化 ============================
function InitializeSetup(): Boolean;
var
  M: Longword;
begin
  Result := False;

  // 单实例互斥（防连点双开）
  MutexHandle := 0;
  M := CreateMutexW(0, False, 'HuFuSetup_{2F8A5D41-7B63-4C3E-9E15-A8D4B0C6F293}');
  if GetLastError = 183 then begin // ERROR_ALREADY_EXISTS
    SuppressibleMsgBox(ExpandConstant('{cm:MutexRunning}'), mbError, MB_OK, IDOK);
    exit;
  end;
  MutexHandle := M;

  // S1（审计 R47）：x64 包在 ARM64 仿真下"能装不能用"——明示拦截
  if ProcessorArchitecture = paARM64 then begin
    SuppressibleMsgBox('检测到 ARM64 Windows：虎符当前仅支持 x64 系统，' +
      'ARM 仿真下输入法无法工作，已停止安装。', mbError, MB_OK, IDOK);
    exit;
  end;

  NoHKLMMode := CmdLineHas('/NOHKLM');
  SelectDirMode := CmdLineHas('/SELECTDIR');

  DetectExistingInstall();

  if WildRemnant and (not WizardSilent()) then
    MsgBox(ExpandConstant('{cm:WildRemnant}'), mbInformation, MB_OK);

  // 老用户升级：开门见山说清楚（2026-10-13 用户定稿文案）
  // 注意：Inno 脚本 Format 只认 %s（0 基）——%1/%n 会静默截断（探针实测，蓝虎同日坑）
  if UpgradeMode and (not WizardSilent()) then
    MsgBox(ExpandConstant('{cm:UpgradeFound}'), mbInformation, MB_OK);

  LogLine('HuFuSetup ' + '{#SetupVersion}' + ' Upgrade=' + IntToStr(Ord(UpgradeMode)) +
    ' PrevDir=' + PrevDir + ' PrevVer=' + PrevVersion +
    ' WildRemnant=' + IntToStr(Ord(WildRemnant)) +
    ' NoHKLM=' + IntToStr(Ord(NoHKLMMode)));

  Result := True;
end;

// 升级时跳过目录页（/SELECTDIR 可强制显示）
function ShouldSkipPage(PageID: Integer): Boolean;
begin
  Result := (PageID = wpSelectDir) and UpgradeMode and (not SelectDirMode);
end;

// ============================ 进程清理（原生 COM，零子进程）============================
// 【2026-10-14 去 PowerShell 化】火绒等安全软件对"安装器启动 powershell.exe"有
// 行为告警（用户实测两次弹窗）。杀 server 与清场全部改为 Inno 内嵌 WMI COM
// 直杀（WbemScripting），不再产生任何子进程 → 该类告警从根上消失。
// 引擎（install.ps1 血律脚本）仍是 PowerShell——那是唯一残余的一次提权 PS。

function WmiConnect(): Variant;
var
  Locator: Variant;
begin
  Locator := CreateOleObject('WbemScripting.SWbemLocator');
  Result := Locator.ConnectServer('.', 'root\CIMV2');
end;

// 循环杀 hufu-server（TSF 会重生它，循环兜住重生窗口期）
function KillServerInline(): Boolean;
var
  Svc, Q: Variant;
  I, Round, Alive, Total: Integer;
begin
  Result := True;
  Alive := 0;
  try
    Svc := WmiConnect();
    for Round := 1 to 12 do begin
      Q := Svc.ExecQuery('SELECT ProcessId FROM Win32_Process WHERE Name = ''hufu-server.exe''');
      Total := Q.Count;
      if Total = 0 then break;
      for I := 0 to Total - 1 do begin
        try
          Q.ItemIndex(I).Terminate();
        except
          // 枚举漂移/杀不动（提权）：下一轮兜
        end;
      end;
      Sleep(700);
    end;
    Q := Svc.ExecQuery('SELECT ProcessId FROM Win32_Process WHERE Name = ''hufu-server.exe''');
    Alive := Q.Count;
  except
    LogLine('警告：WMI 杀 server 异常（继续，安装引擎会再处理）');
    exit;
  end;
  if Alive > 0 then begin
    Result := False;
    LogLine('警告：hufu-server 未能杀净 ' + IntToStr(Alive) +
      ' 个（多为管理员程序触发的提权重生，改名腾位不受影响，重启自愈）');
  end;
end;

// ============================ 安装目录清场（原生 COM，零子进程）============================
// 点下"安装/卸载"即刻：杀光一切会锁文件的进程——
//   · 任何可执行文件路径在目标目录下的进程（server/smoke/未来新增助手，一网打尽）
//   · 命令行引用目标目录的 cmd/powershell/wscript 控制台（卸载.bat、设置.bat、
//     采集脚本等遗留窗口）
//   · 候选窗/托盘/皮肤动效窗/设置后端——全部由 server 进程持有，随 server 死亡
//     （设置页是浏览器标签，杀浏览器会误伤用户其他标签，不杀；server 死即页面失效）
// 安全阀：排除安装/卸载器自身（否则卸载时 unins000 会自杀）；
//         目录前缀带 \ 终界（D:\HUFU 不误伤 D:\HUFU2）。
// 安装：SweepInstallDirProcesses(WizardDirValue, False)——残留的卸载向导也杀；
// 卸载：SweepInstallDirProcesses(AppDir, True)——保护运行中的 unins000.exe。
function SweepInstallDirProcesses(const DirName: String; KeepSelf: Boolean): Boolean;
var
  Svc, Q, Obj: Variant;
  I, Killed, Left, Left2, Total: Integer;
  InstDir, Pfx, SelfExe, Ep, Cm, Nm, Report: String;
  PathHit, ConsoleHit: Boolean;
begin
  InstDir := DirName;
  if InstDir = '' then begin
    LogLine('清场：目录为空，跳过');
    Result := True;
    exit;
  end;
  Pfx := UpperCase(InstDir) + '\';
  // {srcexe} 仅安装语境有效：卸载语境展开会抛异常（2026-10-14 卸载实测崩溃点）。
  // 护栏：卸载时置空（不匹配任何进程；unins000 自身由 KeepSelf 参数保护）。
  try
    SelfExe := UpperCase(ExpandConstant('{srcexe}'));
  except
    SelfExe := '';
  end;
  Killed := 0;
  Left := 0;
  Left2 := 0;
  try
    Svc := WmiConnect();
    Q := Svc.ExecQuery('SELECT Name, ExecutablePath, CommandLine FROM Win32_Process');
    Total := Q.Count;
    for I := 0 to Total - 1 do begin
      try
        Obj := Q.ItemIndex(I);
        Nm := Obj.Name;
        Nm := UpperCase(Nm);
        Ep := '';
        if not VarIsEmpty(Obj.ExecutablePath) then Ep := Obj.ExecutablePath;
        Cm := '';
        if not VarIsEmpty(Obj.CommandLine) then Cm := Obj.CommandLine;
        Ep := UpperCase(Ep);
        PathHit := ((Pos(Pfx, Ep) = 1) or (Ep = UpperCase(InstDir))) and (Ep <> '') and (Ep <> SelfExe);
        if PathHit and KeepSelf and (Ep = Pfx + 'UNINS000.EXE') then PathHit := False;
        ConsoleHit := (Cm <> '') and (Pos('|' + Nm + '|',
          '|CMD.EXE|POWERSHELL.EXE|PWSH.EXE|CONHOST.EXE|WSCRIPT.EXE|CSCRIPT.EXE|') > 0) and
          (Pos(UpperCase(InstDir), UpperCase(Cm)) > 0);
        if PathHit or ConsoleHit then begin
          Obj.Terminate();
          Killed := Killed + 1;
        end;
      except
        // 枚举漂移（进程恰好消亡）或杀不动（提权）：跳过该项继续
      end;
    end;
    if Killed > 0 then Sleep(600);
    // 复点：路径可见残留（真失败）
    Q := Svc.ExecQuery('SELECT ExecutablePath FROM Win32_Process');
    Total := Q.Count;
    for I := 0 to Total - 1 do begin
      try
        Obj := Q.ItemIndex(I);
        Ep := '';
        if not VarIsEmpty(Obj.ExecutablePath) then begin
          Ep := Obj.ExecutablePath;
          Ep := UpperCase(Ep);
        end;
        if (Ep <> '') and ((Pos(Pfx, Ep) = 1) or (Ep = UpperCase(InstDir))) and (Ep <> SelfExe) then
          Left := Left + 1;
      except
      end;
    end;
    // 名字兜底点验：提权残留（路径不可见）——如实记账
    Q := Svc.ExecQuery('SELECT Name FROM Win32_Process WHERE Name = ''hufu-server.exe'' OR Name = ''hufu-tsf-smoke.exe'' OR Name = ''unins000.exe''');
    Left2 := Q.Count;
  except
    LogLine('警告：WMI 清场异常（继续，改名腾位兜底）');
    Result := True;
    exit;
  end;
  Report := 'KILLED=' + IntToStr(Killed) + ' LEFT2=' + IntToStr(Left2);
  Result := (Left = 0);
  if Result then begin
    if Left2 > 0 then
      LogLine('清场：路径进程清净（' + Report + '）；检测到提权残留进程（如管理员任务管理器'
        + '触发的 server 重生），普通权限无法终止——不影响安装（改名腾位绕开锁），重启自愈')
    else
      LogLine('清场：目标目录进程已清净（' + Report + '）');
  end else
    LogLine('提示：目标目录仍有 ' + IntToStr(Left) + ' 个进程未死（' + Report +
      '）——多为 server 重生竞速，改名腾位已绕开文件锁；若弹占用提示点重试即可');
end;

// ============================ PE 腾位 ============================
function FindFreeOldName(const APath: String): String;
var
  N: Integer;
  Cand: String;
begin
  N := 1;
  repeat
    Cand := APath + '.old' + IntToStr(N);
    N := N + 1;
  until (not FileExists(Cand)) or (N > 99);
  Result := Cand;
end;

procedure RenameOccupiedPEs();
var
  Names: TArrayOfString;
  I, J, P: Integer;
  Target, NewName, One: String;
begin
  One := PEFiles;
  SetArrayLength(Names, 0);
  I := 1;
  while I <= Length(One) do begin
    P := Pos(',', Copy(One, I, MaxInt));
    if P = 0 then begin
      SetArrayLength(Names, GetArrayLength(Names) + 1);
      Names[GetArrayLength(Names) - 1] := Trim(Copy(One, I, MaxInt));
      break;
    end else begin
      SetArrayLength(Names, GetArrayLength(Names) + 1);
      Names[GetArrayLength(Names) - 1] := Trim(Copy(One, I, P - 1));
      I := I + P;
    end;
  end;

  RenamedOldCount := 0;
  for J := 0 to GetArrayLength(Names) - 1 do begin
    Target := WizardDirValue + '\' + Names[J];
    if FileExists(Target) then begin
      NewName := FindFreeOldName(Target);
      if RenameFile(Target, NewName) then begin
        RenamedOldCount := RenamedOldCount + 1;
        LogLine('腾位：' + Names[J] + ' → ' + ExtractFileName(NewName));
      end else
        LogLine('警告：无法腾位 ' + Names[J] + '（留给复制阶段重试）');
    end;
  end;
end;

procedure CleanStaleOldFiles();
var
  SR: TFindRec;
  Dir: String;
begin
  Dir := WizardDirValue;
  if FindFirst(Dir + '\*.old*', SR) then begin
    try
      repeat
        if not DeleteFile(Dir + '\' + SR.Name) then
          LogLine('旧腾位文件暂未能删（可能仍被加载，留待重启/下次安装）：' + SR.Name);
      until not FindNext(SR);
    finally
      FindClose(SR);
    end;
  end;
end;

// 【2026-10-14 断流终检】PEFiles 清单逐一在位校验。2026-10-13 现场实锤：
// BackupDiff 中途断流后（源 DLL 未落地）安装照常进入注册环节，SystemIME
// 活路径被腾位成空，重启后输入法即死。缺任何一件 = 文件落地断流 → 当场
// 中止（raise → Inno 错误框），不带病注册；此时未写任何注册表，原输入法
// 保持原样可用，重跑安装即恢复。正常升级全件在位，本检查零影响。
procedure CheckCriticalPEs();
var
  Names: TArrayOfString;
  One: String;
  I, J, P: Integer;
  Missing: String;
begin
  One := PEFiles;
  SetArrayLength(Names, 0);
  I := 1;
  while I <= Length(One) do begin
    P := Pos(',', Copy(One, I, MaxInt));
    if P = 0 then begin
      SetArrayLength(Names, GetArrayLength(Names) + 1);
      Names[GetArrayLength(Names) - 1] := Trim(Copy(One, I, MaxInt));
      break;
    end else begin
      SetArrayLength(Names, GetArrayLength(Names) + 1);
      Names[GetArrayLength(Names) - 1] := Trim(Copy(One, I, P - 1));
      I := I + P;
    end;
  end;
  Missing := '';
  for J := 0 to GetArrayLength(Names) - 1 do
    if (Names[J] <> '') and (not FileExists(WizardDirValue + '\' + Names[J])) then
      Missing := Missing + Names[J] + ' ';
  if Missing <> '' then begin
    LogLine('终检失败：关键文件未落地：' + Missing + '——中止安装（不注册，防悬空）');
    raise Exception.Create('安装文件未完整落地：' + Missing + #13#10 +
      '文件拷贝环节中断（磁盘空间/杀软拦截？）。尚未做任何系统注册，' + #13#10 +
      '当前输入法保持原样；请重新运行本安装程序。');
  end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := '';
  // 【清场 2026-10-14 用户指令】确认安装的第一时间：杀 server（含 TSF 重生兜底循环）
  // → 清场一切安装目录进程（smoke/unins000/bat 控制台/任何 {app} 进程）
  KillServerInline();
  SweepInstallDirProcesses(WizardDirValue, False);
  // S4（审计）：野残留=死 HKCU TIP 键。先清掉，install.ps1 的 $tipAlready
  // 才会判"全新安装"→走首装路径（含 ctfmon 刷新），装完即可切换输入法。
  if WildRemnant then begin
    RegDeleteKeyIncludingSubkeys(HKCU, 'Software\Microsoft\CTF\TIP\' + CLSID);
    LogLine('野残留：已预清死 TIP 键，引擎按全新安装处理');
  end;
  CleanStaleOldFiles();
  // 铁律口径（2026-10-13）：exe 体系不装 安装.bat（已装目录无此入口）。
  // 升级目录里的遗留——zip 时代真·安装.bat 或旧 exe 的 stub——一并清除：
  // 前者直跑 install.ps1 属双体系误用，后者已无意义。
  if FileExists(WizardDirValue + '\安装.bat') then begin
    DeleteFile(WizardDirValue + '\安装.bat');
    LogLine('已清除遗留 安装.bat（exe 体系：重装/升级请直接运行安装包）');
  end;
  RenameOccupiedPEs();
  NeedsRestart := False; // 完成页重启引导由 NeedRestart() 决定
end;

// ============================ 数据拷贝（差异备份） ============================
function RunBackupDiff(): Boolean;
var
  Code: Integer;
  BackupRoot, PrevTag, ManifestPath: String;
begin
  // 【铁律】覆盖安装时用户财产零改动：码表\、数据\皮肤\ 不写入、不补缺、
  // 不复活已删——升级走剔除冻结项的 hufu-files-upgrade.lst；全新安装才用
  // 全量清单落盘（全新目录不存在"用户已删"概念，全量即正确）。
  ManifestPath := ExpandConstant('{tmp}') + '\hufu-files.lst';
  if UpgradeMode and FileExists(ExpandConstant('{tmp}') + '\hufu-files-upgrade.lst') then
    ManifestPath := ExpandConstant('{tmp}') + '\hufu-files-upgrade.lst';
  LogLine('BackupDiff 清单: ' + ManifestPath);
  if UpgradeMode then
    LogLine('  升级模式：码表\、数据\皮肤\ 冻结（零写入/零补缺/零复活已删）')
  else
    LogLine('  全新安装：全量清单落盘');

  PrevTag := PrevVersion;
  if (PrevTag = '') or (PrevTag = '未知版本') then PrevTag := '旧版';
  // 注意：GetDateTimeString 第 2/3 参是 Char——不能传空串（运行时 Type
  // Mismatch，审计 M3 实锤）；格式串不含分隔符，给 '/' 不影响输出
  BackupRoot := WizardDirValue + '\升级备份\v' + PrevTag + '-' +
    GetDateTimeString('yyyymmdd', '/', '/') + '-' + GetDateTimeString('hhnn', '/', '/');

  Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
    '-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File "' +
    WizardDirValue + '\hufu-setup-tools.ps1" -BackupDiff' +
    ' -Manifest "' + ManifestPath + '"' +
    ' -SrcRoot "' + ExpandConstant('{tmp}') + '\hufu-pkg"' +
    ' -DstRoot "' + WizardDirValue + '"' +
    ' -BackupRoot "' + BackupRoot + '"',
    WizardDirValue, SW_HIDE, ewWaitUntilTerminated, Code);
  Result := (Code = 0);
  if not Result then begin
    LogLine('BackupDiff 失败，退出码 ' + IntToStr(Code));
    SuppressibleMsgBox(Format(ExpandConstant('{cm:DataCopyFailed}'), [IntToStr(Code),
      WizardDirValue + '\setup-tools.log']), mbError, MB_OK, IDOK);
  end;
end;

// ============================ 引擎：复用 install.ps1 ============================
function RunInstallEngine(): Boolean;
var
  Code: Integer;
  Extra: String;
begin
  Extra := '';
  if NoHKLMMode then Extra := ' --no-hklm';
#ifdef RUST_ENGINE
  // Rust 引擎轨：零 PowerShell 子进程。引擎自身把进度写 setup-engine.log
  //（--log 参数），退出码语义与 PS 轨一致（0=成功；1=致命；提权失败仅告警
  // 不置败——install.ps1 L344/352 同款语义）。
  Exec(WizardDirValue + '\hufu-setup-engine.exe',
    'install' + Extra + ' --log "' + WizardDirValue + '\setup-engine.log"',
    WizardDirValue, SW_HIDE, ewWaitUntilTerminated, Code);
  EngineExitCode := Code;
  Result := (Code = 0);
  if not Result then begin
    LogLine('hufu-setup-engine 退出码 ' + IntToStr(Code));
    SuppressibleMsgBox(Format(ExpandConstant('{cm:EngineFailed}'), [IntToStr(Code),
      WizardDirValue + '\setup-engine.log']), mbError, MB_OK, IDOK);
  end;
#else
  // 血律 M1（审计发现）：install.ps1 成功路径无结尾 exit，$LASTEXITCODE
  // 是最后一次原生调用的遗留码（install.ps1 L339-344 明文警告过的反模式）。
  // 改用 $? 判定；install.ps1 自身显式 exit N 时会直接终结本进程=透传 N。
  Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
    '-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -Command "' +
    'try { Set-Location -LiteralPath ''' + WizardDirValue + ''';' +
    '& ''.\install.ps1''' + Extra + ' 2>&1 | Out-File -Encoding utf8 ''' +
    WizardDirValue + '\setup-engine.log''; ' +
    'if ($?) { exit 0 } else { exit 1 } } catch { exit 1 }"',
    WizardDirValue, SW_HIDE, ewWaitUntilTerminated, Code);
  EngineExitCode := Code;
  Result := (Code = 0);
  if not Result then begin
    LogLine('install.ps1 退出码 ' + IntToStr(Code));
    SuppressibleMsgBox(Format(ExpandConstant('{cm:EngineFailed}'), [IntToStr(Code),
      WizardDirValue + '\setup-engine.log']), mbError, MB_OK, IDOK);
  end;
#endif
end;

function ServerAlive(): Boolean;
var
  Code: Integer;
begin
#ifdef RUST_ENGINE
  Exec(WizardDirValue + '\hufu-setup-engine.exe', 'server-alive',
    WizardDirValue, SW_HIDE, ewWaitUntilTerminated, Code);
#else
  Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
    '-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -Command "' +
    'exit [int](-not (Get-Process hufu-server -ErrorAction SilentlyContinue))"',
    '', SW_HIDE, ewWaitUntilTerminated, Code);
#endif
  Result := (Code = 0);
end;

procedure StartServerViaExplorer();
var
  Code: Integer;
begin
  // install.ps1 L513-516 同款：经 explorer 中转，保证普通权限令牌
  Exec(ExpandConstant('{sys}\explorer.exe'), '"' + WizardDirValue + '\hufu-server.exe"',
    '', SW_HIDE, ewNoWait, Code);
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  DirNow: String;
  SR: TFindRec;
  Remnants: TArrayOfString;
  N, I: Integer;
begin
  if CurStep = ssPostInstall then begin
    // 1) 数据文件：差异备份 → 拷入（不进卸载日志）；升级时码表/皮肤整目录
    //    冻结不写入（清单由 RunBackupDiff 按模式挑选）
    if FileExists(ExpandConstant('{tmp}') + '\hufu-files.lst') or
       FileExists(ExpandConstant('{tmp}') + '\hufu-files-upgrade.lst') then
      RunBackupDiff()
    else
      LogLine('无数据清单（空数据包），跳过数据拷贝');

    // 1b) 【2026-10-14 断流终检】注册环节前关键 PE 必须全部在位（见
    //     CheckCriticalPEs 注释）；缺件中止，不带病注册
    CheckCriticalPEs();

    // 2) 系统注册：零修改复用 install.ps1（内部自带 UAC 提权段 +
    //    语言列表 enable + 默认输入法覆盖 + 自启 + 降权启动 server）
    RunInstallEngine();

    // 2b) S6（审计）：机器级注册缺失（提权被拒等）要浮出水面，不能只躺日志
    if (not NoHKLMMode) and
       (not RegKeyExists(HKLM, 'SOFTWARE\Microsoft\CTF\TIP\' + CLSID)) then begin
      LogLine('警告：HKLM TIP 键缺失——机器级注册未完成');
      SuppressibleMsgBox('机器级注册似乎未完成（可能未允许提权）。' + #13#10 +
        '输入法可能暂时无法在所有应用中使用；重新运行本安装程序并允许提权即可补全。' + #13#10 +
        '详见：' + WizardDirValue + '\setup-engine.log',
        mbInformation, MB_OK, IDOK);
    end;
    // 3) 回读校验
    DirNow := '';
    RegQueryStringValue(HKCU, 'Software\HuFu', 'InstallDir', DirNow);
    if DirNow <> WizardDirValue then
      LogLine('警告：HKCU InstallDir=' + DirNow + ' 与安装目录不一致（注册可能异常）');

    // 4) RunOnce：重启后清 .oldN + 自检（幂等，无事则秒退）
#ifdef RUST_ENGINE
    RegWriteStringValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\RunOnce',
      'HuFuPostBoot',
      '"' + WizardDirValue + '\hufu-setup-engine.exe" postboot');
#else
    RegWriteStringValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\RunOnce',
      'HuFuPostBoot',
      'powershell.exe -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File "' +
      WizardDirValue + '\hufu-postboot.ps1"');
#endif

    // 5) server 存活核验（install.ps1 已启动；未活则经 explorer 拉起）
    Sleep(3000);
    if not ServerAlive() then begin
      LogLine('install.ps1 后未见 hufu-server，经 explorer 拉起');
      StartServerViaExplorer();
    end;

    // 5b) 【2026-10-14 补账】升级腾位残件当场清：引擎已把 server 切到新
    //     PE，.oldN 此刻已解锁——能删的当场删光，不等重启/下次安装。
    //     仍被占用的（罕见：旧 DLL 镜像还在某进程里）留给 postboot/
    //     下次安装/卸载三重兜底。先收集再删（边枚举边删会跳项）。
    N := 0;
    SetArrayLength(Remnants, 0);
    if FindFirst(WizardDirValue + '\*.old*', SR) then begin
      repeat
        SetArrayLength(Remnants, N + 1);
        Remnants[N] := SR.Name;
        N := N + 1;
      until not FindNext(SR);
      FindClose(SR);
    end;
    if N > 0 then begin
      for I := 0 to N - 1 do
        if not DeleteFile(WizardDirValue + '\' + Remnants[I]) then
          LogLine('腾位残件仍占用：' + Remnants[I] + '（postboot 重启后清）');
      LogLine('升级腾位清理：发现 ' + IntToStr(N) + ' 个 .old*，已尝试删除');
    end;

    // 6) 升级换了目录（/SELECTDIR 高级用法）的提示
    if UpgradeMode and (PrevDir <> '') and (PrevDir <> WizardDirValue) and
       (not WizardSilent()) then
      MsgBox(ExpandConstant('{cm:DirChanged}'), mbInformation, MB_OK);
  end;
end;

function NeedRestart(): Boolean;
begin
  // 腾位发生（旧 PE 待重启清理）或覆盖升级（新 DLL 需全进程加载）
  // 或野残留首装（预清后走首装路径，msctf 列表刷新需重启兜底）
  Result := (RenamedOldCount > 0) or UpgradeMode or WildRemnant;
end;

procedure CurPageChanged(CurPageID: Integer);
begin
  // 完成页重启提示：字号加大（2026-10-13 用户定稿，默认 9pt 偏小）
  if CurPageID = wpFinished then
    WizardForm.FinishedLabel.Font.Size := 12;
end;

// ============================ 卸载 ============================
// 【交互口径 2026-10-13 用户定稿】全程最多 3 次点击：
//   ①系统确认框（第一下；否=直接退出，无任何其他弹窗）
//   ②备份确认（是=已备份继续；否=打开文件夹并退出）
//   ③完成告知（数据位置 + .old 需重启清理 + 是否现在重启）
// 「连数据一起删」不再交互询问，仅 /PURGEDATA 命令行。
function InitializeUninstall(): Boolean;
begin
  // 不在此弹任何窗：所有交互都在系统确认框（必然先出现）之后
  PurgeDataFlag := CmdLineHas('/PURGEDATA');
  Result := True;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Code: Integer;
  AppDir: String;
  ElevatedOk: Boolean;
  SR: TFindRec;
  Remnants: TArrayOfString;
  N, I: Integer;
begin
  AppDir := ExpandConstant('{app}');

  if CurUninstallStep = usPostUninstall then begin
    // 【2026-10-14 补账】腾位改名产生的 .oldN 不在卸载器台账内
    // （unins000.dat 只记安装原名，改过名=账外文件）——卸载收尾主动清账：
    // 先收集再删（边枚举边删会跳项）；删不动的（罕见：残进程仍占 .oldN
    // 镜像）由 UninstallNeedRestart 的重启提示兜底。
    N := 0;
    SetArrayLength(Remnants, 0);
    if FindFirst(AppDir + '\*.old*', SR) then begin
      repeat
        SetArrayLength(Remnants, N + 1);
        Remnants[N] := SR.Name;
        N := N + 1;
      until not FindNext(SR);
      FindClose(SR);
    end;
    for I := 0 to N - 1 do
      if not DeleteFile(AppDir + '\' + Remnants[I]) then
        LogLine('残占未删：' + Remnants[I] + '（重启后自清或手动删）');
    if N > 0 then
      LogLine('腾位残账清理：发现 ' + IntToStr(N) + ' 个 .old*，已尝试删除');
    exit;
  end;

  if CurUninstallStep = usUninstall then begin
    // 程序文件尚在（Inno 随后才删白名单）——引擎脚本此刻可用

    // 0) 备份确认（唯一一道；口径：是=已备份继续，否=打开文件夹并退出）
    //    注意：通过后才置 UninstallReallyStarted（退出路径不得触发完成弹窗）
    if not UninstallSilent() then begin
      if MsgBox(ExpandConstant('{cm:UninstBackupAsk}'), mbConfirmation, MB_YESNO) = IDNO then begin
        Exec(ExpandConstant('{sys}\explorer.exe'),
          '/select,"' + AppDir + '\码表"', '', SW_SHOW, ewNoWait, Code);
        RaiseException(ExpandConstant('{cm:UninstAborted}'));
      end;
    end;
    UninstallReallyStarted := True; // 已过系统确认框+备份确认：真正开始卸载

    // 0a) 修实证债务 R16：清默认输入法覆盖（必须先于语言列表减法）
    //    S2（审计）：只清"指向虎符"的覆盖，他人输入法的默认设置不碰
#ifdef RUST_ENGINE
    Exec(AppDir + '\hufu-setup-engine.exe', 'clear-override',
      AppDir, SW_HIDE, ewWaitUntilTerminated, Code);
#else
    Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
      '-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -Command "' +
      '$v=(Get-ItemProperty ''HKCU:\Control Panel\International\User Profile''' +
      ' -Name InputMethodOverride -ErrorAction SilentlyContinue).InputMethodOverride;' +
      'if ($v -and $v -like ''*8F5C2A11*'') { Remove-ItemProperty ' +
      '''HKCU:\Control Panel\International\User Profile''' +
      ' -Name InputMethodOverride -ErrorAction SilentlyContinue }; exit 0"',
      '', SW_HIDE, ewWaitUntilTerminated, Code);
#endif
    LogLine('InputMethodOverride 清除完成（仅虎符值）');

    // 0b) S3（审计）：清掉 RunOnce 尾巴，防重启后引用已删除的脚本
    RegDeleteValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\RunOnce',
      'HuFuPostBoot');

    // 1) 杀 server（uninstall.ps1 也会杀；先杀减一次复活窗口）+ 清场安装目录进程
    //    （卸载时残留的设置.bat 窗口/smoke 等也会锁文件；保护 unins000 自身）
    KillServerInline();
    SweepInstallDirProcesses(AppDir, True);

    // 2) 提权一次跑引擎尾巴：[原版 uninstall.ps1] + PFRO 挂删 SystemIME
    //    残留 + HKLM 清净校验（Rust 轨=hufu-setup-engine uninstall 自带
    //    全部三件；PS 轨=tools 内部以管理员运行 uninstall.ps1，
    //    它检测到已是管理员即不再二次自提权）
#ifdef RUST_ENGINE
    ElevatedOk := ShellExec('runas',
      AppDir + '\hufu-setup-engine.exe',
      'uninstall --log "' + AppDir + '\setup-engine.log"',
      AppDir, SW_HIDE, ewWaitUntilTerminated, Code);
    if (not ElevatedOk) or (Code = 1223) then begin
      // UAC 被拒 → 回退每用户清理（--no-hklm 语义；机器级残留留待重装覆盖）
      LogLine('提权被拒，回退每用户卸载（HKLM/SystemIME 残留将保留）');
      Exec(AppDir + '\hufu-setup-engine.exe',
        'uninstall --no-hklm --log "' + AppDir + '\setup-engine.log"',
        AppDir, SW_HIDE, ewWaitUntilTerminated, Code);
    end else if Code <> 0 then
      LogLine('引擎卸载尾 退出码 ' + IntToStr(Code) +
        '（4=HKLM 残留，详见 setup-engine.log）');
#else
    ElevatedOk := ShellExec('runas',
      ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
      '-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File "' + AppDir +
      '\hufu-setup-tools.ps1" -ElevatedUninstallTail -InstDir "' + AppDir + '"',
      AppDir, SW_HIDE, ewWaitUntilTerminated, Code);
    if (not ElevatedOk) or (Code = 1223) then begin
      // UAC 被拒 → 回退每用户清理（-NoHKLM 语义；机器级残留留待重装覆盖）
      LogLine('提权被拒，回退每用户卸载（HKLM/SystemIME 残留将保留）');
      Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
        '-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File "' + AppDir +
        '\uninstall.ps1" -NoHKLM',
        AppDir, SW_HIDE, ewWaitUntilTerminated, Code);
    end else if Code <> 0 then
      LogLine('ElevatedUninstallTail 退出码 ' + IntToStr(Code) +
        '（4=HKLM 残留，详见 setup-tools.log）');
#endif

    // 3) 彻底删数据（仅当用户明确选择 /PURGEDATA 或界面勾选）
    if PurgeDataFlag then begin
#ifdef RUST_ENGINE
      Exec(AppDir + '\hufu-setup-engine.exe',
        'purge-data --log "' + AppDir + '\setup-engine.log"',
        AppDir, SW_HIDE, ewWaitUntilTerminated, Code);
      LogLine('PurgeData 完成');
#else
      Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
        '-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File "' + AppDir +
        '\hufu-setup-tools.ps1" -PurgeData -InstDir "' + AppDir + '"',
        AppDir, SW_HIDE, ewWaitUntilTerminated, Code);
      LogLine('PurgeData 完成');
#endif
    end;
  end;
end;

function UninstallNeedRestart(): Boolean;
var
  SR: TFindRec;
  AppDir: String;
begin
  // 安装目录残留 .old*（被占用未删净）→ 提示重启完成清理
  AppDir := ExpandConstant('{app}');
  Result := False;
  if FindFirst(AppDir + '\*.old*', SR) then begin
    Result := True;
    FindClose(SR);
  end;
end;

procedure DeinitializeUninstall();
var
  AppDir, DataList, Msg: String;
  Q: Integer;
begin
  if UninstallSilent() then exit;
  // 用户在系统确认框点了"否"，或在备份确认选择退出 → 什么都不弹
  if not UninstallReallyStarted then exit;

  AppDir := ExpandConstant('{app}');
  if PurgeDataFlag then
    SuppressibleMsgBox(ExpandConstant('{cm:PurgeDoneInfo}'), mbInformation, MB_OK, IDOK)
  else begin
    DataList := '';
    if DirExists(AppDir + '\码表') then DataList := DataList + '码表\' + #13#10;
    if DirExists(AppDir + '\码表导出') then DataList := DataList + '码表导出\' + #13#10;
    if DirExists(AppDir + '\数据') then DataList := DataList + '数据\' + #13#10;
    if DirExists(AppDir + '\模型') then DataList := DataList + '模型\' + #13#10;
    if DataList = '' then
      DataList := '（本机未发现用户数据目录）' + #13#10;
    // 完成框：代码拼串（#13#10 换行 + 路径置顶；不用 Format/%n——脚本层 %n 非换行）
    Msg := ExpandConstant('{cm:UninstDoneKeep}') + #13#10#13#10 +
      '你的数据仍保留在下面这个文件夹（没有删除）：' + #13#10#13#10 +
      AppDir + #13#10#13#10 +
      '里面有：' + #13#10 + DataList + #13#10 +
      '确认不再需要时，手动删除上面这个文件夹即可彻底清理。';
    if UninstallNeedRestart() then begin
      Msg := Msg + #13#10#13#10 + ExpandConstant('{cm:UninstOldLeft}') + #13#10#13#10 + ExpandConstant('{cm:UninstRebootAsk}');
      if MsgBox(Msg, mbInformation, MB_YESNO) = IDYES then begin
        LogLine('用户选择立即重启完成清理');
        Exec(ExpandConstant('{sys}\shutdown.exe'), '/r /t 0', '',
          SW_SHOW, ewNoWait, Q);
      end;
    end else
      SuppressibleMsgBox(Msg, mbInformation, MB_OK, IDOK);
  end;
end;
