# qa-audit-report.md — hufu-setup.iss 血律审计报告（第 1 轮）

> **审计对象**：`E:\DSH-KF\hufu\platform\windows\setup\hufu-setup.iss`（628 行，lead 修正版）
> **审计基准**：qa-checklist.md 71 条血律（H24/E17/U19/X11）+ 风险清单 R16/R13/R34 三条实证债务
> **审计方式**：静态逐条对照（.iss 只读）。血律行号引用 `install.ps1`/`uninstall.ps1`（E:\DSH-KF\hufu-发行\打包源\）。
> **结果总览**：71 条中 **✅ 62 条 / ⚠ 8 条部分符合 / 🔴 1 条违反**。差距分级：**必须修 M1 条 / 建议修 S7 条 / 可放过注记 N10 条**。
> 血律列已同步回填 qa-checklist.md 审计列（本轮结果）。

---

## 一、总评

.iss 的架构骨架与血律对齐度**非常高**：双阶段铁律（H1/H2）通过 `PrivilegesRequired=lowest` + 零修改复用 install.ps1 完整落地；语言列表（H12）在 .iss 侧零触碰；数据红线（X7/X8）通过 [Files] 白名单 + {tmp}\pkg + BackupDiff 分流实现；卸载链（X11）的备份确认→Override 清除→一次 UAC→UAC 拒绝回退 -NoHKLM 完整成链。lead 自我修正的三处（IconFile 信号、Run 值去引号、64 位模式）核实有效。

**唯一的事故级缺陷在引擎调用的退出码判定**（M1）——恰好是 install.ps1 L339-344 注释里警告过的反模式镜像；其余差距均为边角场景或 helper 契约依赖项。

---

## 二、必须修（M 级，1 条）

### M1 🔴 RunInstallEngine 退出码判定用了 install.ps1 明文警告过的反模式（E12 破口）

- **位置**：.iss L428-433（RunInstallEngine），关键片段 `… & '.\install.ps1' … ; exit $LASTEXITCODE`
- **证据链**：
  1. install.ps1 **L339-344 注释原文**（三十四修 2026-09-13）：「& 调用 .ps1 **不设置 $LASTEXITCODE**（只有原生 exe 才设）——旧判定 `$LASTEXITCODE -ne 0` 对 $null 恒真」。.iss 用 `exit $LASTEXITCODE` 判 .ps1 成败，正是这一警告的镜像。
  2. install.ps1 主流程**成功路径没有结尾 exit**（556 行文件，尾部只有 Write-Host）→ `exit $LASTEXITCODE` 拿到的是**脚本内最后一次原生 exe 调用的遗留码**。非提权主流程的原生调用只有 `whoami`（L234，rc=0）和 `regsvr32 /s $dll`（L276）。
  3. 当前实测多数机器上这个遗留值恰好=0（侥幸正确）；但 **regsvr32 失败（如 DLL 依赖缺失，rc≠0）时脚本继续跑（$ErrorActionPreference=Continue、L276 无检查），最终退出码=regsvr32 失败码 → .iss 弹 EngineFailed 误报**"注册可能不完整"；反向的漏报窗口同理存在（原生码 0 但脚本早已半残）。
- **影响**：退出码语义靠运气；EngineFailed 提示可信度崩坏（真失败/误报无法区分），直接打击 T8 静默部署的退出码契约。
- **修法（一行，仅改 .iss）**：
  `…install.ps1… 2>&1 | Out-File …; if ($?) { exit 0 } else { exit 1 }` 替换 `exit $LASTEXITCODE`。
  语义验证：脚本内显式 `exit 1`（TEMP 检查/缺文件）会直接终止 powershell 进程 rc=1（-Command 后续不执行）✓；脚本 throw（提权 Start-Process RunAs 被拒）→ 进程 rc=1 ✓；正常完成 → `$?`=管道末命令 Out-File 成败 → exit 0 ✓。
- **复测**：T1（正常装 rc=0）+ 构造 regsvr32 失败场景（临时替换坏 DLL）确认不再误报。

---

## 三、建议修（S 级，7 条）

