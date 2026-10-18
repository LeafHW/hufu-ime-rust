<!-- SPDX-FileCopyrightText: 2026 明雅流风 <crrvx@outlook.com> -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# AGENTS.md

虎符-输入引擎（hufu-ime），rust 实现

## 项目定位

- **虎符 hufu**：fcitx5 原生输入引擎（Rust 实现），内核独立于方案与平台
- 平台优先级：windows → linux / android → macos / ios
- 运行时不依赖 librime、不依赖 Lua

## AI 风格

1. 中文回答，简明扼要
2. 若有任何未尽事宜，随时提问/建议
3. 提出问题时，同时列举可行的方案供用户选择

## 原则

1. 自顶向下设计，自底向上实现
2. 模块化：高内聚，低耦合
3. 文档：详略得当，内容完备
4. 代码：简练，清晰
5. 测试：单元测试，集成测试

## 协作流程（若无 jujutsu，再考虑使用 git）

1. 修改开始前：在当前所在节点上，先执行 `jj new` 开新副本，再进行改动
2. 修改过程中：可按需执行更多的 `jj new`
    - 新副本只落在 AI 自己 `jj new` 的链上，不改动其他已有节点
3. 修改过程中：尽量按功能拆成多条 jj commit
4. 修改完成后：须同步对应文档，并按改动编辑 jj commit 消息（简短一句）
    - 提交后交由用户审阅确认
5. jj 历史整理, push/fetch，pr：除非用户直接要求，否则 AI 不代做

## 相关项目

- [tigirl](https://github.com/lvyww/tigirl)
- [tigerclaw](https://github.com/lvyww/tigerclaw)
- [tiger-sentense-rime](https://github.com/lvyww/tiger-sentense-rime)
- [hufu-ime-rust](https://github.com/LeafHW/hufu-ime-rust)
- [hux-ime](https://github.com/crrvx/hux-ime)

## 背景与约定

- 约定：
  - 项目/引擎/内核：中文「虎虚」，英文「hux」
  - Rust 模块布局用 `foo.rs` + `foo/`，**切忌 `mod.rs`**
- 移植纪律：
  - 计算部分机械翻译 + 差分逐位验证，交互部分按行为契约设计
  - 重构 / 优化不得改变可观测行为
- 文档：
  - 活文档只写现状（索引见根 `README.md`「文档索引」表）
  - 未完事项（待办 / 登记不修 / 待定配置项）进 `docs/open-items.md`
- 非简单任务：下放到子代理
- 指代 jj 历史节点：使用 change id 而非 commit id
- PR 描述：更应关注项目的功能/bug

## 特殊目录

- `_tmp/`：本地开发 / 临时记录
- `_external/`：用于快捷访问 本地外部文件
