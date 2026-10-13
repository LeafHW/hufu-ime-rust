# qa-checklist.md — HuFu exe 安装器 · 血律审计清单

> **用途**：把 install.ps1 / uninstall.ps1 里沉淀的全部实测铁律（"血律"——每条背后都是一次真机事故）
> 逐条映射到 exe 方案的负责环节，等 lead 的 `hufu-setup.iss` 落盘后逐条审计打勾/标红并回报差距。
>
> **审计方法**：`.iss` 落盘后，按"责任方"列检查对应代码段是否存在且语义等价（不是形似）；
> 需要运行验证的按"验证方法"列在受控真机执行（先 `qa-backup-state.ps1` 备份，测后 `-Compare` 取证）。
>
> **行号基准**：`E:\DSH-KF\hufu-发行\打包源\install.ps1`（556 行）/ `uninstall.ps1`（206 行）。
> exe 方案对 install.ps1 的要求是**零修改复用**（血律天然继承），审计重点=调用时机/顺序/参数与新增环节是否破坏血律。
>
> 严重度标记：🔴 破坏即事故级（复现历史实测故障）｜🟡 功能退化｜🟢 体验/规范。

---

## H 节 · install.ps1 安装铁律（24 条）

| # | 行号 | 铁律内容 | exe 方案责任方 | 验证方法 | 审计结果 |
|---|---|---|---|---|---|
| H1 🔴 | 3-8 | 双阶段总纲：文件/HKCU/语言列表/自启/server **永远普通权限**；提权子进程只做 HKLM+SystemIME+msctf | Inno `PrivilegesRequired=lowest` + install.ps1 零修改复用（自带 -PhaseElevated） | 审计 .iss 无任何提权启动 install.ps1 主流程的路径；装后进程快照确认 server 普通权限令牌 | ✅ |
| H2 🔴 | 4-5 | server 绝不提权启动：提权启动锁命名管道 ACL → 所有普通应用连不上 → 只能打字母（2026-08-29 实测） | ssPostInstall 调 install.ps1（其 L513-516 explorer 中转降权）；完成页绝不直接 Run server | 装后 `Get-Process hufu-server` 属主为当前用户非管理员；管道连通（记事本可出候选） | ✅ |
| H3 🟡 | 20-27 | 原地安装 $inst=$src；数据（数据/码表/码表导出/模型）住安装目录 | exe 拷贝程序+数据到 {app} 后由 install.ps1 在 {app} 内跑 | 装后 {app}\数据\config.json 存在；HKCU InstallDir={app} | ✅ |
| H4 🟡 | 242-246 | 拒绝 TEMP 目录安装（exit 1 + 提示移稳定位置） | exe 天然免疫（程序落 {app}）；目录选择页仍应拦 TEMP/下载路径 | 目录页输入 %TEMP% 路径 → 警告/拒绝 | ⚠ N3 |
| H5 🔴 | 41-46 | 升级检测=本 TIP 键已存在（HKCU 或 HKLM）→ 按"升级"语义走 | .iss 检测段（多信号）判定升级时锁旧目录；install.ps1 自身检测只用于 ctfmon 刷新决策 | 存量机上跑安装器 → 目录页被跳过/预填旧目录 | ✅ |
| H6 🔴 | 74-82 | smoke msctf 登记 60 秒超时杀进程（CTF 服务未就绪会无限挂死安装窗口，三十四修补） | install.ps1 零修改复用（自带 Invoke-SmokeReg） | 代码审查 .iss 不内联重写登记逻辑 | ✅ |
| H7 🔴 | 275-281 | 顺序铁律：regsvr32 必须先跑（其会覆盖 HKCU CLSID 为安装目录路径）→ 之后重写为 SystemIME 副本 → 最终值必须落 SystemIME + 回读双保险 | install.ps1 零修改复用 | 装后 `HKCU\Software\Classes\CLSID\{8F5C2A10…}\InprocServer32` 默认值 = C:\Windows\SystemIME\HuFu\hufu_tsf.dll | ✅ |
| H8 🔴 | 283-303 | HKCU TIP 键树全套不可省（切换器/搜索框生命线，afc2dfc 曾误删致 Win+空格切不到）：Description/Category 键树/LanguageProfile Enable(DWORD)/IconIndex/IconFile/Icon 双写 | install.ps1 零修改复用 | qa-backup-state -Compare：装前后 hkcu-ctf 树新增 TIP\{CLSID} 全套键 | ✅ |
| H9 🔴 | 294-297 | 键盘分类 {34745C63} **只写 HKLM 全局库**，勿在 HKCU TIP 树写（msctf 判非法周期删除，净室实测稳定复现） | install.ps1 零修改复用；.iss 不得"补写" | 装后 HKCU TIP 树无 34745C63 键；HKLM 全局库有 | ✅ |
| H10 🟡 | 304-306 | 不注册 DisplayAttribute 键（自定义标记反致 QQ/开始菜单/记事本不画下划线） | install.ps1 零修改复用；.iss 不加写 | reg query HKCU\…\DisplayAttribute 无虎符键 | ✅ |
| H11 🔴 | 313-320 | 32 位宿主：HKCU Wow6432Node\CLSID 视图注册（提权装→SysWOW64 副本路径；否则安装目录 32 位 DLL） | install.ps1 零修改复用；exe 程序文件须包含 hufu_tsf32.dll | 装后 HKCU\Software\Classes\Wow6432Node\CLSID\{CLSID}\InprocServer32 指向 SysWOW64\SystemIME | ✅ |
| H12 🔴 | 383-453 | 语言列表只走 CTF 原生 API（smoke enable=EnableLanguageProfile）；**禁** Get/Set-WinUserLanguageList 整表重写（吞他人输入法，他机实锤）；**禁** 裸写注册表真存储（分钟级被 msctf 拔除，18:53 实锤）；失败=提示手动设置页添加，不裸写兜底 | install.ps1 零修改复用；.iss/自定义页**绝不**触碰语言列表 | 机器预装第三方输入法（虎爪/搜狗）→ 装虎符后他人输入法全部在场（-Compare uprofile/ctf 树他人条目零变化） | ✅ |
| H13 🟡 | 456-461 | ctfmon 未运行则先冷启动（语言列表读取之前） | install.ps1 零修改复用 | 代码审查；升级场景见 H18 | ✅ |
| H14 🟡 | 462-485 | 装前快照/装后回读核验他人条目零变化 + install.log 留证（注册表只读，绝不作为重写原料） | install.ps1 零修改复用 | {app}\install.log 有 "install CTF原生启用… 丢失=[]" 行 | ✅ |
| H15 🟡 | 489-492 | Set-WinDefaultInputMethodOverride 显式设默认首选（仅列表首位重启后可能不是虎符，用户实锤） | install.ps1 零修改复用；卸载侧由 helper -ClearOverride 对称清理（见 X11/U19） | 装后 reg query "HKCU\Control Panel\International\User Profile" /v InputMethodOverride = 0804:{CLSID}{PROFILE} | ✅ |
| H16 🟡 | 495-508 | HKCU Run 自启 HuFu（带引号路径）+ 开始菜单 lnk（URL 127.0.0.1:4390 + 图标.ico） | install.ps1 零修改复用 | Run\HuFu 值存在且带引号；开始菜单"HuFu 虎符输入法设置.lnk" | ✅ |
| H17 🔴 | 510-518 | 启动 server：先杀已有进程；脚本被提权时经 explorer.exe 中转降权启动 | install.ps1 零修改复用（H2 的执行面） | 提权安装场景装后 server 令牌为普通权限（Process Explorer 查属主） | ✅ |
| H18 🔴 | 520-539 | 刷新纪律：只杀 ctfmon（**禁**杀 TextInputHost/ShellExperienceHost——触发 msctf 校验杀死虎爪）；升级安装（TIP 已在）跳过刷新；已冷启动跳过（每次重启 ctfmon 都是一次校验风险） | install.ps1 零修改复用；.iss PrepareToInstall 只杀 hufu-server，不碰 ctfmon/宿主 | 升级安装日志出现"升级安装：跳过 ctfmon 刷新"；同机虎爪存活 | ✅ |
| H19 🟡 | 262-264 | HKCU\Software\HuFu\InstallDir 记录（DLL 自愈链/卸载器读取） | install.ps1 零修改；.iss ssPostInstall 后回读校验 InstallDir={app}（lead 架构） | 装后 InstallDir 值 = {app} 真实路径 | ✅ |
| H20 🟡 | 325-332 | -UserOnly 开关：多用户机器新用户免提权只写 HKCU 配置 | install.ps1 零修改保留开关；.iss 不调用即可（文档保留逃生门） | 代码审查：.iss 不删改 install.ps1；-UserOnly 手测仍可用 | ✅ |
| H21 🟡 | 230-235, 336-369 | 权限三分支：isAdmin 直跑子阶段（& 调用+查 $?）；inAdminGroup 弹 UAC；两者皆无→提示+尽力而为 | install.ps1 零修改；.iss 侧对应"接受降级安装"交互 | 标准用户+管理员凭据（肩眺）场景装完当前用户 HKCU 齐全 | ✅ |
| H22 🔴 | 343-363 | 提权调度细节：固定 Windows PowerShell 路径；-PassThru 捕获退出码；子进程日志按 UTF-8 回读；提权后主流程回读校正 HKCU COM（顺序铁律的校验半边） | install.ps1 零修改复用 | 日志窗口可见"提权阶段"输出无乱码；HKCU COM 终值=SystemIME | ✅ |
| H23 🟡 | 364-381 | 无管理员降级路径：提示 + smoke 尽力而为 + hasBase 判定（SystemIME+HKLM TIP 双存在=每用户即全功能） | install.ps1 零修改；.iss 需在 UI 呈现该降级语义（见 T5） | 无管理员账户装完：普通应用可打字；提示开始菜单/UWP 需管理员重跑 | ✅S6已修(第2轮,引擎后 HKLM TIP 缺失浮出提示) |
| H24 🟢 | 541-556 | 完成语义：无需重启声明 + 已开应用重开后才加载新输入法 + 不得移动文件夹提醒 | exe 改为"立即重启/稍后重启"二选一（需求⑤）——**语义变更点**：文案须同时说明已开应用需重启才生效 | 完成页含二选一按钮 + 文案含"已开应用…"提示 | ✅ |

