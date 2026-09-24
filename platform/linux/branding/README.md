<!-- SPDX-FileCopyrightText: 2026 明雅流风 <crrvx@outlook.com> -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# platform/linux/branding — 虎符 Linux 端品牌图形

**唯一矢量源是 `hufu.svg`**：深底圆角方块 + 两半错金兵符（中缝锯齿）——取「虎符」本义，
纯几何形状、**不含文字**（无字体依赖），22–48 px 下均可辨识。

各尺寸位图都由它生成，**不要手改位图**：

```sh
rsvg-convert -w 22 -h 22 platform/linux/branding/hufu.svg -o platform/linux/branding/hufu-22.png
rsvg-convert -w 48 -h 48 platform/linux/branding/hufu.svg -o platform/linux/branding/hufu-48.png
```

`platform/linux/checks/check-branding.sh` 校验「矢量源存在 + 位图尺寸与文件名一致 + PNG 为
RGBA + 有渲染器时 SVG 可渲染」，已接入 `ci-local.sh`。它**不做逐字节比对**：不同 librsvg
版本的渲染字节可能不同，逐字节会把版本差异误报成回归。

## 取用方式

| 场景 | 取用 |
|---|---|
| fcitx5 输入法条目 | `conf/hufu.inputmethod.conf` 的 `Icon=hufu`（按主题名解析） |
| 状态区「虎符」菜单 | `hufu.cpp` 的 `menuAction_.setIcon("hufu")` |
| 桌面项 | `desktop/hufu-settings.desktop` 的 `Icon=hufu` |
| 安装落点 | `install.sh` 装到 `~/.local/share/icons/hicolor/{scalable,22x22,48x48}/apps/`（卸载对称删除） |

新增尺寸或格式时：**先在本目录落地并补上生成命令**，再由各处取用——不要让各处各自维护一份图形。
