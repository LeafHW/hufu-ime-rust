# Windows ARM64 通用 EXE 开发交付（2026-11-26）

**本页是1.9.9.10历史验证记录。** 该包实际安装发现仿真探针退出后目录重命名失败，
已由 [1.9.9.11修复版](../20261126-stage-rename-fix/README.md) 替代；当前交付目录仅保留新版。

基线 `dev-latency` / `395e0ec797713ec71bceed952905fc518d3adbd5`，
工作分支 `feat/windows-arm64-universal`。本轮完成源码接入、构建和隔离验证，
没有提交、推送、发布、执行安装器或修改日用输入法注册。

## 唯一交付

原交付文件名 `HuFuSetup-1.9.9.10.exe`（已撤下，当前文件见上方修复记录）。

- 大小：**39,502,283 字节，约 37.67 MiB**。
- SHA256：`d46200ef856b2246ec5e3dc1fac7237ae1842ed4b8af678bb4d93046c7657cd1`。
- 一个 EXE 包含 x64 / ARM64，保留原在线更新协议及 64 MiB 上限。
- 这是开发验证包；版本号用于本地构建，没有写入官方更新源。

最终载荷为 `_tmp/arm64/build/payload-e080f8d8385448229d9fe2f9a4e33973`。
按用户要求，6个中间测试安装包及其收据/校验文件已清理，最终EXE与校验文件移至仓库根
`交付/`，文件哈希保持。构建日志、验证证据和缓存保留；历史报告中的路径是测试时的位置。
清理清单见 [package-cleanup.json](package-cleanup.json)。尤其前两次的成功构建输出
不能当作预检通过证据。

架构、构建与事务接口见 [实施说明](../../windows-arm64.md)，
正式发布前缺口见 [待办](../../open-items.md)。

## 最终文件与实测结果

| 项目 | 结果与证据 |
| --- | --- |
| 完整生产构建 | x64 GNU、ARM64 MSVC、x86 GNU；设置/跟打器共用 x64；两个评分 DLL 从锁定源码构建 |
| 最终 ARM64 安装前预检 | `OK payload arm64 1.9.9.10-35747341ed57`；原生/x64/x86 三个进程实际加载 |
| 包清单回归 | Python 9 项通过，包含架构/路径/哈希/普通ARM伪装/去重/数据保护及64MiB边界 |
| 构建退出门禁 | 最终引擎的有效预检、失败预检、进程启动失败 3 项通过；[结果](build-gate-delivery/report.json) |
| ARM64X | 7 项真实加载、7 项结构正负例；[完整记录](arm64x/README.md) |
| 导入和资源 | 25 个 PE 导入核对、48 次 Windows 图标提取通过；两个后台各4个前端文件完整嵌入 |
| 独立解包 | x64 25 文件、ARM64 28 文件（均含清单），102 项发行资源大小/哈希全部保持；[结果](extraction.json) |
| 实际皮肤渲染 | 4 款正式 SSF、中文目录、原生 ARM64 helper 经 ARM64X 出图；[结果及BMP](render/README.md) |
| 维护引擎开发测试 | 最终 ARM64 16 项通过；另有先前 x64 仿真15项、Linux清单8项；[范围](../../../platform/windows/hufu-setup-engine/UNIVERSAL.md) |
| 评分 | 重建x64与现发DLL的125个候选double全同；ARM64 Top1 25/25、全排名24/25；[原始差分及限制](../../windows-arm64-native.md) |
| 发行入口只读核验 | `release.ps1 -Universal -VerifyOnly` 对最终EXE大小、SHA及构建收据通过，未传 `-Publish` |
| 原版在线更新器 | 原1.9.9.9发现1.9.9.10、SHA错误拒绝、镜像回退、64MiB边界及最终EXE实际下载共6项通过；[结果](update/README.md) |

原始完整构建日志 `_tmp/arm64/build-universal-delivery.log`；
静态资源库存仍位于原构建目录
`_tmp/arm64/delivery/20261126-arm64-dev/setup-6954c91b6c5f491eb70945ba68f2a7d8/staging/inventory.json`。
两份最终 `package-architecture.json` 与构建收据随本验证目录保存。

独立解包工具为 Linux innoextract 的 Inno 6.7 支持分支
`LordAro/innoextract@e8960a05eef5ea218bf05a1e23adc3f688c3d5b5`，
仅解包未执行 EXE。`tools/verify_universal_extraction.py` 随后逐文件对照源载荷和资源。
发行资源取自原版 `HuFuSetup-1.9.9.9.exe`，其SHA256为
`266b448139fcf5b05c305537aabf58f3c24fa785196a875f1d1cfacecfd1f0f8`，没有读取日用安装数据。

## 本轮发现并修复的集成问题

主构建入口起初漏传 `HUFU_TSF_REGISTRATION_DLL`，导致转发后的实现自报注册路径错误。
同时 Windows PowerShell 对 GUI 子系统 EXE 的直接调用没有等待，留下旧成功退出码。
现在64位构建显式设置 façade 名、x86清除该变量；生产调用函数通过管线等待并重置/读取
全局退出码，进程无法启动也失败。`test_windows_build_gate.ps1` 直接加载生产函数进行验证，
避免用另一份实现代替门禁。早期 `build-gate/`、`build-gate-final/` 为修订过程记录，
最终新引擎以 `build-gate-delivery/` 为准。

Inno 的 ssPostInstall 异常本身不会自动回滚。新流程备份程序、卸载器、7个旧维护入口和
本产品卸载键，冻结目标目录；核心失败时明确失败页、非零退出并补偿。机器和原用户阶段
分别恢复自身注册，提权恢复被拒则如实报告不完整。核心提交后辅助启动异常不会只撤回文件。
所有提权维护阶段转发同一日志位置，不可写时在变更前失败。此设计经过代码审查和引擎隔离测试，
**尚未在真实安装器/UAC/重启场景完成端到端故障验收**。

## 验证边界

ARM64X 构造、渲染、评分及下载检查不等于已安装输入法真实键入验收。
本机x64属于Windows ARM仿真，没有原生x64最低系统实测；设置和跟打器做了构建、导入及资源检查，
没有本轮完整交互验收。多用户、UAC拒绝/恢复、升级/卸载/重启仍需隔离Windows环境。

ARM64评分最大绝对差约0.36157，一组非首位候选交换顺序，严格数值诊断未通过且根因未完全定位。
保留默认ARM计算路径与全部失败证据，没有把禁用NEON的较差结果作为发布版本。
