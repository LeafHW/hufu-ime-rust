# 虎符设置（新）

React 19 + TypeScript + Vite 的新设置窗口。**10 个页面全部接入真实数据**：概览、方案、输入、键位编辑、皮肤工作室、候选挂件、整句模型、数据与备份、云同步、关于。
窗口壳已是 `hufu-settings.exe` 默认入口（`/next?shell=1`），旧设置页 `/` 保留作回滚。严格验收结果与已知差异见 `hufu/docs/设置窗口验收报告.md`。

## 运行

前置：`hufu-server` 在 `127.0.0.1:4390` 运行（本机输入法常驻即可）。

```bash
cd hufu/settings-app
npm install
npm run dev        # http://127.0.0.1:5173
```

Vite 会把 `/api` 代理到 `http://127.0.0.1:4390`，所以页面读的是真实配置/状态/皮肤/同步数据。
生产构建：

```bash
npm run build      # dist/
```

## 当前已接的真实能力

- **壳**：标题栏、侧栏、三态主题（跟随/深/浅）、Hash 路由、Toast。
- **概览**：`/api/about`、`/api/state`、`/api/config`、`/api/skins`、`/api/skin`、`/api/sync/status`、`/api/sync/schemes` 真实读取；最近切换/同步时间/设备/方案/文件数都来自本机。
- **试打**：真实 `<input>`；聚焦后静态预览让位，输入法在设置窗内画真实候选窗。
- **皮肤工作室**：真实皮肤列表（19 个 JSON + 4 个 .ssf）；选中即读 `GET /api/skin?id=`；候选窗按皮肤颜色/字号/圆角/透明度实时渲染；透明度/高亮/圆角/字号/间距滑杆可调；`保存` 调 `POST /api/skin` 真写盘；A/B 对比；SSF 显示预览图，完整编辑器在下一阶段。
- **云同步**：真实连接状态、设备、上次同步、方案勾选与文件数；`立即同步` 调 `POST /api/sync/now`；连接保存+测试、勾选保存、自动同步开关、云端文件/回收站管理面板均接真实接口。
- **方案**：真实方案列表、切换、打开文件夹、导出码表、反查/拆分保存（先取最新 config 再合并）。
- **输入**：中英切换/编码/候选/标点/音效/OpenCC/剪贴板真实读写；音效试听与拆分方案来自真实接口。
- **键位编辑**：主键盘/小键盘/Shift+符号三分页，真实 `keymap.map/map_idle/shift_symbols/shift_letters_always` 写入，冲突检测。
- **整句模型**：真实模型状态、开关、12 项权重、神经重排、`/api/sentence_test` 真实候选。
- **候选挂件**：真实矢量/SSF 皮肤 overlay 读取、图片与预设、裁剪/贴法/透明度/延时保存。
- **数据与备份**：真实 `/api/settings_export` / `/api/settings_import`，带摘要确认。
- **关于**：真实版本/目录/GitHub。

## 编码内联开关（2026-11-05）

皮肤工作室工具栏在「横排」旁恢复「编码内联」，完整含义为「编码内联到应用」。

- **沿用原契约**：JSON 皮肤 `layout.inline_preedit: boolean`，缺省 `true`；不是 `config.json` 的全局配置，不创建 `appearance.inline_preedit` 等新键。
- **真实保存**：先 `GET /api/skin?id=…` 读取最新皮肤，只改该字段，再 `POST /api/skin`（`select:false`）。不顺带提交编辑器未保存值、不丢挂件、不切换当前皮肤；服务器写盘并推进 `skin_ver`，当前皮肤下次打字使用新值。
- **运行语义**：开启时编码进入应用预编辑组段、候选窗不重复显示；关闭时不建编码组段，编码仅显示在候选窗。静态预览同步这一口径，真实输入仍以 TSF 宿主验证为准。
- **SSF 支持（2026-11-05 晚）**：`SsfOverrides` 新增 `inline_preedit` 覆盖（server 投影 `layout` 同键下发，DLL 零改动）。选位图皮肤时开关可拨动，立即写入该皮肤 `overrides.json`（per-skin 持久化），缺省=内联。
- **部署验收（2026-11-05 晚）**：构建链 dist→cargo build→嵌图标→热换 D:\HuFu 全通；API 回读验证通过。
- **不部署即可验收**：启动 `npm run dev`，打开 `http://127.0.0.1:5173/?shell=1#skin`（代理真实 `4390` 服务）。切换开关后读取同皮肤 API、刷新页面，再在支持 TSF 的输入框里试打；最后恢复开关原值。只浏览别款皮肤不会影响当前皮肤，须显式「设为当前」后才用于输入。
- **本轮实测**：TypeScript 与生产构建通过；1038×740、980×740、900×740 无页面横向溢出且两开关可见；CDP 真点击写后读回、刷新保持、恢复原皮肤完整对象和全局配置不变通过；SSF 禁用通过。证据在 `.review/inline-preedit/`，没有部署或发布。

## 已知差异 / 未覆盖

严格验收未通过项（多显示器混合 DPI、Snap Layouts、旧页个别控件、动图挂件预览、完整发行包等）逐条列在
`hufu/docs/设置窗口验收报告.md` 第 5 节；不要把本 README 当作“旧页 100% 等价”的声明。`.ssf` 完整可视化编辑与实时 SSE/rev 仍属后续。

## 结构