---

## E 节 · 提权段（-PhaseElevated）注册表与文件全集（install.ps1 L85-228，17 条）

> exe 方案责任方=install.ps1 零修改复用（一次 UAC 由其内部 -PhaseElevated 完成）。
> 审计要点：.iss 不得在提权段之外重复/抢先做这些写入（会破坏 H7 顺序铁律与 H12 纯净性）。

| # | 行号 | 内容 | 验证方法 | 审计结果 |
|---|---|---|---|---|
| E1 🔴 | 86-99 | New-Item SystemIME 目录；拷贝 hufu_tsf.dll（被占用→改名 .oldN 腾位再拷）；拷贝失败 throw | 装后 SystemIME\HuFu\hufu_tsf.dll 存在且哈希={app} 内 DLL | ✅ |
| E2 🔴 | 100-101 | icacls：目录 ALL APPLICATION PACKAGES:(OI)(CI)RX + 文件 RX（打包进程可读的生命线，缺=搜索框/UWP 打不了字） | `icacls C:\Windows\SystemIME\HuFu` 输出含 ALL APPLICATION PACKAGES 两行；开始菜单搜索框可打中文 | ✅ |
| E3 🔴 | 103-121 | SysWOW64\SystemIME\HuFu 32 位目录+DLL（腾位兜底）+icacls 同款 | SysWOW64\SystemIME\HuFu\hufu_tsf32.dll 存在+ACL 含 AAP | ✅ |
| E4 🔴 | 122-125 | HKLM Classes WOW6432Node\CLSID\{CLSID}：default/InprocServer32=SysWOW64 路径/ThreadingModel=Apartment | reg query 三值齐全 | ✅ |
| E5 🟡 | 130-150 | 升级清理 .oldN 残留（两基座 SystemIME/SysWOW64；仍占用则跳过留给下次） | 升级前预置 HuFu.old1 → 装后消失（或仍占用则日志说明） | ✅ |
| E6 🟡 | 151-160 | ProgramData\HuFu\diag 诊断日志升级即清 | 预置假 diag 文件 → 装后消失 | ✅ |
| E7 🟡 | 161-170 | %LOCALAPPDATA%\HuFu 早期版本残留清除（占用则下次再清） | 预置假目录 → 装后消失 | ✅ |
| E8 🔴 | 172-175 | HKLM Classes\CLSID\{CLSID}：default='HuFu TSF Service'；InprocServer32=**SystemIME 路径**；Apartment | reg query 值=SystemIME（非 {app}） | ✅ |
| E9 🔴 | 176-178 | HKLM CTF\TIP\{CLSID}\Description='HuFu 虎符输入法（虎码）' | reg query 值一致 | ✅ |
| E10 🔴 | 179-199 | 8 分类**双层四写**：TIP\Category\Category\{c}\{CLSID} + TIP\Category\Item\{CLSID}\{c} + 全局 Category\Category\{c}\{CLSID} + 全局 Category\Item\{CLSID}\{c}（缺项=Win+空格跳过本输入法，用户实测"只能前 3 个来回切"） | 装后逐键 query 8×4 全在（qa-backup hklm-cat/hklm-tip diff 可证） | ✅ |
| E11 🔴 | 200-205 | LanguageProfile\0x00000804\{PROFILE}：Description/Display Description/Enable=1(DWORD)/IconFile=SystemIME/IconIndex=0 | reg query 五值齐全，Enable 为 DWORD 型 | ✅ |
| E12 🔴 | 207-227 | smoke reg 登记（60s 超时语义，H6）+ 提权段退出码传播（smoke 失败→exit 非 0） | 代码审查 .iss 不吞 install.ps1 退出码 | ✅M1已修(第2轮,try/catch+$?判定) |
| E13 🔴 | 213-223 | 提权段把 HKCU COM 回写为 SystemIME 路径 + 校验失败告警（回写半边；与 H7 配对成完整闭环） | 装后 HKCU CLSID InprocServer32=SystemIME | ✅ |
| E14 🟡 | 28-33 | hufu_tsf32.dll 路径全脚本头定义（三十四修补：原只在提权段定义致 32 位视图注册空值异常） | 代码审查 .iss 不拆散 install.ps1 结构 | ✅ |
| E15 🟡 | 34-39 | SystemIME 固定路径 C:\Windows\SystemIME（$env:SystemRoot 推导）+ 注释实锤 SearchHost 不加载用户目录 DLL | 升级后新开开始菜单搜索可打中文 | ✅ |
| E16 🔴 | 84 | 提权子阶段**绝不启动 server**（标题铁律） | 装后 server 由普通权限路径启动（进程属主核对） | ✅ |
| E17 🟢 | 347-356 | 提权日志 %TEMP%\hufu-install-elevated.log 按 UTF-8 回读主窗口 | 装完主窗口提权段输出无乱码 | ✅ |

