# 游戏宿主支持（子类化流派，2026-10-06 定稿）

## 结论
LOL 等全屏游戏进程内，**任何**前景 `AdviseKeyEventSink` 挂载都会让
msctf/CUAS 为文本无关窗口装上键盘吞键管线——对局键盘整体失灵（sink
零回调、切走输入法不恢复）。2026-10-05 消融 A1/A2/E6/E6v2 多轮实锤，
先拆 IME 链（ImmAssociateContextEx NULL）也护不住。

定稿方案（逆向虎娘/Tigirl 同款流派）：游戏宿主**永不挂 TSF 键 sink**，
改为**子类化游戏窗口**（SetWindowLongPtrW 换 WndProc）：

- 进场（Activate 命中名单/全屏形态）：拆焦点窗 IME 链 + 标记游戏模式；
- 首个焦点事件：子类化游戏窗（原 proc 存静态原子量）；
- WndProc 按键流推断聊天态：对局 Enter=开聊天，聊天中 Esc/Enter=关；
- 聊天开：WM_KEYDOWN 喂引擎（ipc key_request），被消费的键返回 0 吞
  掉；其派生 WM_CHAR（游戏泵 TranslateMessage 在 Dispatch 前已生成）
  按 lParam 扫描码匹配一并吞；上屏=PostMessage WM_CHAR（lParam=1 作
  注入标记，真键派生字符 lParam 扫描码位非 0 永不误吞）；
- 候选+编码行：推 server 跨进程置顶窗，固定屏幕位（游戏窗左下，
  聊天输入区上方）；
- 聊天关：一切原样转交原 proc（技能键原生畅通）；
- Deactivate：还原原 proc。

## 宿主判定（二选一命中即游戏模式）
1. `C:\ProgramData\HuFu\game-hosts.txt`：每行一个小写进程名，# 注释
  （league of legends.exe 已内置，无需名单）；
2. 进程内存在全屏弹出窗（EnumWindows 进程级扫描，≥显示器 99.5%）。

## 已知限制（均实测确认）
- **独占全屏**：候选窗不可见（独占模式下除游戏画面外无叠加层物理
  不可见；进程内建窗会触发失焦弹桌面）。打字/上屏/技能一切正常。
  **解法：游戏视频设置切「无边框窗口」**（候选+编码完整显示）。
- 聊天开着时按 Enter=直接发送草稿（未上屏编码随之发出）——先空格
  上屏再 Enter；
- 聊天中 Esc 关闭后引擎组段未冲销（下次开聊天续旧编码），输入继续
  或空格上屏即可；minor，待后续修；
- 对局中切走输入法再切回：Deactivate 还原子类化路径存在但**未实测**。

## 禁止事项（改代码前必读）
- 游戏进程内不要 AdviseKeyEventSink（任何参数组合，含 fForeground=0）；
- 不要在游戏进程内创建任何可见窗口（owned/无主/child 全部实锤弹桌面
  或画不上）；
- 不要对已 advised 的 sink 调 UnadviseKeyEventSink（僵尸吞键）。