```
src/
  api.ts / types.ts         API client 与真实数据结构（含离线 fallback）
  components/               Shell（标题栏/侧栏）、Candidate、Icon、UI primitives
  pages/                    Overview / Schemes / Input / Keymap / SkinStudio / Buddy / Sentence / Data / Sync / About
  styles/app.css            从视觉稿迁移的 tokens + 组件样式
public/hufu-icon-q-256.png  Windows 安装器 Q 虎头
```

## 设计依据

- 方案：`hufu/docs/设置窗口重设计.md`
- 视觉稿 / 无构建 demo：`hufu/docs/design/`
- 视觉方向 A；图标 Q 版虎头；云同步合符 = Q 虎头一分为二 + 错金缝。

## 在 WebView2 壳里跑

壳在 `hufu/platform/windows/hufu-settings`（P0 已跑通，x86_64-pc-windows-gnu，无 MSVC/WinRT）：

```powershell
cd E:\DSH-KF\hufu\platform\windows\hufu-settings
cargo build --release --target x86_64-pc-windows-gnu
.\run.ps1          # 默认 /next（server 已部署）
.\run-dev.ps1      # 仅前端 HMR 开发：自动确保 Vite 在 5173
```

React 在 `?shell=1` 下显示自定义标题栏；原生 caption 已去掉（`WS_POPUP|WS_THICKFRAME`），
拖拽和最小化/最大化/关闭通过 WebView2 WebMessage（`win:drag|minimize|maximize|close`）交给宿主。
日常入口 `run.ps1` 直接加载 server 的 `/next`。

实机截图（PrintWindow，1280×800）：

- 概览：`screenshots/shell-overview.png`
- 皮肤工作室：`screenshots/shell-skin.png`
- 云同步：`screenshots/shell-sync.png`
- `/next` 实机：`screenshots/shell-next.png`
- Ctrl+Alt+H 拉起：`screenshots/shell-hotkey.png`
- 自定义标题栏：`screenshots/custom-chrome-next.png`
- 全页面：`screenshots/all-pages.png`（10 页，1280×800 实机）
- 缩放矩阵（100/125/150/200%）：`screenshots/dpi-matrix.png`
- 200% 皮肤抽屉 / 云同步：`screenshots/dpi-200-skin.png` / `screenshots/dpi-200-sync.png`

## DPI / 响应式（2026-10-31）

- `?shell=1` 下窗口填满 WebView 视口；壳按 DIP 定尺寸，WebView2 跟桌面缩放比走。
- 断点：≥1180 完整侧栏 + 三栏；980–1180 侧栏收 64px + 三栏（列表/检查器收窄，工具栏收起缩放/A/B）；<980 侧栏 64px + 两栏，检查器变「参数」右侧抽屉；<760 列表在上、表格单列。
- 概览 <980 状态卡 2 列、试打说明隐藏、常用操作下移；云同步 <980 单列；小高度一律页面滚动，不再硬裁。
- 已渲染验证 100%（1280×800）、125%、150%（1280×693）、175%（spot-check）、200%（960×520）；矩阵见 `screenshots/dpi-matrix.png`，200% 皮肤/同步见 `screenshots/dpi-200-skin.png` / `dpi-200-sync.png`，175% 见 `dpi-175-skin.png`。
- 壳侧用 Per-Monitor V2 + `WM_DPICHANGED`；开发截图可用 `--dpi 144/192` 模拟。自定义标题栏是 CSS/WebView 内容，跟着缩放比走；浏览器矩阵只看内容布局。

## 服务端 /next（已部署 2026-10-31）

- `hufu-server/build.rs` 会把本目录的 `dist/` 嵌进 server exe：`GET /next` 返回
  `dist/index.html`，`GET /assets/*` 返回哈希资源，`/hufu-icon-q-256.png` 也一起；
  老 `GET /` 仍是旧页，互不影响。
- 构建顺序：`npm run build` → 重建 `hufu-server`。`dist/` 不再被 `.gitignore` 忽略，
  发布机不需要 Node。
- 已验证：`cargo test --release --target x86_64-pc-windows-gnu -p hufu-server next_ui`
  2 个测试通过（嵌入表 + 真实 HTTP GET `/next`/JS 资源）。
- 已替换当前 4390 的 live server（旧版备份 `D:/HuFu/hufu-server.exe.bak-20261031-195621`）；`GET /next`、`/assets/*`、`/` 均 200。`Ctrl+Alt+H`、语言栏管道、托盘菜单都会拉起新壳。壳可手工这样开：

```powershell
hufu-settings.exe --url "http://127.0.0.1:4390/next?shell=1"
```

日常 `run.ps1` 走 `/next`；`run-dev.ps1` 仅前端 HMR 开发用。

## P1 状态

已完成：壳、概览、皮肤工作室、云同步；真实 API 读 + 皮肤保存；主题三态；
1080/125/150/200% 响应式（侧栏收窄、参数抽屉、页面滚动）；`/next` 已部署，
壳默认 `/next`，`Ctrl+Alt+H`/语言栏管道/托盘菜单已拉起新壳。
已完成自定义标题栏（原生 caption 去掉，HTML 拖拽 + 最小化/最大化/关闭接 WebView2 WebMessage，
最大化/还原/拖拽/关闭实机验证）。
未完成：方案/输入/整句/数据/挂件/键位等页、同步写入、SSE/rev、
`设置.bat`/开始菜单/安装器迁移、多显示器混合 DPI 实机。
