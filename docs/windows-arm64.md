# Windows ARM64 通用 EXE（开发验证）

基线：`dev-latency` 的 `395e0ec797713ec71bceed952905fc518d3adbd5`。
目标是向上游交付一个同时服务 x64 与 ARM64 Windows 的 EXE，不改变线上更新协议。
本分支的 ARM64 支持目前属于开发验证；本机安装验证与尚未完成的输入验收分别记录。

当前开发包为 [1.9.9.13图标修复版](validation/20261126-tsf-icon-fix/README.md)，
首轮源码接入与完整验证见 [2026-11-26记录](validation/20261126-universal/README.md)，
发布前工作见 [待办](open-items.md)。

## 组件与安装契约

| 组件 | x64 系统 | Windows 11 ARM64 |
| --- | --- | --- |
| 后台、评分库、维护程序、渲染助手 | x64 | ARM64 原生 |
| 64 位 TSF | x64 Rust DLL | ARM64X 入口转发到 ARM64 / x64 Rust DLL |
| 32 位 TSF / 注册探针 | x86 | x86 |
| 设置、WebView2Loader、跟打器 | x64 | 同一份 x64 程序，系统仿真运行 |

CLSID、语言档案 GUID、Inno AppId 与数据格式保持原值。ARM64 与 x64 宿主共享
一个 64 位 COM 注册入口，x86 使用独立注册视图；不会把两份普通 64 位 DLL
依次注册到同一个槽。ARM64X 使用 [微软 pure forwarder 方案](https://learn.microsoft.com/en-us/windows/arm/arm64x-build#building-an-arm64x-pure-forwarder-dll)。

打包源的 `x64/`、`arm64/` 各有 `package-architecture.json`，schema 1 包含
`system_architecture`、`version`、`files[{path,machine,sha256,role}]`。
PE 架构、角色、必需文件、路径和 SHA256 同时校验。ARM64X 还须验证 CHPE、
动态重定位及两个导出视图，安装前用原生/x64/x86 三个独立进程实际加载。

系统 TSF 和其依赖按 `version-manifestHash` 部署为不可变代次，验证后才切换
注册。转发实现名含内容代次，避免旧宿主加载新旧混合实现。当前用户与提权机器阶段
分离，后台不由提权进程启动。升级保留码表、用户设置、皮肤和跟打记录；仍遵循
现有数据清单的首装/升级规则，不自动迁移 `/SELECTDIR` 指定的新目录。

Inno 在停止旧后台之前先解压并预检选中架构。首次准备冻结目标目录，备份受影响程序、
`unins*.exe/dat/msg` 及本产品当前用户卸载键。核心安装失败时明确显示失败、返回非零并
尝试补偿；恢复失败保留备份和日志，不宣称已全部恢复。核心引擎成功后即视为提交，
后续启动提示等便利操作异常只记录警告，不能仅撤回程序而遗留新注册。

详细的机器/用户事务、超时处理、卸载和恢复边界见
[维护引擎契约](../platform/windows/hufu-setup-engine/UNIVERSAL.md)。

## 构建

需要 Rust Windows 工具链、VS 的 ARM64/ARM64EC/x64/x86 工具、Windows SDK、
ClangCL、CMake、Python、Node/npm 及 Inno Setup 6.7+。工具链只在构建进程中
初始化，不要求改变系统 PATH。GNU x64/x86 的 CRT 与导入库必须来自同一工具集；
不再使用原作者账户和 E 盘的硬编码路径。工具准备与桥接命令见
[ARM64X 工具目录](../platform/windows/arm64x/)。ARM64 Rust 组件使用静态 CRT。

`DataSource` 必须是明确提供的发行资源源，至少含 `码表/`、`数据/`，可含
`licenses/`。它不从本机日用目录取数据，不包含模型，也不允许用户调整日志入包。
本轮验证资源来自已校验的原版 `HuFuSetup-1.9.9.9.exe` 的只读解包，不能把
源码 `assets/` 的较小集合假充为完整现发资源。

```powershell
# 默认会先构建一次设置页，再构建所有目标并检查当前宿主载荷；不安装或发布。
platform\windows\setup\build-universal.ps1 `
  -Version 1.9.9.13 -DataSource C:\build\hufu-package-source `
  -BuildRoot C:\build\hufu-universal -DistDir C:\build\hufu-output `
  -ToolsRoot C:\build\hufu-tools -Iscc C:\tools\Inno\ISCC.exe
```

可以用 `python tools/windows_package.py ui-build settings-app` 在另一构建环境
先构建前端，再传 `-UseExistingUi`；源码与 dist 指纹必须仍然匹配。
`-BuildOnly` 只生成已校验载荷。已有载荷可交给 `build-universal-setup.ps1`
重封装，`-StageOnly` 只生成清单和 Inno 输入，不编译安装器。

`发行脚本/release.ps1 -Universal` 接入同一构建链；旧 x64 ZIP/EXE 入口保留。
通用模式默认只构建，发布必须显式指定。资源按内容 SHA256 统一存储，既保留两套
目标文件清单，又避免重复封装字典、皮肤和共用工具。

评分 DLL 的可复现来源、同模型差分与当前限制见
[原生评分验证](windows-arm64-native.md)。不要直接运行虎爪旧发布脚本替代此构建：
其当前目标已改成静态链接 EXE，且可能清理旧 DLL 或更新日用目录。

## 在线更新

正式发布仍只能有一个 `HuFuSetup-<version>.exe`。Gitee/GitHub 镜像必须提供
相同文件；现有 `update.json`、SHA256、size、sources 和下载后手动启动方式不变。
不增加架构分流，不把两个不同 EXE 写成同一更新清单的两个镜像。

旧更新器上限是 **67,108,864 字节**，包含安装器全部最终字节。
新旧 EXE 构建脚本及 Gitee 发布脚本都在发布前检查该上限。超过即失败；
仅提高新客户端上限不能解决旧客户端首次升级。最终签名若改变文件大小和哈希，
必须重新执行大小检查并生成清单。

## 验证与交付边界

- 静态/单元：`python -m unittest discover -s tools -p test_windows_package.py`；
  维护引擎测试使用隔离注册表子树，不操作正式 CLSID 或卸载键。
- ARM64X：真实 ARM64、仿真 x64 加载同一入口；x86 加载其独立 DLL，核对
  全部导出、类工厂、实例化及最终注册路径。PE 头检查不能替代实际加载。
- 打包：独立解包后按两份清单逐文件核对；原资源哈希保持；检查各程序依赖、
  设置页嵌入和最终 EXE 大小。通用构建还逐帧核对 TSF 虎头资源，并比较 Windows
  六档尺寸绘制像素；仅返回有效 HICON 不足以判断图标正确。构建机上的 x64 仿真
  不等于原生 x64 Windows 验收。
- 原生评分：重建 x64 与现发 DLL 的固定 125 条候选评分完全一致；ARM64
  默认路径 Top1 为 25/25 相同、完整排名 24/25 相同。最大绝对评分差约 0.36157，
  有一组非首位候选互换。消融尚未完全定位误差，严格数值诊断未通过。
- 已在本机执行真实 ARM64 EXE 安装、UAC 允许、用户启用和后台启动；三种架构通过
  已登记 COM 创建 TSF。具体安装/卸载/覆盖验证以当前交付记录为准。
- 尚需专用环境验收：UAC 拒绝与恢复、跨账户、重启、原生 x64 最低系统版本、
  已安装 TSF 的真实桌面/AppContainer 输入。未完成这些验证的包只作开发验证交付。

源码面向 `dev-latency` 提交 PR，开发包版本不代表官方发布；Release 与官方更新源
仍由上游发行流程维护。验证目录中的“未提交/推送”描述各轮测试当时的状态。
公开的 PowerShell transcript 隐去了账户与计算机名称字段；测试输出、退出码和文件哈希保留。

## 安装包体积

实测原版 `HuFuSetup-1.9.9.9.exe` 为 27,362,643 字节（26.10 MiB），
当前开发包 `HuFuSetup-1.9.9.13.exe` 为 39,427,051 字节（37.60 MiB），
增加 12,064,408 字节（11.51 MiB，44.1%）。两者包含不同版本的维护引擎，
该比较是实际交付包比较，不是同版本禁用/启用 ARM64 的控制实验。

相对于当前 x64 载荷，ARM64 独有文件压缩前共 22.52 MiB：后台 14.61 MiB、
整句评分 DLL 3.59 MiB、TSF 与 ARM64X 入口 2.94 MiB、其他维护/渲染/检查文件
1.37 MiB。共用程序与 102 项发行资源按内容去重；资源与原版逐文件哈希相同。
Inno 使用整体压缩，不能将单个文件的原始大小直接当作 EXE 的下载增量。