---

## U 节 · uninstall.ps1 卸载铁律（19 条）

> exe 卸载流（lead 架构）：helper -ClearOverride → 一次 UAC 跑**原版 uninstall.ps1**（零修改）
> + -ElevatedUninstallTail（PFRO 挂删 SystemIME 残留+HKLM 校验）→ Inno 删程序白名单 → 可选 -PurgeData。

| # | 行号 | 铁律内容 | exe 方案责任方 | 验证方法 | 审计结果 |
|---|---|---|---|---|---|
| U1 🔴 | 6-7, 70-73 | 顺序铁律：语言列表减法 → 杀 server → **杀 ctfmon** → 清注册（后杀 ctfmon 会在重启时用缓存档案重建 HKCU TIP，实测残留根因） | 原版 uninstall.ps1 零修改复用；.iss 卸载流不得在其之前杀 ctfmon 或清注册 | 卸载后 HKCU CTF\TIP\{CLSID} 不复活（重启/重登后再核） | ✅ |
| U2 🟡 | 15-29 | 提权模式：非管理员+在管理员组 → 固定 Windows PowerShell 路径 -Verb RunAs -Wait 重启自身（$PSHOME pwsh 坑，修复标签任务6） | 原版 uninstall.ps1 复用；.iss 用同语义调用（一次 UAC） | 卸载全程只见一次 UAC | ✅ |
| U3 🔴 | 33-53 | 语言列表**纯减法**：①真存储删自己值（幂等）②装配表按 CLSID 认领删槽（绝不误删他人槽位）——禁整表重写 | 原版 uninstall.ps1 复用 | 卸载后他人输入法（含虎爪）全部在场 | ✅ |
| U4 🟡 | 54-68 | 末态回读核验残留=0 + install.log 留证 | 原版 uninstall.ps1 复用 | install.log 有 "uninstall 减法移除: … 残留…——应全为 0" 行 | ✅ |
| U5 🔴 | 75-79 | msctf 档案注销（smoke unreg）仅完整卸载（管理员）做；每用户卸载不动（清了会导致无提权重装失忆） | 原版 uninstall.ps1 复用；.iss 卸载=完整卸载语义则必然提权 | 完整卸载后 HKLM CTF\TIP\{CLSID} 消失 | ✅ |
| U6 🟡 | 82 | 删 HKCU Run\HuFu 自启 | 原版 uninstall.ps1 复用 | Run 值消失（-Compare hkcu-run diff 可证） | ✅ |
| U7 🔴 | 84-91 | HKCU 清理全集：CTF\TIP\{CLSID}、Classes\CLSID\{CLSID}、Software\HuFu（InstallDir）、装配表槽位 | 原版 uninstall.ps1 复用 | -Compare：hkcu-hufu=GONE-KEY、hkcu-ctf 树 TIP 段移除 | ✅ |
| U8 🟢 | 92-95 | 开始菜单 lnk 清理（精确名+*HuFu* 通配双保险） | 原版 uninstall.ps1 复用 | 开始菜单无 HuFu 条目 | ✅ |
| U9 🟡 | 96-102 | %LOCALAPPDATA%\HuFu 早期残留清除（占用则提示重跑） | 原版 uninstall.ps1 复用 | 预置假目录 → 卸载后消失 | ✅ |
| U10 🔴 | 104-123 | HKLM 清理全集：CTF\TIP、Classes\CLSID、WOW6432Node CTF\TIP、WOW6432Node Classes\CLSID、全局分类库 8+1 项（Category\{c}\{CLSID} + Item\{CLSID}） | 原版 uninstall.ps1 复用 | -Compare：hklm-tip/hklm-cat/hklm-clsid/hklm-clsid32=GONE-KEY 或 diff 清零 | ✅ |
| U11 🔴 | 124-139 | SystemIME 腾位链清除：HuFu* 全部；当前目录被占用→改名 HuFu.oldN（腾位语义） | 原版 uninstall.ps1 复用 + **ElevatedUninstallTail 补 PFRO 挂删**（exe 新增，见 X11） | 卸载后 SystemIME 无 HuFu；占用场景见 T10 | ✅ |
| U12 🟡 | 140-146 | ProgramData\HuFu 删除+复查（实测偶发删后又被写回） | 原版 uninstall.ps1 复用 | -Compare programdata 清单差异=REMOVED | ✅ |
| U13 🟢 | 148-149 | 清 %TEMP%\hufu-*.log | 原版 uninstall.ps1 复用 | TEMP 无残留日志 | ✅ |
| U14 🔴 | 151-154 | HKCU 二次复查（ctfmon 已死后防外部重建） | 原版 uninstall.ps1 复用 | 卸载尾段后 TIP 键不存在 | ✅ |
| U15 🔴 | 156-159 | **数据保留铁律**：卸载=解除注册+停进程+C 盘产物清干净；安装目录与数据**一概不删**（用户拍板 2026-09-07） | Inno 卸载只删程序白名单（[Files] 记录的程序文件）；码表/码表导出/数据/模型 整体保留 | 卸载后 {app}\码表、码表导出、数据\config.json 全在 | ✅ |
| U16 🟡 | 161-175 | 腾位残留二扫（ctfmon 已杀后锁已释放，能删则删；剩余计数如实告知） | 原版 uninstall.ps1 复用 | 占用场景卸载输出剩余计数（T10） | ✅ |
| U17 🟡 | 177-178 | **最后**拉起 ctfmon（系统输入法框架恢复） | 原版 uninstall.ps1 复用；.iss 卸载收尾不得早于此 | 卸载后其它输入法正常工作 | ✅ |
| U18 🟢 | 180-206 | 完成输出：数据保留位置指引（config.json/用户调整/码表导出计数）+ 手动彻底清理指引 | exe 完成页继承同语义文案（需求④"告知位置"） | 卸载完成页含保留路径与文件计数 | ✅ |
| U19 🔴 | （缺口） | **【R16 实证缺口】原版 uninstall.ps1 全文不清默认输入法覆盖**——install.ps1:490 设了 InputMethodOverride，卸载不清=指向幽灵 TIP | exe 新增 helper **-ClearOverride**（提权前普通权限段先做）——修复责任在 helper | 卸载后 reg query InputMethodOverride 无 0804:{虎符} 值（-Compare uprofile diff 可证） | ✅S2已修(第2轮,.iss内联+8F5C2A11值过滤) |

