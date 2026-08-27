# hufu mac — macOS InputMethodKit 前端

薄壳架构（与 Windows TSF 前端一致）：输入法进程只做事件转发与绘制，
引擎在 `hufu-server` 守护进程里。

```
按键 → IMK handle() → Unix socket ($XDG_RUNTIME_DIR/hufu-ime.sock)
      → hufu-server 引擎 → {consumed, commit, state}
      → setMarkedText 组段（inline 编码）+ insertText 上屏
      → CandidatePanel（NSVisualEffectView 材质随皮肤 JSON）
```

## 文件

- `HuFuIME/HuFuInputController.swift` — IMKInputController：键码映射（kVK_ANSI_* →
  引擎键名）、EngineClient（4B 长度 + JSON 帧，与 Windows 管道同协议）、组段/上屏/
  候选窗跟随插入点
- `HuFuIME/CandidatePanel.swift` — NSPanel（nonactivating、floating、canJoinAllSpaces）
  + NSVisualEffectView：皮肤 `material` → `solid/translucent/frosted/glass` 映射
  （frosted=behindPageBackground 磨砂、glass=fullScreenUI 玻璃），
  颜色 `#RRGGBBAA` 直读皮肤 JSON（skin op 实时拉取）
- `HuFuIME/Info.plist` — IMK 注册（InputMethodServerControllerClass、tsInputSourceID、
  zh_Hans locale）
- `build.sh` — 构建 HuFuIME.app（内含 hufu-server 二进制）

## 皮肤材质对照（hufu-skin Material → NSVisualEffectView）

| Material | 材质 | 说明 |
|---|---|---|
| solid | underWindowBackground, alpha=0 | 纯色 back_color |
| translucent | menu, alpha≈0.55 | 半透明 |
| frosted | underPageBackground | 磨砂（近似 Acrylic） |
| glass | fullScreenUI | 玻璃 |

## 构建（需在 Mac 上）

```bash
platform/macos/build.sh        # 产出 build/HuFuIME.app
# 启动引擎 + 安装 + 注销重登
```

## 与 Windows 前端的功能对齐表

| 能力 | Windows TSF | macOS IMK |
|---|---|---|
| 键→引擎 IPC | 命名管道 | Unix socket（同帧协议） |
| 组段 | ITfComposition | setMarkedText |
| 上屏 | SetText+EndComposition | insertText |
| 候选窗 | 分层 GDI（v2→D2D+Acrylic） | NSVisualEffectView |
| 皮肤 | weasel 互导 JSON | 同一 JSON |
| 试用台/设置 | localhost Web UI | 同一 Web UI |
