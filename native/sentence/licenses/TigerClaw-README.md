# 虎爪输入法（TigerClaw）

面向 Windows 的 TSF 输入法，采用 TSF 前端、C++ Core 与独立界面进程的分离架构。

主线 Core 位于 `next/TigerClaw.Core.Native/`，标准产物仍为 `TigerClaw.Core.exe`。
`publish.bat`、`publish_arm64.bat` 和 `next/build_next.bat` 默认构建 C++ Core；
`publish_core_arm64.bat` / `publish_arm64_cpp_core.bat` 用于 ARM64 Core 单独更新。
C# Core 保留作历史实现与差分测试参考；全拼开发在独立 claw_pinyin 仓库。

## 文档入口

- [用户使用说明书](用户使用说明书.md)
- [整句输入使用说明](整句虎使用说明.md)
- [开发文档索引](docs/README.md)
- [项目状态与开发约定](AGENTS.md)

## 许可证

Copyright (C) 2026 Yu Cun (lvyww).

本项目自身的原创源代码、脚本及配套项目文档采用 **GNU General Public License version 3 only（GPL-3.0-only）**。完整许可证见 [LICENSE](LICENSE)。本声明不自动授权使用未来版本的 GPL。

第三方代码、参考实现、依赖和独立授权的资源保留各自版权及许可证；本项目的授权声明不覆盖或撤销这些许可。已核对的主要来源及授权范围见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。

```text
TigerClaw - a Windows TSF input method.
Copyright (C) 2026 Yu Cun (lvyww)

This program is free software: you can redistribute it and/or modify
it under the terms of version 3 of the GNU General Public License as
published by the Free Software Foundation.

This program is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of
MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
GNU General Public License for more details.

You should have received a copy of the GNU General Public License
along with this program. If not, see <https://www.gnu.org/licenses/>.

SPDX-License-Identifier: GPL-3.0-only
```

`LICENSE` 末尾的 “How to Apply These Terms” 是许可证原文附带的通用示例；本项目采用的版本范围以本页的 `GPL-3.0-only` 声明为准。

## x64/x86 精简包

双击 `publish_lite.bat` 构建独立精简包，不安装。保留四套普通码表、反查和 21 款正式皮肤；不包含整句方案、外部模型、内置字体。程序整句能力保留。新配置使用微软雅黑、字号 24，已有配置保持。完整打包入口仍保留。
