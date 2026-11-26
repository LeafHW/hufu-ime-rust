# Windows ARM64 TSF 构建与隔离验证

保持既有 `x86_64-pc-windows-gnu` / `i686-pc-windows-gnu`，新增
`aarch64-pc-windows-msvc`。ARM64 包的 `hufu_tsf.dll` 是 ARM64X pure forwarder，
两种宿主分别转发到同目录的原生 ARM64 / GNU x64 实现；x86 仍加载 `hufu_tsf32.dll`。
Rust TSF 本身不需要编译为 ARM64EC。

## 工具链

隔离根默认为 `C:\Workspace\hufu-build-tools`，不改变用户/系统 PATH。
Windows ARM64 或 x64 宿主都可使用；MSVC 需 ARM64/ARM64EC、x64/x86 工具与 Windows 11 SDK。
本轮实际版本为 Rust 1.100.0、VS 2026 18.10 / MSVC 14.51、SDK 10.0.26100。

以官方 rustup-init 在隔离环境安装对应宿主工具链，使用 `--no-modify-path`：

```powershell
$env:RUSTUP_HOME = 'C:\Workspace\hufu-build-tools\rustup'
$env:CARGO_HOME = 'C:\Workspace\hufu-build-tools\cargo'
# ARM64 宿主用 aarch64-pc-windows-msvc；x64 宿主用 x86_64-pc-windows-msvc。
rustup-init.exe -y --no-modify-path --profile minimal --default-toolchain stable
rustup target add aarch64-pc-windows-msvc x86_64-pc-windows-gnu i686-pc-windows-gnu
```

重现本轮应安装/指定 `1.100.0-<宿主>`，后续初始化传 `-Toolchain`。
本轮机器已核对 `stable-aarch64-pc-windows-msvc` 实际为 1.100.0。

GNU 标准库仅附 CRT 启动对象，不包含全部系统导入库及 libgcc。`gnu-libraries.lock.json`
锁定 Fedora 44 aarch64 的八个包，源码来源分别是 Fedora 官方 MinGW CRT/GCC/winpthreads。
在该构建环境安装锁定 NEVRA 后运行：

```bash
python3 prepare-gnu-libraries.py --tools-root /mnt/c/Workspace/hufu-build-tools \
  --archive /mnt/c/Workspace/hufu-build-tools/downloads/hufu-gnu-libraries-fedora44.zip
```

脚本先运行 RPM 完整性验证、检查每份输入的包归属，再复制 `.a/.o` 和许可证；
`mingw/provenance.json` 记录全部文件哈希及锁文件哈希，ZIP 使用固定条目时间。
Windows x64 构建机可以解压该同一工具包至 ToolsRoot，无须运行 Fedora 的编译器。
仅构建工具使用这些库，安装包不包含工具链。解压后在任意系统验证：

```text
python prepare-gnu-libraries.py --tools-root C:\Workspace\hufu-build-tools --verify
```

本轮工具包 SHA256：`811fcae4a8133245685b374042b40521e6be94906486c79c0da06415bbdae183`。
`gnu-link.cpp` 是调用 rust-lld 的 Windows 启动器，只将 Rust 自带 CRT 启动对象换为
与导入库同批的 GCC CRT，避免新旧 `DllEntryPoint` 或 LLVM/GCC 重定位实现混用。
它处理 Windows response file、保留其余链接参数，并在 GNU 输入缺失时明确失败。
不启用旧 `llvm-mingw-entry` 特性。

```powershell
. .\Initialize-Toolchain.ps1 -Architecture ARM64
# 可改为 x64/x86；只设置当前 PowerShell 进程，重复切换不会累计 VS PATH。
```

初始化会设置各 GNU target 的 linker/library flags，并为 ARM64 MSVC 加 `+crt-static`，
避免 TSF/注册工具额外依赖 VCRUNTIME140.dll。`-ToolsRoot` 和 `-Toolchain` 可覆盖本机路径。

## 构建入口

```powershell
.\Build-Tsf.ps1 -TargetDirectory C:\build\target -OutputDirectory C:\build\tsf
.\Build-Forwarder.ps1 -Arm64Dll C:\build\tsf\arm64\hufu_tsf.dll `
  -X64Dll C:\build\tsf\x64\hufu_tsf.dll -OutputDirectory C:\build\arm64x
```

`Build-Tsf.ps1` 输出 ARM64/x64/x86 的 TSF 与 smoke，允许用 `-Architectures` 单独构建。
ARM64 的图标从源 ICO 编译为 MSVC `.res`，不复用 AMD64 COFF。
64 位实现以 `HUFU_TSF_REGISTRATION_DLL=hufu_tsf.dll` 编译，自注册始终返回邻接 façade；
façade 缺失即失败，绝不把实现 DLL 写成 COM 入口。x86 保持自身路径。

桥脚本从两份实际 DLL 自动解析并比较全部导出，为每个符号生成两套 `.def`，
再由 MSVC 生成 ARM64X。图标也位于 façade。实现名为
`hufu_tsf_arm64_<generation>.dll` / `hufu_tsf_x64_<generation>.dll`，代次来自
`SHA256(arm64文件SHA256 + LF + x64文件SHA256)` 的前 20 个十六进制字符。
结果清单 `arm64x-manifest.json` 包含全部文件、架构、大小、哈希与导出集。
只应按清单收入三个 DLL；`forwarder-build/` 是构建证据，不进入安装包。

## 验证入口与边界

```powershell
.\Verify-Forwarder.ps1 -FacadeDirectory C:\build\arm64x `
  -Arm64Probe C:\build\tsf\arm64\hufu-tsf-smoke.exe `
  -X64Probe C:\build\tsf\x64\hufu-tsf-smoke.exe `
  -X86Probe C:\build\tsf\x86\hufu-tsf-smoke.exe `
  -X86Dll C:\build\tsf\x86\hufu_tsf32.dll -OutputDirectory C:\build\probe
python verify_forwarder.py C:\build\arm64x\hufu_tsf.dll
python test_verify_forwarder.py C:\build\arm64x\hufu_tsf.dll
```

运行探针调用 `probe-dll <路径>`，仅加载、解析全部导出、创建 IClassFactory / TIP，
不调用 DllRegisterServer、Activate、输入引擎或任何键入钩子；禁用加载画像写入。
它还严格比较返回的注册路径与被请求的 façade 的规范路径。
验证器比较前后 HKCU/HKLM 的 32/64 位本产品 COM/TIP 树，并包含架构错误、缺实现 DLL 的负例。
静态 Python 验证器解析 CHPE/ARM64X DVRT，实际应用 fixup 还原 x64 导出视图，
核对两套转发及实现架构/导出/内容代次；仅凭 `Machine=AA64` 不会通过。

smoke 的安装器接口另有 `reg <DLL>`、`unreg`、`enable`、`disable`、`inspect-profile`。
`reg` 只登记共享档案/类别，不激活提权账户；用户启用由未提权工具执行。
enable/disable 在新 COM 对象中读回状态；所有 API 失败均非零退出。
这些修改型接口没有在本轮隔离加载验证中执行。
构建、静态验证和隔离 COM 构造均不能替代真实注册、UWP/Office 输入或升级/重启验收。

参考：[微软 ARM64X pure forwarder](https://learn.microsoft.com/en-us/windows/arm/arm64x-build#building-an-arm64x-pure-forwarder-dll)。