---

## X 节 · exe 方案新增铁律（lead 架构 11 条，.iss 落盘后逐条核对）

| # | 铁律内容 | 责任方 | 验证方法 | 审计结果 |
|---|---|---|---|---|
| X1 🔴 | 门禁：仅 x64 + Win10 1607+（32 位/ARM64 拒装，防假成功——风险清单 R46/R47） | .iss ArchitecturesAllowed + [Code] 版本门禁 | 静态审查 +（可选）32 位 VM 实测拦截文案 | ✅S1已修(第2轮,x64os+paARM64双门禁) |
| X2 🟡 | 单实例互斥（双开=文件竞态+双 server，R63） | .iss InitializeSetup CreateMutex | 连点两次双击 → 第二实例置前退出（T14） | ✅ |
| X3 🔴 | PrivilegesRequired=lowest 全程普通权限（H1/H2 的 .iss 面） | .iss 指令 | 静态审查 + 双击启动无 UAC（UAC 只在 install.ps1 内部段出现一次） | ✅ |
| X4 🔴 | 检测多信号：InstallDir 主 + HKCU/HKLM TIP 键 + Run\HuFu + HKLM TIP IconFile + 目录有效性交叉；野残留=注册在目录亡 → 先清注册再装（R36/R41） | .iss 检测段 [Code] | T13 三个子场景（InstallDir 删/野残留/多目录）逐一通过 | ⚠ N2 |
| X5 🟡 | 升级锁定旧目录跳过目录页（防止双装双条目，R17/R75） | .iss 检测段→目录页逻辑 | 存量升级目录页直接跳过且显示旧路径 | ✅ |
| X6 🔴 | PrepareToInstall：杀 hufu-server + 5 个 PE（hufu-server/hufu_tsf.dll/hufu_tsf32.dll/hufu-tsf-smoke.exe/TigerClaw.Sentence.Native.dll）存在即改名 .oldN + 清陈旧 .old*；**不碰 ctfmon/系统宿主**（H18） | .iss PrepareToInstall [Code] | T10 占用场景：{app} 内 PE 全部腾位成功、SystemIME 由 install.ps1 提权段腾位 | ✅ |
| X7 🟡 | 程序文件走 [Files]（进 Inno 卸载日志=白名单依据）；数据文件不走 [Files] 不进卸载日志（U15 的机制面） | .iss [Files] 段 + 数据走 {tmp}\pkg | 卸载日志列程序文件；数据目录不在其中 | ✅(N7) |
| X8 🔴 | 数据保护拷贝：helper -BackupDiff 差异先备份到 {app}\升级备份\v旧版-日期\ 再覆盖；数据\config.json **仅缺失才拷**（保用户设置，R23/R30） | helper BackupDiff | T2：升级前改 config.json → 升级后原值保留 + 升级备份目录含被覆盖旧文件 | ✅(N7) |
| X9 🔴 | ssPostInstall 固定顺序：BackupDiff → 跑零修改 {app}\install.ps1 → 回读校验 InstallDir → 注册 HKCU RunOnce HuFuPostBoot（重启后清 .old*+自检——R13 的修复面） | .iss ssPostInstall [Code] | 代码审查顺序 + 重启后 {app}\*.old* 清零（T9） | ✅(终局 T9 实证)：postboot 真机跑过（07:38:27 日志"删4剩0 exit=0"62ms）+RunOnce 值被系统自动删=闭环 |
| T11/R10 | Fast-Startup（快速启动）下 PFRO 是否处理（最难路径：混合关机恢复会话） | 真机"关机→开机"观测 | LastBootUpTime 不变+HiberbootEnabled=1 铁证下：PFRO 未处理（HuFu.old1 保留） | ✅定论：机制性非 bug，完整重启自然清理，PostBoot 尽力删兜底充分（audit-report 8.6） |
| X10 🟡 | NeedRestart=升级或腾位发生时为真（驱动完成页"立即/稍后"二选一，需求⑤） | .iss NeedRestart [Code] | 全新装无占用→不要求重启；升级→要求（UI 二选一） | ✅ |
| X11 🔴 | 卸载流：/PURGEDATA 解析 → 非静默先弹码表备份确认（未备份→打开文件夹**中止**）→ 是否彻底删数据（默认否）→ helper -ClearOverride（U19/R16）→ 一次 UAC 跑原版 uninstall.ps1 + -ElevatedUninstallTail（PFRO 挂删 SystemIME 残留+HKLM 校验）→ Inno 删程序白名单 → PURGEDATA 则 helper -PurgeData → 完成页告知保留数据位置 | .iss 卸载流 + helper | T6/T7 逐项验收（备份确认中止逻辑/Override 清除/数据白名单删除） | ✅S3已修(第2轮)；S7记录为已知限制：他用户 HKCU 孤儿=引擎级每用户语义，v1 手工指引（他账户登录跑一遍安装即覆盖） |

