# 游戏宿主支持（子类化流派，2026-10-06 定稿）

## 【2026-11 重要变更】默认改为"游戏宿主完全惰性"

用户口径：**游戏里虎符打不了字可以接受，但必须能切到别的输入法**。
原先"命中游戏形态 → 拆 IME 链 + 子类化"的流派有两个副作用会毁掉这条底线：

1. 在无 TSF 文档的进程里挂键 sink → CUAS 武装吞键管线（整进程键盘死、拆不掉）；
2. `ImmAssociateContextEx(hwnd,NULL,0)` 拆链 → **该窗口任何输入法都接不上**，
   切到微软拼音也打不了字。

现在的默认行为（`tsf.rs` `Activate`）：

- 命中游戏特征（全屏弹出形态 / 进程名名单 / 反作弊模块 / 游戏输入栈模块且无 TSF 文档）
  → **完全惰性**：不挂 sink、不拆链、不子类化。键盘与输入法切换完全交给系统。
- **UI-less 接管（2026-11）**：`candidate_ui::publish` 读取 `BeginUIElement` 的 `pbShow`；
  FALSE（宿主自绘）时 `candidate_ui::host_draws()` 为真，`candwin2::show` 直接收窗不再自绘。
  例外名单（cs2/dota2/deltaforce）继续自绘——它们只替内置白名单输入法名画候选，对虎符不画。
  `state_request()` 轮询响应里无条件 `publish` 一次，保证后台重排/云候选到达后候选页 ≤140ms 刷新
  （宿主自绘路径不走 `update_ui`，只靠这一处）。
- **硬/软两级判定（2026-11 B 方案）**：硬否决（全屏弹出窗/进程名单/反作弊）= 永不接管；
  软否决（游戏输入栈模块 + 无文本焦点）= 先惰性，`OnSetFocus` 出现可编辑焦点时补挂，
  补挂前用 `caret_evidence`（GetSelection，fail-closed）取证，且 `game_hard_veto()` 复检
  （进程内一旦有全屏窗口就不补挂）。误判的正常软件点进输入框会自动恢复中文；
  真游戏（无可编辑焦点）永远不接管。trace 里 `class=hard` / `class=soft` 可用于日志归类。
- **准入制（2026-11）**：即使不是游戏，`Activate` 时若拿不到文档焦点（没有能输入的场合），
  也**不挂键 sink**，改为等 `OnSetFocus` 出现真正的文档焦点再补挂。没有挂载就绝不会
  武装 CUAS——这是"任何情况下都不影响切输入法"的结构性保证，用户无需做任何操作。
- `league of legends.exe` 保持旧流派（已适配的已知用例）。
- 其他游戏想要"游戏内打字"：建旗标文件 `C:\ProgramData\HuFu\game-takeover`，
  恢复旧的子类化流派（含拆链，风险自担）。
- **误判白名单**：`C:\ProgramData\HuFu\not-game.txt`（每行一个 exe 名，# 注释），
  命中的进程永不按游戏处理（比全局 game-takeover 安全，用于播放器/模拟器/工具类误判）。
- 急停：`C:\ProgramData\HuFu\caps-no-hook`（caps 虚拟化让路）。

验证方法（无需真实游戏）：给宿主进程设环境变量 `HUFU_FORCE_GAME=1`，
trace 应出现 `activate: 硬否决(class=hard) → 完全惰性`，且窗口内键入的字母直接进入控件。

## 结论（2026-10-06 旧版，保留作背景）
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