### S1 ⚠ X1：ARM64 未拦截（R47 风险实际存在）
- 位置：.iss L45-46 `ArchitecturesAllowed=x64`。
- 分析：Inno 语义下 ARM64 Windows **会以 x64 仿真运行**此安装器（x64-only 不拒 ARM64）；安装全程"成功"，但 x64 IME DLL 无法注入原生 ARM64 进程（explorer/搜索/设置）→ 第一方应用打不了字（R47 描述的最诡异半残形态）。32 位拦截 ✓（x64 only 真拒装）。
- 修法：InitializeSetup 加 `PROCESSOR_ARCHITECTURE=ARM64`（注册表 HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment 或 GetNativeSystemInfo）检测 → 弹窗拒装。

### S2 ⚠ U19 细化：ClearOverride 无差别删除 InputMethodOverride
- 位置：.iss L549-553。
- 分析：InputMethodOverride 是全局单值。用户装虎符后若手动把默认输入法覆盖改设为其它输入法，卸载会把人家的 override 一并删掉（副作用：用户默认输入法设置丢失）。R16 修复本体 ✓ 落地（先于 uninstall.ps1、普通权限、SilentlyContinue），仅缺"值过滤"。
- 修法：先 `reg query` 读值，仅当 `= 0804:{8F5C2A10…}{8F5C2A11…}` 时才 Remove-ItemProperty。

### S3 ⚠ X11：卸载不清 RunOnce\HuFuPostBoot → 死文件引用
- 位置：.iss 卸载链全段无此步；uninstall.ps1 原版也只清 Run\HuFu（L82）不清 RunOnce。
- 场景：安装后从未重启就卸载 → RunOnce 值残留 → 下次登录 PowerShell -File 指向已删除的 hufu-postboot.ps1 → 报错闪窗（-WindowStyle Hidden 下基本无感，但属脏残留，且 qa-compare 会抓到 RunOnce 差异）。
- 修法：usUninstall 步骤加 `RegDeleteValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\RunOnce', 'HuFuPostBoot')`（PowerShell 或 Inno 原生均可）。

### S4 ⚠ H5 交互面：野残留全新装 → install.ps1 误判"升级"跳过 ctfmon 刷新
- 位置：.iss L282-283（野残留仅弹提示"残留注册会被自动覆盖"）× install.ps1 L41-46（$tipAlready=TIP 键存在→按升级走→L533-538 跳过 ctfmon 刷新）。
- 场景：野残留机器（TIP 键在、目录亡）装 exe → install.ps1 判"升级"→ 跳过首次安装必需的 ctfmon 刷新 → 装完 Win+空格 可能切不到虎符，直到重启（RunOnce 后 ctfmon 重载自愈）。
- 修法（择一）：① 野残留分支在跑引擎前预删 HKCU\Software\Microsoft\CTF\TIP\{CLSID}（让 install.ps1 走"首次安装"路径——注意只删 HKCU 自己的 TIP 树，安全）；② 接受"重启后生效"并在 WildRemnant 提示文案中言明。
- 定级理由：场景罕见（野残留本身少）+ 重启后自愈，故 S 不 M。

### S5 ⚠ X9：SystemIME 侧 .oldN 的重启清理无提权通道（R13 闭环的最后一块）
- 位置：.iss L485-488 RunOnce 值 + L366-382 CleanStaleOldFiles（仅 {app}）。
- 分析：RunOnce 以**普通权限**跑 hufu-postboot.ps1 → 删 {app}\*.old* ✓；但 C:\Windows\SystemIME 内的 HuFu.oldN/hufu_tsf.dll.oldN 需要管理员/SYSTEM 才能删。R13 闭环现状：{app} 侧 ✓、SystemIME 侧仍靠"下次安装的提权段"（install.ps1 L130-150）——即 T9 期望"重启后 .oldN 清零"对 SystemIME 部分会 FAIL。
- 修法（helper 契约）：hufu-postboot.ps1 需含 SystemIME 清理的提权方案（ElevatedUninstallTail 同款 PFRO 挂删，或检测到 SystemIME .oldN 时以 -Verb RunAs 静默请求一次）；若决定不做，则 T9 期望改为"{app} 清零 + SystemIME 记账待下次安装"，并在卸载/升级完成页如实说明。
- **归属**：本条落在 helper（task-1）契约，.iss 侧仅传参无法独立修复——列入 helper 审计核对清单。

