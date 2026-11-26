# 1.9.9.12：TSF 注册后的读回与本机完整安装验证

唯一交付：`C:\Workspace\hufu-ime-rust\交付\HuFuSetup-1.9.9.12.exe`。
39,508,495 字节（37.68 MiB），SHA256：
`4787a4294babbc643b22c68d1e2b9b58329c86bfd1a193da00faab50338d92b1`。

仍为 ARM64 通用安装包开发验证版，未提交、推送或修改官方更新源。
用户在本轮明确授权任意修改和清除本机虎符、无需备份，以便测试；没有修改其他输入法。
最终本机保持 1.9.9.12 安装完成，原生 ARM64 后台运行。

## 故障与复现

1.9.9.11 已通过不可变目录部署及 ARM64/x64/x86 加载检查。本轮错误发生在
`RegisterProfile` 成功后的独立 `inspect-profile`：立即读回退出 2，旧引擎立即撤回注册。
原始日志和已恢复的机器事务见 `initial-setup-engine.log`、`initial-rollback.json`。

授权后使用真实 HuFu 登记、真实 msctf API 连续做了三轮注销/注册，每次只调用一次
变更接口，再用新进程只读观察。三个注册周期均先返回 `GetProfile=0x80004005`，
约 582 / 537 / 554 ms 后成功；注销也先读到旧档案，随后消失。
完整结果见 `live-convergence.log`，复现脚本为 `test-live-convergence.ps1`。
这些时间是本机观测，不是 Windows 对其他机器或所有宿主的时限保证。

对照白虎 `maintenance/RegistrationState.h::awaitRegistrationState`，其已有同样的
独立进程、有界只读重试策略。微软 API 文档将 `S_OK` 定义为单个注册调用成功，
未规定全系统枚举同步时限：
[RegisterProfile](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfinputprocessorprofilemgr-registerprofile)、
[GetProfile](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfinputprocessorprofilemgr-getprofile)。
本轮真实观察和最终 EXE 日志共同支持此修复，不以论坛经验代替测试。

早期探索也发现，`RegOverridePredefKey` 不能隔离 TSF 的全部用户键访问和进程外状态。
一次启用探测在真实 HKCU 产生了仅含 `Enable=1` 的 HuFu TIP 项；当场核对并恢复为
测试前不存在状态，之后停止用该方式验证启用，且在用户授权后转为真实安装验证。
临时 `HuFuDevelopmentTests` 子树及该探索程序已清理。该探索不计为安装验收证据。

## 实现

- `deployment.rs`：注册只执行一次。最多启动 21 次独立只读检查，间隔 500 ms；
  仅档案检查退出 2 可重试，超时、启动失败或异常退出立即失败；单个观察进程限时 5 秒。
  耗尽仍失败并走既有回滚。恢复旧代次时使用相同的观察策略。
- `procop.rs`：捕获维护子进程 stdout/stderr，包括失败 HRESULT。捕获文件独占、
  create-new、关闭自动删除，日志最多取 64 KiB；保留超时终止并确认退出的约束。
- `hufu-tsf-smoke`：读回错误包含操作名和数值 HRESULT。继续要求实际读回成功，
  不绕过检查、不重复注册、不重启 ctfmon、不重写其他输入法列表。

最终包相较 1.9.9.11，仅维护引擎、各架构注册探针、版本文件和清单改变。
TSF DLL、后台、评分库、设置程序与资源保持。见 `payload-comparison.json`。
最终源码哈希在 `fix-source-hashes.json`，架构清单在 `package-arm64.json` / `package-x64.json`。

## 实际验证

环境：Windows 11 ARM64 10.0.28000.3501，同一个用户允许 UAC；x64/x86 在系统仿真中运行。
实际使用最终 SHA256 对应的 EXE，安装到 `%LOCALAPPDATA%\Programs\HuFu`。
安装使用 `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART`，真实机器登记和当前用户阶段均执行。

| 检查 | 结果及证据 |
| --- | --- |
| 维护单元测试 | ARM64、GNU x64 各 20 通过；3 个忽略项为两种子进程夹具和显式暂存集成入口。子进程夹具实际由父测试调用。 |
| 真实暂存集成 | 最终 ARM64 载荷经生产 stage、三架构加载、再次复用及源文件保持检查通过。 |
| 包结构测试 | 9 项通过。 |
| 独立解包 | x64 25 文件、ARM64 28 文件、资源 102 文件逐项哈希通过，`extraction.json`。 |
| 首次登记安装 | 清除本产品 COM/TIP 后安装成功、退出 0。`installed-engine-first.log` 记录第一次观察 E_FAIL、第二次成功，以及用户启用成功。 |
| 卸载与重装 | 首次卸载成功；随后完整循环的重装、同版覆盖、卸载、最终安装均退出 0，见 `installer-cycle.json`。 |
| 卸载后状态 | 机器/用户两视图自有 COM/TIP、卸载入口、程序及后台进程均不存在；不可变代次与用户数据按契约保留。 |
| 已安装 COM | 每次安装/覆盖后均以 ARM64、x64、x86 新进程执行 GetProfile、IsEnabled 和 CoCreateInstance(ITfTextInputProcessorEx)，全部成功。 |
| 实际加载路径 | ARM64/x64 均加载同一 ARM64X 入口、分别进入对应实现 DLL；x86 加载 `hufu_tsf32.dll`，见 `installer-cycle.log`。 |
| 最终安装文件 | 清单内 27 个文件与最终包哈希一致；另有清单自身，见 `installed-files.json`。 |
| 在线更新约束 | 39,508,495 字节小于旧客户端 64 MiB 上限，最终收据与发行 VerifyOnly 通过；本轮未再次调用旧客户端联网下载。 |

最终系统代次：`%WINDIR%\SystemIME\HuFu\generations\1.9.9.12-2e813cb65696`。
三架构探针只验证已登记 COM 构造与加载，**没有 Activate 或真实应用键入**。
本轮不宣称已完成候选上屏、AppContainer 输入、跨账户、UAC 拒绝、断电恢复、重启、
旧 1.9.9.9 x64 安装升级或原生 x64 系统验收。ARM64 评分差异仍见
[发布前待办](../../open-items.md)。

## 交付整理

用户已关闭失败的 1.9.9.11 安装器。校验新包复制后的哈希后，删除交付目录的旧 EXE/
收据/校验文件，以及本次 `_tmp` 下的重复安装 EXE；交付目录只保留一个安装 EXE。
本机旧 SystemIME 不可变代次仍按维护契约保留，不将其当作普通临时安装包随意回收。
原版 1.9.9.9 下载基线仅保留在 `_tmp/arm64/baseline/` 作为历史比较依据。
`delivery-replacement.json` 记录最终路径和清理范围。
