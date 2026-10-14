# hufu-setup-engine 模块契约（全队唯一接口真理）

血律源文件（只读，一个字不许改）：
- `E:\DSH-KF\hufu\platform\windows\install\install.ps1`（556 行 = install.rs 规格）
- `E:\DSH-KF\hufu\platform\windows\install\uninstall.ps1`（206 行 = uninstall.rs 规格）
- `E:\DSH-KF\hufu\platform\windows\setup\helper\hufu-setup-tools.ps1`（只取契约：L418-510 尾巴 PFRO+exit4、L512-551 PostBoot、L553-591 PurgeData）

## 总规则（每个人必须遵守）

1. **行为等价**：每个 PS 行为在 Rust 里都有对应实现；不确定的语义宁可保守（照 PS 字面翻译），不要"优化"。
2. **文案逐字节**：Write-Host 文案原样搬运（供 setup-engine.log 与 PS 版 diff 验收）；install.log 行的格式串逐字节对齐（带 `[yyyy-MM-dd HH:mm:ss] ` 前缀，由 Out::install_log 生成）。
3. **数据铁律**：install/uninstall 永不写/删 `码表\`、`数据\`、`模型\`、`码表导出\`、`用户调整.txt`（uninstall 只读它们出"数据保留指引"文案；唯一例外=purge-data 子命令显式删除）。
4. **server 永不提权启动**：提权上下文里启动 server 必须经 explorer.exe 中转（install.ps1 L510-518）。
5. **不跑 cargo**：模块没齐前整个 workspace 编不过，语法自查即可；Lead 负责合稿编译修错。
6. 所有路径/字符串用 UTF-16 安全写法（std::path::PathBuf + W 系 API）；注册表用 winreg crate。
7. 错误处理照 PS 的 `$ErrorActionPreference='Continue'` 语义：单步失败打 ⚠ 行继续，不 panic、不 unwrap 裸奔（除明确不可达）。

## 模块接口（签名不得更改）

### regop.rs（队友 reg）
```rust
pub fn set_str(root: &str, path: &str, name: &str, val: &str) -> bool;   // name=="(default)" 写默认值
pub fn set_dword(root: &str, path: &str, name: &str, val: u32) -> bool;
pub fn get_str(root: &str, path: &str, name: &str) -> Option<String>;   // "(default)" 读默认值
pub fn key_exists(root: &str, path: &str) -> bool;
pub fn ensure_key(root: &str, path: &str) -> bool;                       // 递归创建
pub fn delete_tree(root: &str, path: &str) -> bool;                      // 递归删除，不存在=true
pub fn delete_value(root: &str, path: &str, name: &str) -> bool;
pub fn enum_subkeys(root: &str, path: &str) -> Vec<String>;              // 失败=空
pub fn enum_value_names(root: &str, path: &str) -> Vec<String>;
```
- root 只取 "HKCU"/"HKLM"；path 为不带 hive 前缀的子键路径（如 `Software\Microsoft\CTF\TIP\{...}`）。
- set_str 的 "(default)"：winreg 里对键 set_value("", val) 写默认值。
- 语义对齐 PS Set-Reg/Set-RegDWord/Set-RegHKLM/New-Item -Force（键不存在则先建）/Remove-Item。
- 返回值：成功 true；失败 false（调用方决定是否打 ⚠）。

### acl.rs（队友 reg）
```rust
pub fn grant_all_app_packages(path: &std::path::Path, inherit: bool) -> bool;
```
- 等价 `icacls <path> /grant "ALL APPLICATION PACKAGES:(OI)(CI)RX"`（inherit=true）或 `"...:RX"`（inherit=false）。
- 实现：GetNamedSecurityInfoW(DACL) → ConvertStringSidToSidW("S-1-15-2-1") → BuildExplicitAccessW(grfAccessPermissions=GENERIC_READ|GENERIC_EXECUTE, grfInheritance=SUB_CONTAINERS_AND_OBJECTS_INHERIT 或 NO_INHERITANCE) → SetEntriesInAclW → SetNamedSecurityInfoW。注意windows-rs 0.58 的 EXPLICIT_ACCESS_W/TRUSTEE_W 均在 Win32::Security。

### procop.rs（队友 proc）
```rust
pub fn kill_by_name(name: &str) -> usize;                    // Toolhelp32 快照匹配进程名(不区分大小写) TerminateProcess，返回击杀数
pub fn proc_running(name: &str) -> bool;
pub fn spawn_wait_hidden(exe: &Path, args: &str, timeout_ms: u32) -> SpawnResult;
pub enum SpawnResult { Exit(i32), Timeout, Fail }
pub fn spawn_capture(exe: &Path, args: &str, timeout_ms: u32, out_file: &Path) -> SpawnResult; // 子进程 stdout 重定向到 out_file（smoke enable 契约）
pub fn spawn_runas_wait(exe: &Path, args: &str, timeout_ms: u32) -> SpawnResult;  // ShellExecuteExW runas + SEE_MASK_NOCLOSEPROCESS，等待+取退出码；UAC 被拒=Fail
pub fn spawn_detached(exe: &str, arg: &str);                  // 火忘（ctfmon/explorer 中转/server）
```
- spawn_wait_hidden 用 CreateProcessW(CREATE_NO_WINDOW)；超时 TerminateProcess 后返回 Timeout。
- spawn_detached 对 explorer.exe：`explorer.exe "<exe路径>"`（参数带引号原样传）。

### lnk.rs（队友 proc）
```rust
pub fn create_url_shortcut(lnk_path: &Path, url: &str, icon: &Path) -> bool;
```
- 等价 WScript.Shell CreateShortcut（TargetPath=http://…, IconLocation=图标.ico）。COM：CoInitializeEx + CLSID_ShellLink → IShellLinkW::SetPath/SetIconLocation → IPersistFile::Save。

### pfro.rs（队友 proc）
```rust
pub fn schedule_delete(path: &Path) -> bool;   // MoveFileExW(src, null, MOVEFILE_DELAY_UNTIL_REBOOT)
```

### install.rs（队友 install）
```rust
pub fn run_install(out: &mut crate::common::Out, no_hklm: bool, user_only: bool) -> i32;
pub fn run_phase_elevated(out: &mut crate::common::Out) -> i32;
```
- 规格=install.ps1 全文逐段：L241-257 安全检查→exit 1；L262-323 Install-UserConfig（regsvr32 /s dll 用 procop::spawn_wait_hidden 调 `regsvr32.exe /s <dll>`，超时给 120s）；L325-332 user_only；L336-381 提权三分支（admin 直跑子命令 `install-elevated`；可提权→spawn_runas_wait 日志走 common::elevated_log()，子进程自己把输出写进该文件，父进程读回逐行 echo——子命令入口见 main.rs 约定 `install-elevated`）；L383-493 语言列表（快照结构自己实现，格式串照抄）；L495-508 Run键+.lnk；L510-518 杀+启 server；L531-539 ctfmon 三分支；L541-556 横幅。
- Set-WinDefaultInputMethodOverride = HKCU `Control Panel\International\User Profile` 值 `InputMethodOverride`=tip_str()（REG_SZ）。
- smoke 调用细节照 L74-82（60s 超时击杀按 0 处理）与 L433-453（enable 输出文件判据：含 `OK 虎符已加入语言列表` 或 (exit==0 且输出非空且不含 `失败`)）。
- run_phase_elevated 规格=L85-228：SystemIME 双架构拷贝（.oldN 腾位 fallback）+ ACL + WOW6432Node + 清理段（HuFu.old* 目录/HuFu\*.old*/ProgramData\HuFu\diag/%LOCALAPPDATA%\HuFu）+ HKLM CLSID/TIP/8分类/LanguageProfile + smoke reg(sysdll) + HKCU COM 回写校验；退出码=smoke rc（L226-227）。
- install.log 追加点与格式串：L448（`install enable 失败: exit={1} {2}`）、L476-477（`install CTF原生启用: enable={1} 他人快照={2}（真存储{3}+装配表{4}） 末态他人={5} 我们在场={6} 丢失=[{7}] ctfmon预热={8}`）——用 out.install_log()，内容部分自己 format。
- 安装目录=common::install_dir()；$src/$inst 同一。

### uninstall.rs（队友 uninst）
```rust
pub fn run_uninstall(out: &mut crate::common::Out, no_hklm: bool) -> i32;
```
- 规格=uninstall.ps1 全文 + tools 尾巴三件（PFRO 登记、HKLM 校验 exit=4、ProgramData 重试 2 次）。
- 自提权：非 admin 且 !no_hklm 且 in_admin_group() → spawn_runas_wait(`uninstall` 无参=完整模式) → exit 子码。
- 顺序铁律照 ps1：列表纯减法→杀 server+ctfmon→smoke unreg→Run键→HKCU 清→lnk→legacy→HKLM 清→SystemIME（锁则改名 HuFu.oldN）→PFRO→校验→启 ctfmon→数据指引文案。
- 卸载的日志行（install.log）：L63（`uninstall 减法移除: 删槽={1} 残留（真存储={2}，装配表={3}）——应全为 0`）。
- SystemIME 清理细节照 ps1 后半段（先试删整目录 HuFu，失败→逐文件；仍失败→改名 .oldN 下一轮；PFRO 兜底=tools L418-436 语义：SystemIME 两处 HuFu.old* 目录内全部文件 + HuFu\*.old* 文件逐个 schedule_delete）。
- HKLM 校验键集（tools L466-472 同款）：HKLM TIP、HKLM Classes\CLSID\{clsid}、HKLM WOW6432Node\CTF\TIP、HKLM Classes\WOW6432Node\CLSID —— 任一残留 → return 4。
- 温馨提示段（数据保留指引）逐字照 ps1 L188-206。

### postboot.rs（队友 uninst）
```rust
pub fn run_postboot(out: &mut crate::common::Out) -> i32;   // 恒 0
```
- 规格=tools Invoke-PostBootCmd：{app}\*.old* 5×1s 重试删；SystemIME 两处 HuFu.old* 目录 + HuFu\*.old* 尽力删；打一行 RESULT 总结（格式照 L549）。

### purge.rs（队友 uninst）
```rust
pub fn run_purge_data(out: &mut crate::common::Out) -> i32;  // 恒 0
```
- 规格=tools Invoke-PurgeDataCmd：删 `码表/数据/模型/码表导出/升级备份`（2×400ms 重试、先归一化属性）；再删根下 `install.log`+`setup-*.log`（活动日志自身跳过=当前 --log 指的那份）；RESULT 行照 L589。

### common.rs / main.rs（Lead 已写/将写）
- Out：line()/host_only()（stdout+引擎日志）、install_log()（追加 {app}\install.log，带时间戳）。
- main.rs 子命令：`install [--no-hklm] [--user-only]`、`install-elevated`、`uninstall [--no-hklm]`、`postboot`、`purge-data`，公共参数 `--log <path>`（引擎日志；install-elevated 默认 %TEMP%\hufu-install-elevated.log）。
- 常量/路径助手/now_stamp()/is_admin()/in_admin_group() 都在 common.rs。

## 版本指纹
- windows = 0.58（对齐 hufu-tsf-smoke），winreg = 0.55。
- workspace = E:\DSH-KF\hufu\platform\windows（我们 crate=hufu-setup-engine，已挂 members）。

## 【2026-10-14 断流修复】引擎线对血律的三处有据偏离（ps1 冻结不改）

背景：2026-10-13 现场（1.8.0→1.8.1 升级，三份取证日志）实锤"升级替换"致死链——
文件落地断流（BackupDiff 中途死）→ 提权段**先腾位后才发现源缺失** → SystemIME 活路径
被改名为 .oldN 而新件未落 → 注册表指向空路径 → 当次会话靠内存旧 DLL 假活 →
重启后输入法全灭。修复只挂失败分支，成功路径逐字节不变（真机回归已验）：

1. **copy_dll_with_old_fallback 事务化**（install.rs）：源预检（缺源不碰目标）
   + 腾位后复制失败立即回滚还原活名；返回 bool，x64 段失败=提权段 return 1
   （ps1 同为 throw）；x86 段新增一行失败播报（ps1 原为完全静默）。
2. **父层硬失败**（install.rs 提权两分支）：提权子进程跑过且失败 + SystemIME
   活件缺失 = 注册表将悬空 → `✗` 两行 + return 1，绝不"安装完成"。
   （UAC 被拒=提权未运行过，HKCU COM 已回落安装目录路径、状态自洽——保持旧告警语义。）
3. **heal_missing_live_dll 自愈**（common.rs，install 提权段拷贝前 + postboot 清理前）：
   活 DLL 缺失而 .oldN 孤本在 → 还原最小编号可还原者（改名失败试下一编号）。
   健康机器零动作零日志。postboot 侧为尽力保险（默认 ACL 下无写权限则静默跳过），
   主救援=重跑安装器时的提权段自愈（真机 B2 实测：故障机一次安装=自愈+升级）。

配套（iss，两轨共享）：PrepareToInstall→[Files] 后 RunBackupDiff → 注册环节前
CheckCriticalPEs() 终检 PEFiles 清单；缺件 raise 中止（未做任何注册，原输入法保持原样）。

真机验收（2026-10-14，D:\HuFu rust 轨健康机上四轮）：
A 回归=成功路径零变化（无自愈/回滚行，enable 行同款）；B3=腾位后复制必败→回滚保活件+
告警继续；B2=注入故障态（活件改 .oldN）→ 一次安装自愈+升级；B4=postboot 健康态零动作恒 0。
✗ 硬失败分支：活件被进程映射时管理员亦不可删（腾位机制的存在理由），无法真机注入；
由组件级验证（rollback/return 1/⚠ 各自实测）+ 代码审查覆盖。
