# ARM64 TSF / ARM64X 第一门槛验证（2026-11-26）

源基线 `395e0ec7`；本轮只构建和隔离加载，没有安装、注册、激活输入法、启动虎符服务或执行真实键入。
此目录记录开发 TSF 产物，不能代替主打包流程的最终 strip 产物验收。

实际完成 ARM64 MSVC、x64 GNU、x86 GNU TSF 与各自 smoke 的 release 构建。
ARM64 使用静态 CRT；PE 导入检查没有 VCRUNTIME/MSVCP、libgcc 或 libwinpthread 的运行 DLL 依赖。
GNU 构建使用锁定 Fedora GCC/MinGW 库和与其同源的 CRT 启动对象，保留 GNU target，未启用旧 LLVM CRT 入口补丁。

最终开发 façade 代次为 `9949fdde6439d2333d80`，SHA256 为
`269abcbd24025eccb6a6643223d9b008931a3827262fe83af7045e017ffa3085`。
ARM64 / x64 两个真实 Windows 进程加载同一个 `hufu_tsf.dll`，分别落到该代次 ARM64 / x64 实现。
两者均解析全部 18 个导出，完成 IClassFactory 和 ITfTextInputProcessorEx 构造；返回的注册路径严格等于 façade。
x86 smoke 独立加载 `hufu_tsf32.dll`，同样通过。

运行验证共 7 项通过：上述三项正例、ARM64→x86 / x86→façade 两项错误架构拒绝、
ARM64/x64 缺失实现 DLL 两项拒绝。验证前后 HKCU/HKLM 的 32/64 位本产品 COM/TIP 树完全一致。
`verification.json` 绑定全部探针和 DLL 哈希，stdout/stderr 与注册表快照随附。

静态结构验证共 7 项通过：真实 ARM64X；拒绝普通 ARM64 DLL、缺失 CHPE、错误 DVRT symbol、
篡改 x64 转发、截断 PE、缺失 sidecar。验证器实际解析/应用 9 条 ARM64X fixup，
还原 x64 导出目录，检查两套转发、sidecar 架构/导出与内容派生代次。
这允许 x64 构建机提前拦截“仅 Machine=AA64 的普通 ARM64 DLL”。

验证入口和可复现工具链准备见 [arm64x/README.md](../../../platform/windows/arm64x/README.md)。
工具链：Rust 1.100.0 ARM64 宿主、VS 2026 18.10 / MSVC 14.51、Windows SDK 10.0.26100；
源码摘要见 `source-sha256.json`，外部 GNU 库锁定身份与生成脚本在同一工具目录。

没有执行 smoke 的 reg/unreg/enable/disable 修改型入口；没有声称 UWP、开始菜单、Office、
真实注册、多用户安装、升级回滚或重启验收。主安装包完成后必须对其实际最终字节再次运行探针。
