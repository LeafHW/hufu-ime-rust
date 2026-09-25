<!-- SPDX-FileCopyrightText: 2026 明雅流风 <crrvx@outlook.com> -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# platform/linux/branding — 虎符 Linux 端品牌图形

**唯一源是 `hufu.png`**：1600×1600 RGBA 的「符」字艺术图（本仓自绘：近白字形 + 暖橙辉光）。
其余文件都由它派生，**不要手改派生文件**：

```sh
python3 platform/linux/branding/build-icons.py
```

生成器只做**版式归一**，不重绘、不降画质：取可见内容（alpha 高于 3%，忽略几乎不可见的辉光尾）
的外接正方形 + 四周 8% 留白、内容居中（当前画布 `viewBox="197 185 1194 1194"`），再据此派生：

| 文件 | 形态 | 用途 |
|---|---|---|
| `hufu.png` | 1600×1600 RGBA，**主源** | 各端按需另生成尺寸 / 格式；本仓不安装它 |
| `hufu.svg` | 自包含：`viewBox` = 归一化画布，主源以 data URI **逐字节内嵌** | 支持 SVG 的主题（缩小渲染到任意尺寸都清晰；放大超过 1600 px 才会软化） |
| `hufu-22.png` / `hufu-48.png` | 22 / 48 px RGBA，同一画布**面积平均**缩小 | 位图主题（或缩放器不可用）时的回退 |

生成器只用标准库（PNG 的解码/编码都在脚本内），**产物可重复**：同一份主源、同一 zlib 实现下
逐字节一致——PNG 的压缩字节由 zlib 实现决定（zlib-ng 与上游 zlib 对同一份像素产出的字节不同），
所以守卫比的是内容：`hufu.svg` 逐字节、位图**逐像素**。

`platform/linux/checks/check-branding.sh` 核对：主源是 RGBA 且边长足够；`hufu.svg` 自包含、且
**内嵌的正是当前主源**；位图尺寸与文件名一致且为 RGBA；**重跑生成与仓内产物比对**（SVG 逐字节、
位图逐像素：换过主源没重跑、或手改过产物都会失败）；有 `rsvg-convert` 时再查 SVG 可渲染
（**不比对字节**：不同 librsvg 版本渲染结果可能不同，逐字节会把版本差异误报成回归）。已接入
`ci-local.sh`。

## 取用方式

| 场景 | 取用 |
|---|---|
| fcitx5 输入法条目 | `conf/hufu.inputmethod.conf` 的 `Icon=hufu`（按主题名解析） |
| 状态区「虎符」菜单 | `hufu.cpp` 的 `menuAction_.setIcon("hufu")` |
| 桌面项 | `desktop/hufu-settings.desktop` 的 `Icon=hufu` |
| 安装落点 | `install.sh` 装到 `~/.local/share/icons/hicolor/{scalable,22x22,48x48}/apps/`（卸载对称删除） |

新增尺寸或格式时：先加进生成器的 `SIZES`（或按需另生成），再由各处取用——不要让各处各自维护一份
图形。
