#!/bin/bash
# 构建 HuFuIME.app（macOS，需 Xcode Command Line Tools）
# 用法：platform/macos/build.sh [输出目录]
set -euo pipefail
cd "$(dirname "$0")"

OUT="${1:-build}"
APP="$OUT/HuFuIME.app"
CONTENTS="$APP/Contents"
MACOS_DIR="$CONTENTS/MacOS"
RESOURCES="$CONTENTS/Resources"

mkdir -p "$MACOS_DIR" "$RESOURCES"

# 1) 先构建引擎守护进程（Unix socket + HTTP + 设置 UI）
(cd ../../engine && cargo build --release -p hufu-server)

# 2) 编译输入法主体
swiftc -O \
    -framework Cocoa -framework InputMethodKit \
    HuFuIME/HuFuInputController.swift HuFuIME/CandidatePanel.swift \
    -o "$MACOS_DIR/HuFuIME"

cp HuFuIME/Info.plist "$CONTENTS/Info.plist"
cp ../../engine/target/release/hufu-server "$MACOS_DIR/hufu-server"

echo "✓ $APP"

cat <<'EOF'
安装：
  1) 先启动引擎：  ~/Library/Input Methods/HuFuIME.app/Contents/MacOS/hufu-server --data ~/.hufu &
  2) 拷贝：        cp -r build/HuFuIME.app ~/Library/Input\ Methods/
  3) 注销重登，系统设置 → 键盘 → 输入法 → 添加「HuFu 虎符输入法」
EOF
