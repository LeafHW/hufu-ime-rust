# hufu-tsf — Windows TSF 前端

纯 Rust（windows-rs 0.58）的 TSF COM DLL，架构同小狼毫：DLL 是薄壳，
按键经命名管道 `\\.\pipe\hufu-ime` 送到 `hufu-server` 引擎进程，
回包 `{consumed, commit, state}` 驱动 TSF 组段与候选窗。

## 构建

```powershell
cd platform/windows
cargo build --release          # hufu_tsf.dll + hufu-tsf-smoke.exe
./target/release/hufu-tsf-smoke.exe   # 冒烟（无需管理员）
```

## 冒烟覆盖（hufu-tsf-smoke）

| 步 | 内容 |
|---|---|
| 1–5 | LoadLibrary / DllRegisterServer(HKCU) / DllGetClassObject / CreateInstance(多接口 vtable) / msctf ThreadMgr |
| 6 | 语言档案注册探测：**msctf 的 Register/RegisterCategory/ActivateProfile 在非管理员下 E_FAIL**（写入型 API 需 HKLM；EnumProfiles 实测只列 HKLM 注册的 TIP） |
| 7–11 | `hufu_test_key` 直驱引擎链：u/j/k/l/m/space 全 consumed |

## 安装（真实使用）

运行 `install\install.ps1`（自动 UAC 提权）：
- HKLM COM 服务器（绝对路径）+ `CTF\TIP` 清单（Category\Category/Item + LanguageProfile\0x0804）
- HKCU 用户侧启用
- 然后注销重登，`Win+空格` 切换

## 结构

- `lib.rs` — 4 个 COM 导出 + `hufu_test_key` 测试导出
- `com.rs` — 类厂、HKCU 注册（DllRegisterServer，无提权场景）、`PROFILE_GUID`
- `tsf.rs` — HuFuTs = ITfTextInputProcessor(+Ex) **+ ITfKeyEventSink**（msctf 要求前景
  按键 sink 同时 QI 得到 ITfTextInputProcessor——独立 sink 会被拒 E_INVALIDARG）；
  EditSession 组段（InsertTextAtSelection → StartComposition → SetText → EndComposition）
- `ipc.rs` — 管道客户端（WaitNamedPipeW 重试）
- `candwin.rs` — GDI 分层候选窗 v1（32bpp DIB + UpdateLayeredWindow，皮肤 JSON 驱动；
  v2 计划 D2D + DirectComposition + DWM Acrylic/Mica）
