# 通用包 ARM64X 隔离核验

本记录绑定 `_tmp/arm64/build/payload-95b54d2fba2246e98c97fe7f937d54e8`，
版本 `1.9.9.10`，ARM64X 代次 `42747a9bd16467ba422c`。
此后重新构建的程序不能仅凭同版本号复用结论，应比对实际文件 SHA256 或重跑。

最终交付绑定已补充：`payload-e080f8d8385448229d9fe2f9a4e33973` 与上述实测载荷的
全部 DLL、smoke、server、设置与跟打器字节一致；仅两架构 setup-engine 及其派生
manifest 更改。已独立逐文件核对，见 `final-payload-binding.json`；新 engine 导入也不
依赖额外动态 CRT。新 engine 的运行门禁与最终 EXE 解包由上层验证记录覆盖，不复用
旧 engine 退出负例冒充新程序测试。

交付文件现位于 `交付/HuFuSetup-1.9.9.10.exe`（整理前位于
`_tmp/arm64/delivery/20261126-arm64-dev/`，历史JSON路径保持），已独立核对
SHA256 `d46200ef856b2246ec5e3dc1fac7237ae1842ed4b8af678bb4d93046c7657cd1`。

本机是 Windows ARM64；没有安装、调用注册/启用接口、启动虎符后台或执行真实应用键入。
以下结果来自本轮实际运行，不能代替真实安装、UAC、多用户、重启及已安装 TSF 验收。

## 实际加载：7 项通过

入口为 `platform/windows/arm64x/Verify-Forwarder.ps1`，载荷的三款 smoke 工具加载
`bridge/hufu_tsf.dll`；桥及两个实现 DLL 已与 `arm64/` 内实际载荷逐字节 SHA256 对照。

- ARM64 与 x64 进程加载同一个 façade，分别进入本代次的 ARM64 / x64 实现。
- 三种宿主都解析 18 项导出、调用 `DllGetClassObject`、取得类工厂并构造 TSF COM 对象。
- x86 工具成功加载自己的 `hufu_tsf32.dll`。
- ARM64 拒绝 x86 DLL；x86 拒绝 ARM64X façade。
- 仅复制 façade、缺少实现 DLL 时，ARM64 和 x64 都拒绝加载。
- 诊断返回的注册路径与请求的 façade 全路径一致；x86 返回自己的 DLL。
- 前后 HKCU/HKLM、32/64 两视图的虎符 CLSID/TIP 快照一致。

详见 `verification.json`、三份 `*-facade/native.stdout.txt`、其余负例日志及
`registry-before.txt` / `registry-after.txt`。
测试负例的唯一 façade 复制件已移至
`_tmp/arm64/validation-fixtures/payload-95b54d2fba2246e98c97fe7f937d54e8/missing-implementation/`，
不是可安装目录，也不作为源码证据提交；原测试路径和 SHA256 保留在 `negative-fixture.json`。

## 静态结构：7 项通过

`static-negative-tests.json`：真实 ARM64X 通过；普通 ARM64、缺 CHPE metadata、错误 DVRT
symbol、错误 x64 转发目标、截断 PE、缺 sidecar 六种变体均拒绝。
验证器实际重放 9 个 ARM64X 动态重定位，核对两套 18 项导出及实现 DLL 的代次哈希；
没有仅凭 `Machine=AA64` 判定 ARM64X。

## 最终载荷内容检查

运行 `inspect_payload.py` 可复核 `payload-inspection.json`：

- 两架构共 25 个 PE 的普通与延迟导入均为 Windows 系统 DLL，没有遗漏的
  VCRUNTIME/MSVCP/libgcc/libwinpthread 动态运行库。
- ARM64 façade 无普通导入；两套 generation sidecar 与最终目录相符。
- 两后台都包含当前 dist 的全部 4 个文件完整字节，逐文件哈希已记录。
- 12 个产品 EXE / TSF 含图标。6 个 server/settings/trainer EXE 的 5 帧图标与
  `setup/icons/虎符.ico` 一致；新 ARM64 TSF/桥的 6 帧与 `hufu-tsf/assets/hufu.ico` 一致。
- 既有 GNU TSF 保持预制 COFF 的 3 帧资源：该历史资源是两层目录、图标组声明
  长度 44 字节，与 SDK rc 的标准三层结构不同；本轮没有改写它。
  Windows 资源只读加载、`FindResourceW` 和 `PrivateExtractIconsW` 实测全部正常：
  12 个文件分别请求 16/32/48/256 像素，共 48 次均返回有效 HICON。
  详见 `probe_icons.py` 和 `windows-icon-probe.json`；不把这项视为外观像素验收。

设置宿主、跟打器及 WebView2Loader 在两载荷中均为 x64，属于本阶段明确保留的共享组件。
Evergreen WebView2 运行时仍为外部前提，并非上述 PE 导入扫描能够证明已随包提供的资源。

## GUI 进程退出门禁负例

`verify-preflight-exit.ps1` 通过 PowerShell 管线等待本载荷的最终 setup-engine，
向它传入此前失败的 `payload-1143f8c8c6af4147916c975a9f920a40/arm64`。
日志明确为 `registration-native probe failed: Exit(2)`，管线读取到 `LASTEXITCODE=1`，
断言脚本返回 0。证据为 `preflight-exit-negative.json`、`bad-payload-preflight.log`
及 `bad-payload-pipeline.*.txt`。没有运行安装或补偿事务。

## 独立构建审查结论

审查范围为 `tools/windows_package.py`、`setup/build-universal.ps1` 及直接调用的
资源、前端和 ARM64X 构建入口。已修正的 façade 编译环境变量与 GUI 进程等待门禁，
在本载荷中通过实际加载及失败退出负例验证。没有发现本载荷遗漏运行库、ARM64X 实现
DLL、图标或已构建的前端资源。本记录绑定的二进制清单、导出、资源、哈希与运行证据
相互对应；单凭当前打包脚本的静态校验范围，不应推称以后每次构建均已运行上述开发验证。