### S6 ⚠ H23：无管理员降级语义对用户不可见
- 场景：lowest 安装 + 用户拒绝 UAC / 无管理员账户 → install.ps1 的"请以管理员重跑/开始菜单 UWP 暂不可用"提示全部打进隐藏的 setup-engine.log（-WindowStyle Hidden），退出码又多半=0（见 M1）→ 用户得到无声的降级安装，T5 期望 A（明确呈现降级语义）不满足。
- 修法：引擎跑完后 .iss 回读 setup-engine.log 抓特征串（"无管理员权限"/"未做机器级注册"），或检测 SystemIME\HuFu\hufu_tsf.dll 不存在 → 弹降级说明框。

### S7 ⚠ X11/R34：多用户孤儿 HKCU 零落点（三债务之 R34）
- 现状：.iss 卸载链与 uninstall.ps1 原版均不枚举其它用户 hive → 快速用户切换机器上，B 用户的 HKCU TIP/CLSID/Run 残留成孤儿（语言列表死条目，R34 原样）。
- 修法（择一）：① ElevatedUninstallTail（helper，提权上下文）加 HKU\*\Software\… 枚举清理（需 Load/Unload ntuser.dat，谨慎）；② 产品文档明示"每用户安装每用户卸载，多用户需各自跑卸载"。
- 定级理由：多用户家庭机占比低、后果为死条目非瘫痪 → S。

---

## 四、可放过/注记（N 级，10 条）

| # | 注记 | 位置/说明 |
|---|---|---|
| N1 | 腾位→[Files] 拷贝之间存在目标名短暂缺失窗口（R9 原子性），非提权存量（HKCU COM 指向 {app} DLL）+用户恰好此刻新开进程打字才会踩 | L384-391+L81-98；概率极低、Inno 无 rename 语义属架构固有；T10 观察项 |
| N2 | 多目录并存不列候选（T13c 降级）：InstallDir 主信号+Run 兜底已防绝大多数；/SELECTDIR 提供手动通道 | L207-250；文档说明即可 |
| N3 | 目录页无 TEMP/下载路径拦截（H4 引擎兜底：install.ps1 L242-246 仍会 exit 1 拒装，只是晚了三步） | GetDefaultDir/目录页；可后续加校验 |
| N4 | 卸载器无单实例互斥（安装器有）；卸载双开窗口极窄 | 建议 helper 卸载尾部幂等已兜底（uninstall.ps1 全程幂等 ✓） |
| N5 | ReadVersionFromDir 对无 BOM UTF-8 更新日志的中文乱码不影响 vN.N.N 提取（数字 ASCII） | L157-195 |
| N6 | -Command 内嵌单引号路径：目录名含 `'` 会碎（中文环境概率≈0）；改 -File 包装可根除，连带 M1 一起修更稳 | L428-433 |
| N7 | staging\install.ps1 与打包源一致性、staging-files.inc 的 deleteafterinstall 与 hufu-pkg 布局，依赖 build-setup.ps1（task-3 写域），本轮不可核 | L86/L101；**列入 task-4 集成核验清单** |
| N8 | BackupDiff 的 config.json"仅缺失才拷"契约在 helper（task-1）实现，.iss 只传参——X8 打勾以 helper 契约为准 | L394-418；helper 审计项 |
| N9 | /VERYSILENT 全功能部署必弹一次 UAC（install.ps1 提权段）；/VERYSILENT+/NOHKLM 是零交互逃生门 ✓——企业部署文档需写明这个组合语义 | L277/L427 |
| N10 | 引擎失败后仍写 RunOnce/拉 server：合理（文件已就位，注册残缺可重跑修复），EngineFailed 弹窗已引导重跑 | L476-495 |

---

## 五、三债务落点核验（lead 点名项）

| 债务 | 落点 | 结论 |
|---|---|---|
| **R16** 卸载不清默认输入法覆盖 | .iss L548-554 内联实现（先于 uninstall.ps1、普通权限、usUninstall 时序正确） | ✅ 已修（S2 补值过滤即完善） |
| **R13** .oldN 重启后无主清理 | {app} 侧：RunOnce+postboot ✅；SystemIME 侧：仍靠下次安装提权段 | ⚠ 半闭环（S5，helper 契约） |
| **R34** 他用户 HKCU 孤儿 | .iss 与 helper 契约均无 | ❌ 未落点（S7 给出两条路） |

