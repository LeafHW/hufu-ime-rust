# 虎符 HuFu · 源码构建指南

> 终端用户免构建安装见 [README 快速指南](../README.md)。

## 引擎 + CLI（Windows / macOS / Linux 均可）

```powershell
cd engine
cargo build --release
cargo test

# REPL 体验引擎
cargo run -p hufu-cli -- repl --dict ../assets/码表/虎码单字
```

## Windows TSF 前端（需要 Windows；x86_64-pc-windows-gnu 工具链）

```powershell
cd platform/windows
cargo build --release          # hufu_tsf.dll + hufu-tsf-smoke.exe
./target/release/hufu-tsf-smoke.exe   # 冒烟：COM 层 + 管道引擎链
# 注册见 platform/windows/install/README.md
```

### 系统激活（真机安装）

```powershell
# 管理员终端，一条完成全部注册（COM + msctf 档案 + 分类 + 语言列表 + 切换器）
powershell -ExecutionPolicy Bypass -File "platform\windows\install\install.ps1"
# 若切换器不显示第 4 项（全局分类被清）：
powershell -ExecutionPolicy Bypass -File "platform\windows\install\reg-fix.ps1"
```

注意事项：

- `hufu-server.exe` 需先运行（管道与设置页）
- **启动早于注册的应用要重启才能用 HuFu**（QQ/DSH 实测；TSF 应用启动时缓存输入法列表）
- ctfmon 重启/注销后当前输入法会重置回默认，需 Win+空格 重选（系统行为）
- 搜索启动器（Listary 等）打中文时字母进候选——按 `Shift` 切英文直通

## Linux fcitx5 前端

```bash
cmake -S platform/linux/hufu-addon -B platform/linux/build \
    -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr
cmake --build platform/linux/build -j
# 一键安装（addon 系统级 + 引擎 systemd user 服务 + 码表装配）：
platform/linux/install.sh
fcitx5 -r -d && fcitx5-configtool   # 重启后在配置工具添加「虎符」
```

安装 / 卸载 / 功能与作者声明见 **[platform/linux/README.md](../platform/linux/README.md)**；
前端结构与构建细节亦见该 README。

## macOS 前端

尚未开始开发（仅有骨架代码）。结构见 [platform/macos/README.md](../platform/macos/README.md)。
