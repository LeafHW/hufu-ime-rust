# 输入法前端研究纪要（Rime / TigerClaw）

来源：weasel (小狼毫) WeaselTSF 与 Squirrel (鼠须管) 源码调研 + TigerClaw 逆向分析（2026-02，GitHub 拉取）。

## Windows TSF（weasel 路线）

- COM DLL（`weasel.dll`），注册为 TextService（`ITfTextInputProcessorEx`），CLSID 注册在 `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Run` 无关 —— 注册走 `regsvr32` / `DllRegisterServer`，`HKLM\SOFTWARE\Microsoft\CTF\TIP\<CLSID>` 键。
- 进程模型：TSF DLL 薄壳（`WeaselTSF`）+ 前台服务进程（`WeaselServer`，命名管道 `weasel.{pid}.ipc` 级联查找：DLL 尝试本进程 `weasel.ipc` → 系统命名管道广播）。HuFu 采用同构方案：`hufu-tsf.dll`（纯 Rust + windows-rs，本机无 Windows SDK 时以 windows-rs crate 的导入库链接，无需 C++ 工具链）+ `hufu-server` 守护进程。
- 候选窗：weasel 用裸窗口 + Direct2D 绘制（`VerticalCandidateWindow`/`HorizontalCandidateWindow`），CompositionWindow 内联编码。HuFu 用 D2D + DirectComposition，层窗口（WS_EX_NOREDIRECTIONBITMAP + WS_EX_LAYERED + WS_EX_TOOLWINDOW|NOACTIVATE），DWM 系统 backdrop（Acrylic=CONTROLLERBackdropType::Acrylic / Mica）实现材质。
- 安装：`weasel` 用 WiX 安装包 + elevhtp 注册；HuFu 用 `regsvr32` 自注册 + `hufu-setup`（可选 MSI 后续补）。
- 关键接口链：`ITfTextInputProcessor::Activate` → 取 `ITfThreadMgr`/`ITfDisplayAttributeMgr`，`AdviseKeyEventSink`，`ITfCompartmentMgr` 键盘开关；`ITfTextEditSink` 焦点跟随；`SetFocus` 时 `ITfContext::GetSelection` 定位 composition；`ITfInsertAtSelection::InsertTextAtSelection` 起组；`ITfRange::SetText` 更新；`ITfCandidateList` UIElement 或自绘窗（weasel 自绘，HuFu 同）。

## macOS IMK（Squirrel 路线）

- `IMKInputController` 子类（Swift/ObjC），`InputMethodServer` + `Info.plist` 的 `tsInputSourceID`/`tsInputModeCharacterRepertoire` 注册；置于 `~/Library/Input Methods/`。
- 候选窗 `IMKCandidates`（可自绘 NSView 替代）；皮肤材质用 `NSVisualEffectView`（vibrancy: behindWindow / underWindowBackground），与 HuFu `Material::Frosted/Glass` 对应。
- Squirrel 的 `squirrel.yaml` 字段（color_scheme、style/font_point…）与 weasel 同构 —— HuFu 的 weasel 配色互导入（`hufu-skin::from_weasel_colors` / `to_weasel_patch`）覆盖两者。

## TigerClaw（虎爪）行为语义

- **顶功**：编码长度上限（默认 4）。第 5 码、或死端字母（无续码）时，前串首选自动上屏、新字母成为新串起点。整句模式接管时不顶功（sentence_takeover 守卫）。
- **选重**：`;` = 次选、`'` = 第三、数字 = 第 N 候选；若 `;`/`'` 与现有编码可续（trie 有续码），优先作编码字符。
- **整句**：方案名含「整句」自动启用；TCSKNM02 ngram + beam（宽 200）；奖励/惩罚权重（出字奖励 2.0、名次惩罚 0.03、孤立生僻 λ=9 前 4 码不受限 / λ=2、置信 0.995、补充语料 9.0/2.0/16.0）；提前上屏：同一前缀提案连续 3 键稳定即自动上屏。
- **配置**：`config.txt` 45 键（界面语言、皮肤、候选数、模糊音、自动造词、热键…）—— HuFu 全部映射进 `hufu-config` JSON，由设置 GUI 编辑，不暴露 YAML/Lua。
- **皮肤**：PNG/九宫格背景 + 透明色键；HuFu 用矢量皮肤模型（19 颜色角色 + 布局 + 材质）替代位图，同时提供 weasel 配色互导。

## 已验证不采用

- TCSKNM01 v2 加密模型（无法加载，且许可不明）。
- GitHub 上的「虎码」第三方仓库（SEO 垃圾仓，码表质量差）；一律使用本地 TigerClaw/Rime 发行数据。