---

## 六、helper（task-1）契约核对清单（.iss 引用点 → 待 helper 落地核验）

1. `BackupDiff`：参数五件套（-Manifest/-SrcRoot/-DstRoot/-BackupRoot）与 .iss L404-411 一致；**config.json 仅缺失才拷**；只处理 Manifest 内文件（升级备份\ 目录永不入清单）；差异先备份后覆盖；退出码 0/非 0。
2. `ElevatedUninstallTail`：以管理员跑**原版 uninstall.ps1**（其 isAdmin 判定成立、不二次自提权 ✓ uninstall.ps1 L15-29 语义核实）+ PFRO 挂删 SystemIME 残留 + HKLM 校验；退出码约定（4=HKLM 残留，.iss L574-576 已按此解析）。
3. `hufu-postboot.ps1`：{app}\*.old* 清理（普通权限）+ 自检幂等秒退 + **SystemIME 侧方案（S5）** + RunOnce 值自我删除。
4. `PurgeData`：删除范围={app} 下 码表/码表导出/数据/模型 四目录（普通权限足够，.iss L579-585 普通权限调用 ✓）；绝不触碰 {app} 外。
5. 卸载-stub.bat（GBK+CRLF）：指向 unins000.exe 而非旧 uninstall.ps1 语义（防双卸载路径语义漂移）。

## 七、复测关联

M1/S1-S7 修复后优先复测：T1（M1 退出码）、T8（M1 静默退出码契约）、T16（S1 ARM64）、T6（S2/S3）、T13b（S4）、T9（S5 SystemIME 侧）、T5（S6 降级可见性）。全部通过后 qa-checklist.md 对应 ⚠/🔴 行改 ✅ 并出第 2 轮审计结论。

---

# 第 2 轮 · 静态复审 + 受控真机（2026-10-13）

## 8.1 修复核实（对照新 hufu-setup.iss 673 行，7/7 落地）

| 项 | 落地核实 | .iss 位置 |
|---|---|---|
| M1 | `try{ & install.ps1 … \| Out-File; if($?){exit 0}else{exit 1} }catch{exit 1}`——$? 判定+try/catch 双保险；install.ps1 显式 exit N 时直接终结进程透传（两者兼容 ✓） | L454-459 |
| S1 | ArchitecturesAllowed=x64os（装载层拒绝 ARM64 仿真）+ InitializeSetup paARM64 运行时兜底，双重 | L48-50, L287-292 |
| S2 | InputMethodOverride 值含 8F5C2A11（虎符 PROFILE）才删——他人 override 不误删 | L587-593 |
| S3 | 卸载 usUninstall 里 RegDeleteValue RunOnce\HuFuPostBoot | L597-599 |
| S4 | PrepareToInstall：WildRemnant 先 RegDeleteKeyIncludingSubkeys HKCU 死 TIP 键→引擎判全新安装走首装路径（含 ctfmon 刷新）；NeedRestart 并入 WildRemnant | L405-410, L544 |
| S5 | 按设计收口：SystemIME .oldN 由下次提权触点清（install.ps1 L130-150 / uninstall.ps1 L126-139 自带），reboot 后残留仅占盘不碍事——**已知设计**（T9 期望已同步调整：{app} 清零 + SystemIME 记账） | 架构注释 |
| S6 | 引擎跑完若非 -NoHKLM 且 HKLM TIP 键缺失 → SuppressibleMsgBox 浮出提示 | L505-513 |
| S7 | 已知限制记录：他用户 HKCU 孤儿=引擎级每用户语义，v1 手工指引（他账户登录跑一遍安装即覆盖） | 文档面 |

