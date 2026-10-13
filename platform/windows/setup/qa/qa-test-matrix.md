# qa-test-matrix.md — HuFu exe 安装器 · 受控真机测试矩阵

> **目的**：需求①-⑤ + 血律清单（qa-checklist.md H/E/U/X 71 条）的动态验收。
> **统一取证方法**（每个场景一致）：
> 1. 测试前：`powershell -File qa\qa-backup-state.ps1`（记录生成的 backup-<时间戳> 目录）
> 2. 执行场景步骤
> 3. 测试后：`powershell -File qa\qa-restore-state.ps1 -BackupDir <那个目录> -Compare`
>    → `compare-report.txt` 的每条差异即"安装器动了什么"的客观证据，与"期望"逐项比对
> 4. 附加取证：`{app}\install.log`（install.ps1 留证行）、Inno 安装日志（装时加 `/LOG=E:\DSH-KF\.testtmp\setupqa\setup-<场景>.log`）、`Get-Process hufu-server | select Path`（H2/H17 令牌验证）
>
> **测试机要求**：可控 Win10/11 x64 真机（非日常用机；或本开发机但接受回滚成本）。
> 本开发机现成素材：zip 存量 v1.8.2-无模型 真机安装于 `D:\HUFU\HuFu虎符输入法-v1.8.2-无模型\`（T2 直接可用）。
> **回归顺序建议**：T1→T4→T3→T2→T10→T9→T11→T5→T13→T14→T15→T17→T6→T7→T8→T12→T16。
> 每场景通过标准=全部"期望"项命中；任一未命中 → 记差距（关联血律编号）回报 lead。

---

## T1 全新装（无模型包）

| 项 | 内容 |
|---|---|
| 前置 | 干净系统：无 HKCU\Software\HuFu、无 HKLM CTF\TIP\{8F5C2A10…}、无 SystemIME\HuFu、无 Run\HuFu（-Compare 应先证零基线）；有默认输入法+至少一个第三方输入法在列 |
| 步骤 | ①qa-backup-state ②双击 exe ③目录页接受默认 ④一路下一步 ⑤完成页选"稍后重启" ⑥新开记事本 Win+空格切虎符打字 ⑦开始菜单搜索框打中文 ⑧-Compare |
| 期望 | A.全程只弹一次 UAC（install.ps1 内部提权段）；B.装后立即可打字（记事本候选正常）；C.搜索框可打中文（E2 AAP ACL 生效）；D.他人输入法零丢失（-Compare：hkcu-uprofile/hkcu-ctf 他人条目无 REMOVED，H12）；E.HKCU COM InprocServer32=SystemIME（H7/E13）；F.{app}\install.log 有"install CTF原生启用… 丢失=[]"行（H14）；G.RunOnce\HuFuPostBoot 已注册（X9）；H.Apps&Features 出现虎符条目（需求④）；I.完成页有"立即重启/稍后重启"二选一（需求⑤/X10 全新装无腾位→NeedRestart 期望为否——注意：全新装无需重启提示是否出现需与 lead 确认语义） |
| 取证 | compare-report.txt、setup-T1.log、install.log、Apps&Features 截图 |

## T2 zip v1.8.2 存量升级（锁定旧目录·数据白名单·升级备份）

| 项 | 内容 |
|---|---|
| 前置 | zip v1.8.2-无模型 已装且能打字（本机 D:\HUFU\… 即可）；**先造用户数据**：码表\虎整句\用户调整.txt 写入标记行"QA-MARK-001"；数据\config.json 改一个可核对值（如皮肤键）；码表导出\ 放一个假导出文件 |
| 步骤 | ①qa-backup-state ②双击新版 exe ③观察目录页 ④完成 ⑤-Compare ⑥核对用户数据 |
| 期望 | A.目录页**跳过**且显示旧目录 D:\HUFU\…（X5/H5）；B.用户调整.txt 的 QA-MARK-001 原样在（X8 数据白名单）；C.config.json 保留用户改动值（X8"仅缺失才拷"）；D.码表导出\假文件在（U15 语义前移）；E.{app}\升级备份\v1.8.2-<日期>\ 存在且含被覆盖的旧程序文件（X8 BackupDiff）；F.升级安装日志出现"升级安装：跳过 ctfmon 刷新"（H18）；G.同机第三方输入法/虎爪存活（H12/H18）；H.装完即可打字、搜索框可用；I.RunOnce 已注册、{app} 与 SystemIME 无 .old* 残留（无占用时应零腾位） |
| 取证 | compare-report.txt、升级备份目录清单、install.log、用户数据三处截图 |

## T3 exe→exe 升级

| 项 | 内容 |
|---|---|
| 前置 | T1 装好的 exe 版（假想 v1.9.0）→ 再跑更新版 exe（v1.9.1+），装时同样预置用户数据标记 |
| 步骤 | 同 T2，另检查 Apps&Features 条目数 |
| 期望 | A-E 同 T2；F.Apps&Features 仍**只有一条**虎符（R20 幽灵条目防线）；G.卸载入口指向新 unins000.exe 可用；H.旧版程序文件被替换（-Compare FILE-CHANGED 仅程序文件，数据文件零 CHANGED） |
| 取证 | compare-report.txt、Apps&Features 截图 |

## T4 同版本重装（修复语义）

| 项 | 内容 |
|---|---|
| 前置 | exe 版已装，直接再跑同版本 exe |
| 步骤 | ①qa-backup-state ②同版本 exe ③观察"已安装"交互 ④完成 ⑤-Compare |
| 期望 | A.交互明确（修复/退出，不误导）；B.修复=幂等覆盖+注册重写，**不额外要求重启**（文件未变无腾位，X10）；C.数据零触碰（X8）；D.装后输入法立即可用 |
| 取证 | compare-report.txt（期望差异≈0 或仅时间戳级）、setup-T4.log |

## T5 无管理员（-NoHKLM 降级路径）

| 项 | 内容 |
|---|---|
| 前置 | 标准用户账户（非管理员组，或 UAC 被域策略压制）；系统无机器级底座（或分两组：无底座/有底座） |
| 步骤 | ①qa-backup-state ②以标准用户跑 exe ③观察降级提示 ④完成 ⑤新开应用打字 ⑥开始菜单搜索打字 |
| 期望 | A.安装器不静默失败，明确呈现降级语义（H23）：无底座→提示需管理员重跑；有底座→提示每用户即全功能；B.普通应用（记事本）**可打字**（H23/E15 回退用户目录 DLL）；C.搜索框预期不可用且提示覆盖（H23 已知限制明示）；D.不弹 UAC 或 UAC 拒绝后仍走完每用户段 |
| 取证 | compare-report.txt（hkcu-* 有写入、hklm-* 零变化）、降级提示截图 |

## T6 卸载保留数据（需求④主场景）

| 项 | 内容 |
|---|---|
| 前置 | T2/T3 升级后的机器，用户数据标记仍在 |
| 步骤 | ①qa-backup-state ②设置→应用→虎符→卸载 ③弹码表备份确认（先点"尚未备份"分支 → 应打开码表文件夹并中止）④再卸载并确认已备份 ⑤观察数据确认（默认否）⑥完成 ⑦-Compare |
| 期望 | A.未备份分支=打开 码表\ 文件夹且卸载**中止**（X11）；B.一次 UAC 跑原版 uninstall.ps1（U2）；C.卸载后 码表\（含用户调整.txt 标记）与 码表导出\ 完整保留（U15/需求④）；D.数据\config.json 保留；E.程序文件全删（-Compare FILE-REMOVED 仅程序白名单）；F.HKCU/HKLM 注册全清（hkcu-hufu=GONE、hklm-*=GONE，U7/U10）；G.SystemIME\HuFu 清除（占用场景另见 T10，U11）；H.**InputMethodOverride 已清**（-Compare hkcu-uprofile 有该值移除差异——U19/R16 修复验证）；I.默认输入法自动回落可用（能正常打中文）；J.完成页列出保留数据路径+文件计数（U18/需求④）；K.卸载后 ctfmon 在跑、其它输入法正常（U17） |
| 取证 | compare-report.txt、保留目录截图、完成页截图 |

## T7 卸载 /PURGEDATA（彻底清除）

| 项 | 内容 |
|---|---|
| 前置 | 同 T6 前置（数据标记在） |
| 步骤 | ①命令行跑卸载程序带 /PURGEDATA（Inno 卸载参数）②确认各交互 ③-Compare |
| 期望 | A.程序+数据全删：码表\码表导出\数据\ 目录整体消失（-Compare DIR-GONE/FILE-REMOVED 覆盖数据文件）；B.注册表同 T6-F/G/H；C.无"确认已备份"强阻断绕不过（PURGEDATA 语义=用户明确要全清，交互可简化但数据确认应有） |
| 取证 | compare-report.txt、目录不存在截图 |

## T8 静默 /VERYSILENT

| 项 | 内容 |
|---|---|
| 前置 | 干净机或存量机两组 |
| 步骤 | ①qa-backup-state ②`HuFuSetup.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /LOG=…` ③等待退出码 ④-Compare |
| 期望 | A.退出码 0；B.零交互完成（无 UAC 以外弹窗；提权段 UAC 在 /VERYSILENT 下的表现需与 lead 确认：lowest 模式下 install.ps1 提权段 UAC 仍会弹——静默部署语义=X 确认）；C.装后功能同 T1-B/C；D.静默**卸载**分支：数据保留策略默认生效（X38 对应：静默卸载不做备份强阻断，默认保留数据——R38 语义） |
| 取证 | 退出码、compare-report.txt、setup-T8.log |

## T9 "稍后重启"后 PFRO/RunOnce 生效（.oldN 清零）

| 项 | 内容 |
|---|---|
| 前置 | 先经历 T10（制造腾位），保持不重启 |
| 步骤 | ①确认 SystemIME 与 {app} 存在 .old* ②重启（真重启）③登录后等 1-2 分钟 ④-Compare ⑤再查 .old* |
| 期望 | A.重启后 {app}\*.old* 与 SystemIME 内 HuFu.old* / hufu_tsf*.old* **全部清零**（X9 RunOnce HuFuPostBoot 执行，R13 修复验证）；B.RunOnce\HuFuPostBoot 值已被消费消失；C.输入法重启后立即可用（H21 开机竞态允许短暂延迟，记录从登录到可打字秒数）；D.默认输入法重启后仍是虎符（H15） |
| 取证 | 重启前后两次 -Compare、.old* 目录列表（空）、开机到可打字耗时记录 |

## T10 PE 被占用腾位（预开记事本+切虎符制造加载）

| 项 | 内容 |
|---|---|
| 前置 | 已装旧版；开 3 个记事本各切到虎符（DLL 已加载）；开开始菜单搜索一次（SearchHost 加载 SystemIME 副本）；保持应用全开 |
| 步骤 | ①qa-backup-state ②跑新版 exe 升级 ③观察 PrepareToInstall ④完成（选稍后重启）⑤-Compare ⑥检查腾位文件 |
| 期望 | A.安装不因文件占用失败（X6 腾位）；B.{app} 内 5 个 PE 全部 .oldN 腾位、新文件就位（顺序=旧改名→新拷贝，目标名无缺失窗口，R9）；C.SystemIME 副本腾位由 install.ps1 提权段完成（E1）；D.NeedRestart=真→完成页二选一（X10）；E.装完**新开**应用即可用新版打字；F.已开记事本（旧 DLL）在重启前行为：可继续打字或明确降级，**不得出现打字瘫痪**（R8 混跑协议验证）；G.腾位计数与 RunOnce 清扫项一致 |
| 取证 | compare-report.txt、.oldN 清单、已开/新开应用行为记录 |

## T11 R10 快速启动专项（真重启 vs 关机再开）

| 项 | 内容 |
|---|---|
| 前置 | 两轮：轮 A=真重启（开始菜单→重启）；轮 B=关机再开机（快速启动默认开，`powercfg /a` 确认 hiberboot）；均先经 T10 制造腾位+PFRO 挂删项 |
| 步骤 | 轮 A/轮 B 分别：①记录 SystemIME HuFu.old* 与 PFRO 值（`reg query …\Session Manager /v PendingFileRenameOperations`）②执行对应重启方式 ③登录后核对 |
| 期望 | A.轮 A（真重启）：PFRO 执行、.old* 按挂删项清零（RunOnce 兜底清剩余）；B.轮 B（快速启动）：**预期 PFRO 不执行**（R10【待实测】项的正式验证点）→ RunOnce 自检兜底应仍清掉 {app} 与可达的 .old*，SystemIME 内被挂删项可能残留 → 记录实际行为定结论；C.两种方式下重启后输入法均可用；D.结论回填风险清单 R10（兑现或证伪） |
| 取证 | 两轮的 reg query 前后值、.old* 前后清单、powercfg /a 输出 |

## T12 杀软在场（Defender）首装体验

| 项 | 内容 |
|---|---|
| 前置 | 干净机+实时保护开；给 exe 构造 MOTW：`Set-Content -Path HuFuSetup.exe -Stream Zone.Identifier -Value "[ZoneTransfer]`nZoneId=3"` |
| 步骤 | ①双击带 MOTW 的 exe ②观察 SmartScreen ③放行后安装 ④观察安装期 Defender 行为 ⑤装后打字验证 |
| 期望 | A.SmartScreen 全屏拦截出现（R56 复现）→ 按"更多信息→仍要运行"可继续；B.安装期无 Defender 隔离核心 DLL（hufu_tsf.dll/SystemIME 副本在，E1/E2 自检通过）；C.若被隔离：安装自检给出"被安全软件移除"提示而非装完瘫痪（R22 缓解验证）；D.安装完成页/首屏有安全软件放行预告文案（R58 缓解验证） |
| 取证 | SmartScreen 截图、Defender 保护历史截图、compare-report.txt |

## T13 检测专项（3 子场景）

| 子场景 | 前置构造 | 步骤 | 期望 |
|---|---|---|---|
| T13a InstallDir 丢失 | `reg delete HKCU\Software\HuFu /v InstallDir /f`（保 TIP 键与目录） | 跑 exe | 多信号兜底仍判升级（TIP/Run/IconFile 交叉，X4/R41）→ 锁定旧目录，不判新装、不出双条目 |
| T13b 野残留（注册在目录亡） | 把安装目录整体改名（模拟手删，R36） | 跑 exe | 识别"注册在目录亡"→ 引导清理旧注册（或先卸后装）→ 按新装走且装完可用；**不得**装出指向死路径的混合态 |
| T13c 多目录并存 | 手工造第二目录含程序文件+把 InstallDir 指向它 | 跑 exe | 检测列出两个候选让用户确认主目录（X4/R75）；清理非主目录程序文件但保留其数据（如 lead 定义的语义） |

| 共同取证 | 每子场景前后 -Compare + Apps&Features 条目数截图（期望恒=1） |

## T14 安装器双开互斥

| 项 | 内容 |
|---|---|
| 前置 | 无 |
| 步骤 | 快速连点 exe 两次 |
| 期望 | B.第二实例检测到互斥后置前已有实例并退出（X2/R63），不产生双拷贝/双 server/双 UAC |
| 取证 | 进程列表（只有一个 setup 实例）、setup 日志只有一份 |

## T15 中途强退自愈（模拟断电）

| 项 | 内容 |
|---|---|
| 前置 | 干净机或存量机 |
| 步骤 | ①跑 exe 到文件拷贝中段（进度 30-60%）任务管理器强杀 ②检查状态 ③重跑 exe ④完成 ⑤-Compare |
| 期望 | A.半安装态不崩溃系统输入（其它输入法正常）；B.重跑自愈：幂等覆盖至一致（R64）；C.最终装完功能完整、Apps&Features 恰一条（R20）；D.半途卸载态专项（可选）：卸载中途强杀 → 重跑卸载可完成（U 节幂等） |
| 取证 | 强杀时点截图、重跑 setup 日志、compare-report.txt |

## T16 架构门禁（32 位 / ARM64 拦截）

| 项 | 内容 |
|---|---|
| 前置 | 静态审查 + 若有 32 位 VM 实测 |
| 步骤 | ①静态：审计 .iss ArchitecturesAllowed/ArchitecturesInstallIn64BitMode 与 [Code] 版本门禁（Win10 1607 build 14393）②（可选）32 位 VM 跑 exe |
| 期望 | A.32 位机：拒绝安装+明确文案（R46），不出现"装成功但打不了字"；B.ARM64 机：拦截或明确不支持（R47）；C.旧 Win10（<1607）/Win7：拦截文案一致 |
| 取证 | .iss 相关段引用、（若有）VM 拦截截图 |

## T17 升级时正在打字（候选窗开着）

| 项 | 内容 |
|---|---|
| 前置 | 记事本切虎符，输入未上屏拼音串让候选窗保持打开 |
| 步骤 | ①该状态下跑升级 exe ②观察杀 server 瞬间 ③升级完成 |
| 期望 | A.在打内容丢失范围最小化（组合取消可接受，进程不得崩溃，R68）；B.升级后新开应用可打字；C.若有"3 秒倒计时提示"类缓解（X 确认项）则验证其出现 |
| 取证 | 升级瞬间录屏/截图、setup 日志 |

---

## 场景 ↔ 血律覆盖对照（防漏测）

| 血律节 | 覆盖场景 |
|---|---|
| H1-H24 | T1（主）、T2/T3（H5/H18 升级语义）、T5（H21/H23）、T17（H2/H17） |
| E1-E17 | T1（E2 搜索框/E8-E11 reg 全集）、T10（E1 腾位）、T2（E5-E7 清理） |
| U1-U19 | T6（主）、T7、T8（静默语义）、T9（U11+RunOnce 兜底） |
| X1-X11 | T13（X4）、T14（X2）、T15（X9 幂等）、T10/T11（X9/X10）、T6/T7（X11）、T16（X1） |
| 风险清单回填 | T11 → R10 定论；T9 → R13 验证；T12 → R56/R58；T13b → R36 |

## 记录模板

每场景一份 `T<n>-result.md`：执行日期/机器/版本、期望项逐条 ✅/❌、compare-report 差异摘要、异常截图链接、结论（通过/差距+关联血律编号）。
