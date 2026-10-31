# 虎符设置 · 视觉稿 v0.2

静态设计稿 + 一个可交互的**无构建 Vanilla** 演示；都不是产品代码。用于确认视觉方向 A
（现代中性 + 墨底错金 / 合符 / 虎纹）与前端路线。

## 看图

- `01-overview-dark.png` — 概览 · 暗色（1280×800 @2x）
- `02-skin-dark.png` — 皮肤工作室 · 暗色
- `03-sync-dark.png` — 云同步 · 暗色（Q 虎头「合符」）
- `04-overview-light.png` — 概览 · 亮色
- `02b-skin-light.png` / `03b-sync-light.png` — 皮肤 / 同步 · 亮色
- `05-vanilla-skin.png` — 无构建 Vanilla 演示（皮肤工作室，右下角有演示标识）

## 图标

按用户拍板，采用 **Windows 安装器当前版（Q 版虎头，8/29）**：
`platform/windows/setup/icons/虎符.ico`。视觉稿里放在深色印章底 + 细金边上，
26/44px 可读；云同步页把 Q 虎头一分为二，做成「合符」主视觉。

- `hufu-icon-q-256.png` — 本稿采用的 Q 虎头
- `hufu-icon-new-256.png` — 9/25 Linux「符」字艺术图，留作备选/次级品牌
- `icon-compare.png` — 三代图标对照

## 无构建 Vanilla 演示

直接双击 `vanilla-demo.html`（或拖进浏览器）即可，**没有任何依赖、没有构建步骤**：

- 文件：`vanilla-demo.html`（约 58KB）+ `demo.css`（2KB）+ `demo.js`（7KB）
- 可交互：点侧栏切页（概览/皮肤/同步；其他页给提示）、三态主题、
  点皮肤列表换「墨言/雨竹/鲸语」、切预览背景（棋盘/深/浅/壁纸）、
  拖透明度/圆角/阴影滑杆实时改候选窗、A/B 对比、按钮 toast。
- 生产级 vanilla 也可以继续拆成 ES modules（`pages/*.js`、`components/*.js`），
  同样不需要构建；但状态、弹层焦点、键盘可达性、表单校验都要手写。
- 结论：视觉与 React 版可以完全一致；差别在工程组织与维护成本。

## 自己打开

静态稿：`mock.html?s=overview|skin|sync`，追加 `&theme=light` 切浅色。

设计稿按 **1280×800** 宽屏展示；实际默认窗口 1080×720 时，皮肤工作室会切到
“两栏 + 参数抽屉”，概览会隐藏试打右侧说明。断点见 `../设置窗口重设计.md` 5.4.1。

## 说明

- 截图由 Edge headless 渲染（`--force-device-scale-factor=2`）。
- 图标、配色、候选窗都是真实 CSS/SVG/位图，不是贴图。
- 视觉稿只用于方向确认；真实数据、可访问性、键盘操作在 P1 实现。
