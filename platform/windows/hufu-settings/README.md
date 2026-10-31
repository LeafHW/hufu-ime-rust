# hufu-settings（Windows 设置窗口壳）

虎符设置窗口的 Win32 + WebView2 薄壳。**P0 已跑通**：`x86_64-pc-windows-gnu`
下编译，不依赖 MSVC、不链接 WebView2 `.lib`、不引入 WinRT；加载新前端
（`hufu/settings-app`）后窗口标题「虎符设置」，关闭退出码 0。窗口默认 1280×800 DIP，按桌面缩放比自动换算。

当前默认加载 `http://127.0.0.1:4390/next?shell=1`（hufu-server 已部署 `/next`）；
Vite 只用于前端 HMR 开发。旧设置页仍由 `hufu-server` 在 `127.0.0.1:4390` 提供，`/` 未改动。

## 构建 / 运行

```powershell
cd E:\DSH-KF\hufu\platform\windows\hufu-settings
$env:TMP = 'E:\DSH-KF\.testtmp\tmp'
$env:TEMP = $env:TMP
cargo build --release --target x86_64-pc-windows-gnu

# 生产/日常：直接开新壳（默认 /next）
.\run.ps1

# 前端 HMR 开发：确保 Vite 在跑，再开壳（显式 --url）
.\run-dev.ps1
```

也可分开：

```powershell
# 终端 1
cd E:\DSH-KF\hufu\settings-app
npm run dev

# 终端 2
cd E:\DSH-KF\hufu\platform\windows\hufu-settings
.\target\x86_64-pc-windows-gnu\release\hufu-settings.exe --url "http://127.0.0.1:5173/?shell=1"
```

参数：`--url`、`--user-data`、`--log`、`--auto-close <秒>`、`--dpi <值>`（开发测试用）。
默认 user data 目录为 `%LOCALAPPDATA%\HuFu\WebView2`，日志在同目录
`hufu-settings.log`。

## DPI / 缩放

- 进程在创建窗口前设 **Per-Monitor V2** DPI awareness（`SetProcessDpiAwarenessContext(-4)`，失败回退 shcore `SetProcessDpiAwareness(2)` / `SetProcessDPIAware`）。
- 窗口按 **DIP** 定尺寸：默认 1280×800 DIP 乘当前 DPI 得到物理像素，再 clamp 到工作区（例如 1920×1080@150% → 1920×1040 物理，WebView 内容 1280×693 CSS）。
- 收到 `WM_DPICHANGED` 时按系统建议的 RECT 调窗口，`WM_SIZE` 再同步 WebView2 controller bounds；跨不同缩放比显示器移动时 WebView2 跟着重排。
- 开发/截图用 `--dpi <96|120|144|192>` 覆盖：窗口按该 DPI 定物理尺寸，同时给 WebView2 加 `--force-device-scale-factor`，用来在没有改系统缩放的情况下验证 125/150/200%。例如：

```powershell
E:/DSH-KF/hufu/platform/windows/hufu-settings/target/x86_64-pc-windows-gnu/release/hufu-settings.exe --url "http://127.0.0.1:5173/?shell=1" --dpi 144
```

- 已测：100%（1280×800 CSS）、125%、150%（1280×693 CSS）、200%（960×520 CSS）；100/125 完整三栏，150 保持侧栏、页面滚动，200 侧栏收 64px、皮肤参数变抽屉。矩阵图：`../../../settings-app/screenshots/dpi-matrix.png`。

## 产物

- `target\x86_64-pc-windows-gnu\release\hufu-settings.exe`（约 280 KB）
- 同目录自动复制 `WebView2Loader.dll`（x64，约 195 KB，来自
  `Microsoft.Web.WebView2` NuGet 1.0.4314.35）
- 分发时必须把这两个文件放在一起；不需要 `.lib` / WinRT / MSVC 运行库

## 技术要点

- `[dependencies]` 为空。`src/win32.rs` 手写 kernel32/user32/ole32/dwmapi 动态调用；
  `src/main.rs` 手写 COM vtable（槽位按 WebView2.h 1.0.4314.35 核对）。
- WebView2Loader 用 `LoadLibraryW` + `GetProcAddress("CreateCoreWebView2EnvironmentWithOptions")`。
- 主线程 `CoInitializeEx(COINIT_APARTMENTTHREADED)` + Win32 消息泵；WM_SIZE 同步
  controller bounds；WM_CLOSE 先 `controller->Close()` 再销毁窗口。
- 自定义标题栏：窗口用 `WS_POPUP|WS_THICKFRAME` 去掉原生 caption；React 显示自己的标题栏；拖拽（4px 阈值）→ WebMessage `win:drag` → `WM_NCLBUTTONDOWN`；最小化/最大化/关闭 → `win:*`；`add_WebMessageReceived` 槽位 34 已验证（DWM 深色/圆角保留）。
- 已知简化：handler/COM 引用泄漏到进程退出（生产需补齐 Release）；未做
  NewWindowRequested/外部导航拦截、下载/权限控制、Snap Layouts 悬停（自定义最大化按钮暂不弹系统 Snap 菜单）。
- 主要技术细节与 vtable 槽位参考 `.testtmp/webview2-shell-spike/README.md`
  （spike 原报告）与技能库
  `mem-skill-windows-gnu-webview2-msvc-winrt-6b3d4cc42803`。

## 服务端 /next（已部署 2026-10-31）

- `hufu-server/build.rs` 把 `settings-app/dist` 嵌进 server exe；`/next` + `/assets/*` 由 server 直接服务，不再依赖 Vite。
- 已替换 `D:\HuFu\hufu-server.exe`（旧版备份 `hufu-server.exe.bak-20261031-195621`），live 4390 生效；`/next` 与旧 `/` 都是 200。
- `Ctrl+Alt+H`、语言栏管道 `{"op":"settings"}`、托盘菜单都会拉起同目录的 `hufu-settings.exe`；服务器端只认 `HufuSettingsWnd` 窗口类，避免标题里带「虎符/设置」的浏览器/聊天窗口误命中（实测 DSH 会话标题会）。
- 设置窗口也可手工这样开：

```powershell
E:/DSH-KF/hufu/platform/windows/hufu-settings/target/x86_64-pc-windows-gnu/release/hufu-settings.exe --url "http://127.0.0.1:4390/next?shell=1"
```

日常 `run.ps1` 走 `/next`；`run-dev.ps1` 显式走 Vite 5173，仅前端热更新开发用。

## 下一步

1. `设置.bat`/开始菜单/安装器仍走旧 Edge，迁移到新壳；`Ctrl+Alt+H`/语言栏/托盘入口已接好。
2. 安装器/便携 zip 带上 `hufu-settings.exe` + `WebView2Loader.dll`；检测运行时并给安装提示。
3. 多显示器混合 DPI 的 `WM_DPICHANGED` 实机验证。
4. 剩余页与同步写入。
