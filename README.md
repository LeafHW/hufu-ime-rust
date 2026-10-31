# 虎符 HuFu — 以虎码为核心的跨平台输入法

> 名字取自古代调兵信物「虎符」：以虎码为主码的输入法平台，Windows / Linux / macOS 三端
> （Windows 真机全通；Linux fcitx5 通用输入主链可用；macOS开发中）。
> 目标：吸收 **虎爪输入法（TigerClaw）** 与 **Rime（虎码配置）** 的全部能力，
> 重新实现为「统一引擎 + 多平台前端 + 图形化设置」的产品级输入法。

## 主要功能

- **虎码整句**：整句输入为核心，ngram 组句 + Qwen3 神经重排，全部权重可调
- **多码表格式**：HuFu 原生 / Rime / 多多 / QQ五笔 / 虎整句，放入方案目录自动识别
- **方案管理**：方案即目录，一键切换，每方案独立用户词
- **候选交互**：多种选重键、翻页、竖排/横排、调序/置顶/软删、滚轮缩放、拖动移位、Shift 标点形态
- **符号系统**：快符、分类符号、动态变量（日期/时间）、`\calc` 计算器、`\w` 自动造词
- **繁简与注解**：OpenCC 简繁 + emoji 候选变体，拼音/拆分/Unicode 注释，双拼反查
- **皮肤**：JSON 皮肤 + 毛玻璃等材质，兼容导入 weasel/squirrel 配色
- **图形设置**：本地 Web UI 全图形化，不碰 yaml/lua；用户词管理、数据快照导入导出
- **按键音效**：4 类按键音，音量可调

功能明细详见 [docs/features.md](docs/features.md)。

## 快速指南

### **Windows（免构建）**

1. 从 [Releases](../../releases) 下载最新 Windows 安装包（v1.8.x 为
   `HuFuSetup-*.exe`；旧版为 `HuFu-IME-x.y.z.zip`，以 Release 页为准）
2. 运行安装程序（自动 UAC 提权，双阶段安装）
3. 打开新应用，Win+空格 切到「虎符」即可打字

Windows 详见 [docs/build.md](docs/build.md)

### **Linux（fcitx5）**

```sh
platform/linux/install.sh -umt  # 用户级 一键全量安装
fcitx5 -r -d       # 重启 fcitx5（或以发行版的方式）
fcitx5-configtool  # 手动添加「虎符」（或以发行版的方式）
```

- 默认数据 **随仓库自带**（`assets/`）
- 模型与主题不随 `-s/-u` 安装，分别用 `-m`、`-t` 显式独立安装
- 设置页在应用菜单「虎符设置」或 `http://127.0.0.1:4390/`

Linux 详见 [platform/linux/README.md](platform/linux/README.md)

### **macOS**（开发中，预计11月20日前后上线）

## 版权声明

- 本项目代码以 [GPL-3.0](LICENSE) 发布
- 随发行包分发的第三方组件保留其原始许可证：Qwen3 模型（Apache-2.0）、
  llama.cpp（MIT）、OpenCC 词典数据（Apache-2.0）
- 随仓库分发的码表与输入法资源来自：虎码官方发布（<https://huma.ysepan.com>）、
  虎爪输入法 TigerClaw（<https://github.com/lvyww/tigerclaw>，GPL-3.0）；
  随本仓库分发并保留原项目声明，逐文件来源详见
  [docs/asset-sources.md](docs/asset-sources.md)

## 文档索引

| 文档 | 内容 |
| --- | --- |
| [features.md](docs/features.md) | 功能总览明细 |
| [architecture.md](docs/architecture.md) | 架构设计与数据流 |
| [build.md](docs/build.md) | 源码构建与系统注册 |
| [status.md](docs/status.md) | 当前进度与测试基准 |
| [dictionary-formats.md](docs/dictionary-formats.md) | 码表格式规范（含 TCSKNM02 模型布局） |
| [platform/linux/README.md](platform/linux/README.md) | Linux 前端：简介 / 功能 / 安装卸载 / 作者声明 |
| [asset-sources.md](docs/asset-sources.md) | 资源来源、版本与许可 |
| [research/ime-frontends.md](docs/research/ime-frontends.md) | TSF / IMK 前端研究纪要 |
| [benchmark-latency-100.md](docs/benchmark-latency-100.md) · [benchmark-qwen-vs-ngram.md](docs/benchmark-qwen-vs-ngram.md) | 基准报告 |
| [platform/windows/install/README.md](platform/windows/install/README.md) · [platform/macos/README.md](platform/macos/README.md) | Windows / macOS 平台说明 |