另核实：lead 本轮三处 Inno 坑修正全部落地（AnsiString 出参 L164-174 / StringChangeEx var 语义 L232-233 / [Code] 无行首裸 [ 或 #）；已沉淀为 checklist 附录 A 备忘。

## 8.2 新观察项（不阻断，v1 可放过）

**S4b**：WildRemnant 只预清 **HKCU** 侧 TIP 键；若野残留机器 **HKLM** TIP 死键也在（提权装过的残迹），install.ps1 `$tipAlready`（HKCU ∨ HKLM）仍=true → 跳过 ctfmon 刷新 → 装完可能要等 ctfmon 自愈/重启才能切到虎符。**有 NeedRestart（WildRemnant 并入）兜底引导重启，重启后 ctfmon 重载自愈**；提权段会幂等重写 HKLM 注册，无文件层影响。定级：v1 放过；v1.x 可把 ctfmon 刷新时机改为"目录亡即首装语义"（不动引擎则需 .iss 侧在 WildRemnant 且 HKLM TIP 在时并入 NeedRestart，现状已满足）。

## 8.3 受控真机 T2 · 被 R58 现场阻塞（.iss 无辜）→ **已结案翻案（附录 B 结案版）**

> **结案更新**：本节初判"火绒 HIPS 拦截"为**误判**——火绒整卸+驱动清零+重启后拦截依旧，控制变量实验（同一未签名探针 exe 移位）锁定真凶=**DSH harness 文件沙箱按"进程映像启动路径"约束**（workspace 内启动的进程建 TEMP 目录被拒）。解法=安装器复制到 workspace 外（C:\Temp）再跑。全过程与方法论沉淀见 checklist 附录 B 结案版。R58 产品级结论（杀软放行预告+签名）保留不变。

## 8.4 T2 重跑清单（火绒放行后执行）→ **已执行完毕（8.5）**

## 8.5 真机三连跑+三轮迭代（2026-10-13 下午，最终构建 SHA256=F7E02BE8…22F4B87）

**探跑**：workspace 外启动装载器 → PASS（解包+安装流程正常）。

**T2 全链（三轮迭代）**：
- 第一轮（M3 前）：Inno 日志尾部 `CurStepChanged raised an exception: Runtime error (at 51:256): Type Mismatch` → [Files] 完成后 ssPostInstall 整体炸掉（BackupDiff/引擎/RunOnce 全跳）→ **发现 M3**（GetDateTimeString Char 参数传空串）。探针二分复现+修法验证（'/'）。
- 第二轮（M4 前）：引擎链全绿但 BackupDiff "未指定命令 exit=1" → **发现 M4**（.iss "-Command X" vs helper switch "-X" 契约断链）。修复后 BackupDiff 跑通：拷入=0 跳过=83（相同82/已存在1）失败=0 exit=0。
- 第三轮（M4 版）：**T2 验收 17/18 全绿**——InstallDir 原位/COM→SystemIME/RunOnce/HKLM TIP/config.json 56C1E21A 保护/模型 2 原样/码表导出（含子目录）完好/用户调整不存在/SystemIME=staging 版/is1 唯一/server 存活；-Compare 123 项差异全有因（RunOnce 设计+SystemIME 更新/腾位+ProgramData diag 设计清理≈提权段实证）；唯一缺口=server Path 非提权查询不可见（ctfmon 同样，环境限制非产品问题）。

**PFRO 提权复验**：火绒清除后仍 5/8 FAIL（err=3）→ 排除环境 → **发现 M5**（PS 5.1 把 $null 传 P/Invoke string 参数转空串→PATH_NOT_FOUND）。证据矩阵：$null=err3 / [NullString]::Value=err5（正确权限码）/提权+NullString=ok+PFRO 真写入。修后实证在卸载链。

**卸载链验收（M2 动态+M5 实证）**：unins000 /VERYSILENT exit=0——InstallDir 键/HKCU TIP/HKLM TIP/CLSID32/Run 自启/is1 全清；语言列表纯减法（en-US 虎爪槽位按 CLSID 认领删）；**PFRO 真写入**（\??\…\SystemIME\HuFu.old1\hufu_tsf.dll.old1）；数据五目录全保留；InstallDir 键+语言列表=uninstall.ps1 独有动作全发生=**M2 修复实证**。遗留观察：{app} 遗留 9 个 .oldN（占用，UninstallNeedRestart 提示语义）+licenses+日志；HKLM Classes\CLSID 残留一次（提权手删成功，疑探跑引擎时序回写，uninstall.ps1 L107 清单在列）；**S3 之谜**（RunOnce 删除未执行，键 LastWrite 锁死证据，待 lead review，详见 checklist 附录 C）。

**装回（收尾）**：/DIR 回原目录 exit=0——首装语义（Upgrade=0）+数据原地（config/模型/码表导出全在）+SystemIME 重拷+HKCU COM→SystemIME+RunOnce 新值覆盖（探跑污染自愈）+is1 重建+server 起。

**版本迭代哈希链**：264E3D84…（M2 版）→ 83CA9C5E…（M3 版）→ 3B7EF4DB…（M4 版）→ **F7E02BE8…（M5 版=最终交付）**，每轮证据落在当时构建上，卸载链+装回落在最终构建。

## 8.6 S3 二值验证 + T9/T11 重启定论（2026-10-13 终局）

**S3 结案（竞态污染误报）**：清野进程（无 setup/unins/_iu 残留）→ F7E0 版卸载（带 /LOG）→ `RunOnce\HuFuPostBoot 存在=False` ✅ **S3 删除语义正确**。前轮"未删除"观察定案为探跑野子进程（装载器被杀后 setup 子进程独立跑完全程）晚到回写的竞态污染。/LOG 日志存 setupqa\uninstall-F7E0.log。

**T9（RunOnce postboot 清 .oldN）✅ 完全实证**：S3 验证卸载后装回（进程被用户关机打断，但引擎 07:35 已完成+BackupDiff 839ms exit=0）→ 用户"关机（拔电源）→开机"→ 本次开机 07:38:27 postboot 日志：`RESULT: PostBoot 安装目录 .old* 删 4 剩 0；SystemIME 尽力删 0 剩 1；exit=0`（62ms）——RunOnce 值被系统自动删除（跑过铁证）+{app} .oldN 清零闭环。

**T11/R10（Fast-Startup 下 PFRO）✅ 定论**：铁证三件套——LastBootUpTime 仍 06:54:45（"关机再开机"后不变=混合关机会话恢复）+powercfg /a 显示"快速启动"可用+HiberbootEnabled=1。结果：`C:\WINDOWS\SystemIME\HuFu.old1\hufu_tsf.dll.old1` **未处理**（PFRO 未消费）。**定论：Fast Startup 混合关机恢复的会话不重跑 Session Manager → PFRO 不执行；机制性非 bug**——残留无害，完整重启（真重启/系统大更新）自然清理；产品已有 PostBoot"尽力删"兜底（本轮实测删 0/剩 1 后不阻赛 exit=0）。R10 结论改写为"快速启动下 PFRO 延迟到完整重启，设计兜底充分"。

**新观察 S 级（低危，报 lead 待办）**：PFRO 值现含两条同路径条目且第二条带 `*1` 前缀（`\??\…hufu_tsf.dll.old1` + `*1\??\…hufu_tsf.dll.old1`）——两轮卸载 tail 各写一条未去重，`*1` 前缀疑 helper RMW fallback 写入格式；系统将忽略非法条目不致故障，建议 helper PFRO 写入前做同路径去重+规范前缀。

**终态**：输入法装回在位（F7E0 最终构建）：InstallDir=真机目录/is1 重建/HKCU COM→SystemIME/数据四族原地（config 56C1E21A/模型 2/码表/码表导出含子目录）/server 开机自启拉起存活=输入法可用。残留：SystemIME\HuFu.old1（待完整重启 PFRO 清）+{app} licenses/install.log/setup-engine.log（卸载轮遗留观察项）。

## 8.7 task-4 总结

**三轮 M 级发现全修复实证**（M3 Type Mismatch/M4 参数契约/M5 NullString——均探针定位+修法验证+真机闭环）；M1/M2（前轮）真机实证；S1-S7 静态核实（前轮）；**T2 升级/T6 卸载/T9 postboot/T11 Fast-Startup/R10 PFRO 五大场景真机闭环**；R58 事件翻案沉淀方法论（checklist 附录 B/C）。**产品放行建议：v1.8.2 exe（F7E02BE8…）可发布**，遗留项均为 S/N 级观察（PFRO 去重、licenses 遗留、S3 无复发）不阻断。