---

## 审计流程（.iss 落盘后执行）

1. **静态审计**：通读 hufu-setup.iss（+helper 脚本），对 H/E/U/X 四节 71 条逐条填"审计结果"（✅ 符合 / 🔴 违反 / ⚠ 部分符合 / ➖ 不适用+理由）。
2. **动态验证**：按 qa-test-matrix.md 逐场景执行；每场景前后跑 `qa-backup-state.ps1` / `qa-restore-state.ps1 -BackupDir <dir> -Compare`，把 compare-report.txt 差异与场景期望比对。
3. **回报**：违反项+部分符合项汇总成差距报告发 lead（红项必须含：违反的行号引用、复现场景、建议修法）。

## 维护纪律

- 本清单行号基准是 2026-10 的 install.ps1/uninstall.ps1；若二者升级（版本号变更），须重新核对行号并修订本表。
- 新增血律（任何一次真机事故的教训）必须同步进 H/E/U 节——清单只增不减。

## 附录 A · Inno Setup 6 坑备忘（task-3/4 实战沉淀，写 .iss 前必读）

1. **[Code] 行首（含缩进）不能出现 `[` 或 `#`**——Inno 预处理器会把行首 `[` 当节头、`#` 当指令，报诡异编译错。
2. **LoadStringFromFile / LoadStringsFromFile 出参是 AnsiString 不是 String**（Inno 6.7）——直接传 String 变量类型不匹配；须 AnsiString 接收再 String() 转换。
3. **StringChangeEx 是 var 修改 + 返回 Integer（替换次数）**——不是返回新串；要"返回结果"须 S := Copy(…) 后 var 传入再读 S。
4. **external GetLastError 紧跟 CreateMutexW 调用**——Inno runtime 可能在两次调用间执行内部 API 污染错误码；官方 FAQ 模式可用但勿在两调用间插入代码（2026-10 红队复核：本机实测 CreateMutex+GetLastError 模式工作正常，183 自证有效）。
5. **装载器错误发生在 InitializeSetup 之前 → 无 /LOG 日志**——排障勿指望 Inno 日志；装载器弹框跨进程 WM_GETTEXT 会被 UIPI 挡，读文本用 UIAutomationClient（Add-Type -AssemblyName UIAutomationClient）。
6. **VERYSILENT 不抑制装载器层弹框**——/SUPPRESSMSGBOXES 只管向导期；装载器错误在静默部署下表现为"挂着"或被点掉后 exit 1。

