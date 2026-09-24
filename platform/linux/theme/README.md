<!-- SPDX-FileCopyrightText: 2026 明雅流风 <crrvx@outlook.com> -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# platform/linux/theme — 虎符皮肤 → fcitx5 主题包（转换工具 + 说明）

引擎的 9 套皮肤（`engine/crates/hufu-server/official-skins/*.json`）是给 **Windows 自绘候选窗**
用的；Linux 的候选窗是 fcitx5 的面板，样式只能来自 **fcitx5 主题**。这里把每套皮肤**离线**
转成一套 fcitx5 主题包，随包安装到 `~/.local/share/fcitx5/themes/hufu-<id>/`——
**主题名与皮肤 id、中文名一致**（在引擎设置页选「墨岩」，到 fcitx5 里也选「墨岩（极简黑）」）。

产物落在 **`platform/linux/themes/hufu-<id>/`（纯生成目录：只有产物，不放任何手写文件**，
校验脚本据此逐字节比对）；由本目录的 `build-themes.py` 生成，**不要手改产物**。改皮肤源或改
转换脚本后重跑：

```sh
python3 platform/linux/theme/build-themes.py     # 重新生成 9 套主题包
bash platform/linux/checks/check-themes.sh       # 自检：重跑转换与仓内产物逐字节比对
```

`check-themes.sh` 已接入 `ci-local.sh`（第 ⑨ 步）：皮肤源改了却没重跑转换、或主题包被手改，
CI 都会失败。产物必须可重复——同一份皮肤 JSON 逐字节生成同样的文件（转换脚本不用时间戳、
zlib 级别固定）。

## 怎么选

`fcitx5-configtool → 附加组件 → Classic UI → 主题`（或 `~/.config/fcitx5/conf/classicui.conf`
的 `Theme=` / `DarkTheme=`）里选 `虎符皮肤名`。注意**主题是全局的**：换虎符主题会一并改掉
其它输入法的候选窗样式——这是 fcitx5 主题体系本身的性质，不是本项目的取舍。

## 映射表（皮肤 → fcitx5 主题键）

| 皮肤字段 | fcitx5 落点 |
|---|---|
| `colors.back_color` | `panel.png` 填充（带 alpha；`[Menu/Background] Color` 兜底） |
| `colors.border_color` + `layout.border_width` | `panel.png` 描边 + `[Menu/Background] BorderColor/BorderWidth` |
| `layout.corner_radius` | `panel.png` 圆角 + `[InputPanel/Background/Margin]`（九宫格切片） |
| `layout.hilited_corner_radius` | `highlight.png` 圆角 + `[InputPanel/Highlight/Margin]` |
| `colors.hilited_candidate_back_color` | `highlight.png` 填充 + `[InputPanel] HighlightBackgroundColor` |
| `colors.candidate_text_color` | `[InputPanel] NormalColor`（翻页箭头/菜单勾选也取它） |
| `colors.hilited_candidate_text_color` | `[InputPanel] HighlightCandidateColor` |
| `colors.hilited_candidate_label_color` | `[InputPanel] HighlightColor` |
| `layout.margin_x` / `margin_y` | `[InputPanel/ContentMargin]`（`[Menu/ContentMargin]` 同值） |
| `layout.hilite_padding` | `[InputPanel/TextMargin]`（`[Menu/TextMargin]` 同值） |

## 做不到的（有意为之，不是 bug）

- **模糊 / 材质**：fcitx5 classicui 有 `EnableBlur` + `BlurMask`（且需合成器支持），但引擎 9 套
  官方皮肤的 `material.kind` **全是 `solid`** ⇒ 转出来永远不会触发，故不做。将来真出现
  `glass/frosted` 皮肤时，加 `EnableBlur=True` + `BlurMask=mask.png`（同圆角）约 15 行。
- **动效**：入场/高亮滑动/上屏停留等是自绘候选窗的能力，fcitx5 主题没有对应键。
- **每候选配色**：`comment_text_color`、`label_color`、各类 `*_shadow_color` 无对应键——
  fcitx5 只有「普通/高亮」两组文字色与一组高亮背景色。
- **序号样式**：`label_format`/`label_style`（中文数字、罗马数字）不可表达，序号由 fcitx5 画。
- **字体**：`font_face` / `font_point` 是 fcitx5 的 UI 全局项（`classicui.conf` 的 `Font=`），
  主题里没有字号/字体键，脚本也不代改用户配置。
- **底几乎全透明时抬到可读下限**：转换时 `back_color` 的 alpha 会被抬到 ≥ 0.85（fcitx5 无模糊
  合成，否则如「迷雾改」alpha=0.08 会看不清）。这是转换脚本里唯一的「有损」处理。
- **kimpanel 用户不生效**：那是另一套面板体系（外观由桌面 shell 决定），本主题只作用于 classicui。