## 附录 B · QA 环境干扰事件全记录（2026-10-13 真机 T2 阻塞，结案版·火绒翻案）

- 症状：HuFuSetup-1.8.2.exe 双跑均 exit=1、零 Inno 日志；UIA 抓到装载器弹框"Setup was unable to create the directory '…\Temp\is-*.tmp' – Error 5 拒绝访问"。
- **最终根因：DSH harness 文件沙箱按"进程映像启动路径"约束——从 workspace（E:\DSH-KF）内启动的进程在 %TEMP% 创建目录被拒（err=5）；从 workspace 外（C:\Temp）启动的同一 exe 成功**。csc 现编探针矩阵实锤：同一未签名探针，workspace 内跑=err5、C:\Temp 跑=成功；pwsh/cmd（C:\Windows 启动）全程放行。
- **误判链（教训）**：机器装着火绒（sysdiag 驱动在跑）→ 初判 HIPS 拦"未签名自解压"（R58 画像吻合）→ 信任区/退 UI/关防护/整卸+重启全无效 → 驱动清零后仍拦 → 控制变量锁定启动路径 → 翻案。火绒全程无辜（卸载动作客观上排除了一个变量，且今后 QA 不再受其干扰）。
- **方法论沉淀**：①"签名差异"是伪变量——签名进程都在系统目录、自编 exe 都在 workspace，两变量完全共线；控制变量必须移动**同一个文件**的位置。②QA 环境沙箱（harness 自身）先于第三方杀软成为干扰源候选。③Inno 装载器错误发生在 InitializeSetup 前→无 /LOG；读跨进程弹框用 UIAutomationClient。④装载器成功后自清 is-*.tmp——"无解包目录"≠"没跑成"，以 {app} 产物+退出码判定。⑤**杀 setup 装载器进程不杀其 setup 子进程**（探跑需杀进程树/或等自然结束）。
- **对真机测试的解法**：安装器 exe 复制到 workspace 外（C:\Temp）再跑；{app} 目标目录天然在 workspace 外不受影响。
- **产品级启示不变**（首发用户自家机器无 DSH）：R58"杀软放行预告+签名是根治项（R56/R61/R62）"结论保留；本事件主犯是 QA 环境，不改变产品风险评级。

## 附录 C · 三轮迭代修复记录（2026-10-13 下午，task-4 动态审计发现）

- **M3（已修）**：hufu-setup.iss `GetDateTimeString('yyyymmdd','','')` 第 2/3 参数是 **Char**，传空串运行时 Type Mismatch → CurStepChanged 整体炸掉（BackupDiff/引擎/RunOnce/server 核验全跳过）。修：'' → '/'。探针二分定位+修法验证（全链 STEP 走通）。**Inno 坑家族第五坑**（继 AnsiString/StringChangeEx/行首 [/无装载器日志之后）。
- **M4（已修）**：.iss 六处 helper 调用传 "-Command X" 风格，helper 实现是 **switch -X** 风格 → PS -File 宽松绑定下开关全落空（silent"未指定命令"exit=1）→ BackupDiff 不执行+卸载链 tail 三命令同样空跑。修：-Command X → -X。实证：修复后 BackupDiff 跑通（拷入=0 跳过=83 失败=0）。
- **M5（已修）**：helper MoveFileEx P/Invoke 第二参数 `$null` 被 PS 5.1 转为**空串**（非 NULL 指针）→ 目标路径不存在 err=3。修：`[NullString]::Value`。证据矩阵：非提权 $null=err3/NullString=err5（权限码）；提权+NullString=ok 且 PFRO 真写入。**PS 坑记录：P/Invoke string 参数传 NULL 必须用 [NullString]::Value**。
- **S3 之谜（待 lead review）**：卸载后 RunOnce\HuFuPostBoot 残留，键 LastWrite=卸载前时刻=usUninstall L600 RegDeleteValue 未执行，但同段后续 tail 确实执行（PFRO/SystemIME/HKLM 实证）——顺序执行下矛盾，无卸载 /LOG 无法再切；建议复现时带 /LOG 或 S3 前后加 LogLine。
- 观察记录：引擎 setup-engine.log 0B 属 PS 语义（install.ps1 全 Write-Host 不进 2>&1）；{app} 卸载后遗留 .oldN（占用）+licenses+日志文件（UninstallNeedRestart 提示重启清）；卸载后 HKLM Classes\CLSID 残留一次（提权手删成功，疑探跑引擎时序回写，非 uninstall.ps1 清单缺失——L107 在列）。
