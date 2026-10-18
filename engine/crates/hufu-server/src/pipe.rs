//! Windows 命名管道 IPC 服务（`\\.\pipe\hufu-ime`）。
//!
//! 帧协议：4 字节小端长度 + JSON。请求 `{"op":...}`：
//! `key`（含 key/modifiers）/ `state` / `reset` / `focus` / `ping`。
//! 与 HTTP API 共享 Host 与 parse_key。

use crate::host::{parse_key, Host};
#[cfg(windows)]
use std::io::ErrorKind;
use std::sync::Mutex;

#[cfg(windows)]
const PIPE_NAME: &str = r"\\.\pipe\hufu-ime";
const BUF: usize = 1 << 20;
/// 响应上限：皮肤 op 会携带贴图挂件图（动图 GIF 入皮肤后整份皮肤
/// JSON 实测 13MB+）——12MB 闸会把响应替换成错误对象，DLL 拿错误当
/// 皮肤渲染=全线回默认皮「迷雾」事故（2026-10-03）。抬到 32MB。
/// 请求侧仍用 BUF（请求都很小，op=cand 已改 skin_ver 不携大皮肤），
/// 两者分开防互相牵制
#[cfg(windows)]
const RESP_BUF: usize = 32 << 20;

/// 分派一个操作。返回 JSON 响应。
/// `client_exe`：管道对端进程映像名（服务端经 GetNamedPipeClientProcessId
/// 反查，不可伪造）——敏感操作（剪贴板读取）的白名单以此为准；None =
/// 反查不可用（unix 回退/极端失败），退回客户端自报值。
pub fn dispatch(
    host: &Mutex<Host>,
    req: &serde_json::Value,
    client_exe: Option<&str>,
) -> serde_json::Value {
    let mut host = host.lock().unwrap_or_else(|p| p.into_inner());
    if matches!(req["op"].as_str(), Some("focus" | "reset")) { host.candidate_export.owner.clear(); }
    match req.get("op").and_then(|o| o.as_str()).unwrap_or("") {
        "ping" => serde_json::json!({"ok": true, "server": "hufu"}),
        "key" => match parse_key(req) {
            Some(k) => {
                let schema_before = host.engine.config.schema.current.clone();
                host.session.line_end_hint = req
                    .get("line_end")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                // 【七十七修·键流落盘】server 侧诊断（HUFU_TRACE 门控）。
                // 【三十六修】旧实现 fs::write 覆盖写——trace 永远只有最后
                // 一键；且持 host 锁内逐键 env::var。改追加写 + 开关
                // OnceLock 缓存（trace 开关对 server 属启动期诊断通道）。
                let diag_tail = req.get("digit_tail").and_then(|v| v.as_str()).map(|s| s.to_string());
                static TRACE_ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
                if *TRACE_ON.get_or_init(|| std::env::var("HUFU_TRACE").is_ok()) {
                    use std::io::Write as _;
                    let _ = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(std::env::temp_dir().join("hufu-server-trace.log"))
                        .and_then(|mut f| {
                            f.write_all(
                                format!(
                                    "pid={} key={:?} digit_tail={:?} ctx={:?}\n",
                                    std::process::id(),
                                    k,
                                    diag_tail,
                                    host.session.tail_context
                                )
                                .as_bytes(),
                            )
                        });
                }
                // 【七十七修·digit_tail 后缀补齐】DLL 随空态键携带
                // TestDown 记的直通数字尾巴（不递键宿主跟打器/WinForms/
                // pain 的数字到不了 engine，tail 断粮=数字后 . 出全角；
                // 递键宿主 QQ 的 OnKeyDown 让 engine 自记）。**只补齐
                // 不覆盖**：求 digit_tail 与 engine tail 的最长公共后缀
                // 重叠，仅追加缺失部分——QQ 的正确 tail 不被污染（七
                // 十五修覆盖式同步的教训）。截尾 32 与 host.rs 同构。
                if let Some(dt) = diag_tail {
                    if !dt.is_empty() {
                        let dt_chars: Vec<char> = dt.chars().collect();
                        let tail_len = host.session.tail_context.chars().count();
                        let tail_chars: Vec<char> = host.session.tail_context.chars().collect();
                        let mut overlap = 0;
                        while overlap < dt_chars.len()
                            && overlap < tail_len
                            && dt_chars[dt_chars.len() - 1 - overlap]
                                == tail_chars[tail_chars.len() - 1 - overlap]
                        {
                            overlap += 1;
                        }
                        if overlap < dt_chars.len() {
                            let add: String =
                                dt_chars[..dt_chars.len() - overlap].iter().collect();
                            host.session.tail_context.push_str(&add);
                            let cnt = host.session.tail_context.chars().count();
                            if cnt > 32 {
                                let skip = cnt - 32;
                                host.session.tail_context =
                                    host.session.tail_context.chars().skip(skip).collect();
                            }
                        }
                    }
                }
                let owner = req.pointer("/candidate_ui/context").and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty() && s.len() <= 128).unwrap_or("").to_owned();
                let mut r = host.process_key(k);
                host.candidate_export.owner = owner;
                export_candidates(&mut host, req, &mut r["state"]);
                // Ctrl+M 切方案：落盘 + 后台重装整句（与 HTTP /api/schema
                // 行为一致；旧 setup_sentence 持锁载模型秒级卡全机打字）
                if host.engine.config.schema.current != schema_before {
                    let _ = host.engine.config.save(&host.config_path);
                    crate::reload_sentence_bg(true, false);
                    // 【换方案通知 2026-09-11】键路径响应携带标记：DLL
                    // 收到即置皮肤失效（entrance_anim 等方案相关字段跟
                    // 方案走——不通知则缓存沿用到断段+2.5s，实测切回
                    // 整句无入场动效）。
                    r["state"]["schema_changed"] = serde_json::json!(true);
                }
                // 【重排派发去重 2026-09-08】process_key 内部已调
                // after_ime_op（host.rs）——此处再调=每键双份 RerankJob
                //（worker 去抖吸收，纯浪费，审计 E-5）。
                // 音效热生效：每键带上当前音量（DLL 端 wav 数据可缓存，
                // 音量取响应值——设置页改音量无需重启/失效缓存）
                if r.get("outcome").and_then(|o| o.get("sound")).is_some() {
                    r["outcome"]["sound_vol"] = serde_json::json!(host.engine.config.sound.volume);
                }
                r
            }
            None => serde_json::json!({"error": "按键描述无效"}),
        },
        // 鼠标点击候选（Linux fcitx5 前端；index=页内下标）。语义与数字
        // 选重一致（学习、无闪帧即时上屏）；Windows 前端不使用本 op。
        "select" => {
            let index = req
                .get("index")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize)
                .unwrap_or(usize::MAX);
            let mut r = host.select_candidate(index);
            if r.get("outcome").and_then(|o| o.get("sound")).is_some() {
                r["outcome"]["sound_vol"] = serde_json::json!(host.engine.config.sound.volume);
            }
            r
        }
        // 【右键调频菜单 2026-10-06】候选窗右键菜单动作（Windows DLL）：
        // front=置顶（Ctrl+数字同语义）/ del=软删（Ctrl+Shift+数字）/
        // fwd|back|last={添加}pN 定位。page_index=页内下标（与渲染同序），
        // text=候选词（渲染→点击间重排漂移校验：不符则按词在当前页内
        // 重找）。全部只写 用户调整.txt（append_adjust_log 同码同词旧行
        // 先清=覆盖旧置顶/删除）；响应带刷新后的 state 供 DLL 即时重绘。
        "cand_menu" => {
            let action = req.get("action").and_then(|v| v.as_str()).unwrap_or("");
            let text = req.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let page_index = req
                .get("page_index")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize)
                .unwrap_or(usize::MAX);
            let page_size = host.engine.config.candidates.page_size.max(1);
            let start = host.session.page * page_size;
            // 解析目标候选：优先页内 page_index，词不符/落空则按 text
            // 重找。【全列表兜底 2026-10-06】真机 trace 实锤「每个菜单
            // 动作第一次必『候选不在页』、重试才成功」——词被异步重排
            //（rerank 350ms 去抖/顶功推位）挪出当前页窗口时页内双落空。
            // 菜单意图是「这个词」本身：全列表按词定位，返回绝对下标。
            let resolve = |want: usize, text: &str| -> Option<usize> {
                let get_page = |i: usize| {
                    host.session
                        .candidates
                        .get(start + i)
                        .map(|c| c.text.as_str())
                };
                if let Some(t) = get_page(want) {
                    if text.is_empty() || t == text {
                        return Some(start + want);
                    }
                }
                if text.is_empty() {
                    return None;
                }
                (0..page_size)
                    .find(|&i| get_page(i) == Some(text))
                    .map(|i| start + i)
                    .or_else(|| {
                        host.session
                            .candidates
                            .iter()
                            .position(|c| c.text == text)
                    })
            };
            let abs = match resolve(page_index, text) {
                Some(a) => a,
                None => {
                    // 【T8 真根因 2026-10-07】真机复测（DLL trace + 实机
                    // 快照）：菜单 owner 抢前台 → 宿主失活 → DLL 失活路
                    // 径的空码查询把 session.candidates 顶掉（候选窗画的
                    // 还是旧列表）→ 词不在全列表=「候选不在页」，重试碰
                    // 上会话恢复才成（首点必败的同款）。DLL 动作现携带
                    // 显示时的 raw：克隆会话按码喂键重建候选（不动全局
                    // 会话、不响键音、不染尾巴——tail_context 克隆保留，
                    // 重排语境同源），重建列表上再定位。
                    let raw_req = req.get("raw").and_then(|v| v.as_str()).unwrap_or("");
                    let mut s = host.session.clone();
                    if !raw_req.is_empty() && (s.raw.is_empty() || s.raw == raw_req) {
                        // 【T8 叠喂修复 2026-10-07·「变长/重复词」真因】
                        // raw 已在（==raw_req）时原实现照喂整段 → 会话变
                        // "uruuru" 6 码 → 候选表变长变怪（重复词观感）→
                        // 响应渲染错表 → 下一帧真值恢复=复现不了。先清
                        // 态再按码重建，两条分支同一条确定性路径。
                        if !s.raw.is_empty() {
                            s.clear();
                        }
                        use hufu_types::{KeyCode, KeyInput, Modifiers};
                        for ch in raw_req.chars() {
                            let _ = host.engine.process_key(
                                &mut s,
                                KeyInput {
                                    key: KeyCode::Char(ch),
                                    modifiers: Modifiers::default(),
                                    is_press: true,
                                },
                            );
                        }
                        if let Some(a) = s.candidates.iter().position(|c| c.text == text) {
                            // 在重建会话上执行动作（写 用户调整.txt 同源）
                            let total_s = s.candidates.len();
                            let act_code_s = s
                                .candidates
                                .get(a)
                                .map(|c| c.code.clone())
                                .unwrap_or_default();
                            let h: &mut Host = &mut host;
                            // 【纯位次 2026-10-07】同主路径：原位重排，
                            // 重建会话只当「找到词+基准列表」用。
                            match action {
                                "front" => {
                                    let _ = h.engine.op_move_candidate_abs(&mut s, a, 0);
                                }
                                "del" => {
                                    let _ = h.engine.op_drop_candidate_abs(&mut s, a);
                                }
                                "fwd" => {
                                    if a > 0 {
                                        let _ = h.engine.op_move_candidate_abs(&mut s, a, a - 1);
                                    }
                                }
                                "back" => {
                                    if a + 1 < total_s {
                                        let _ = h.engine.op_move_candidate_abs(&mut s, a, a + 1);
                                    }
                                }
                                "last" => {
                                    if total_s >= 1 {
                                        let _ =
                                            h.engine.op_move_candidate_abs(&mut s, a, total_s - 1);
                                    }
                                }
                                _ => {
                                    return serde_json::json!({"ok": false, "why": "未知动作"});
                                }
                            }
                            // 【整序落盘 2026-10-07】纯排序整套 pN。
                            if !act_code_s.is_empty() {
                                let _ = h.engine.adjust_set_order(&s, &act_code_s);
                            }
                            // 【占位符变号·显示同步 2026-10-09】落盘词表
                            // 已改号，重建会话的候选文本同步成新号再拷回
                            // 活会话（否则窗口仍显 ③，重开才见 ④）。
                            let _ = h.engine.sync_placeholder_texts(&mut s);
                            // 【显示序回写 2026-10-07】重建列表上的新序
                            // 拷回**活会话**（raw/页/选保活会话原样——尾
                            // 巴态不翻转）；返回的 state 即活会话真值，
                            // 与下一拍轮询同源，不再两副面孔。
                            host.session.candidates = s.candidates.clone();
                            let state = host.engine.state(&host.session);
                            return serde_json::json!({"ok": true, "state": state});
                        }
                    }
                    return serde_json::json!({"ok": false, "why": "候选不在当前页"});
                }
            };
            let total = host.session.candidates.len();
            // 【整序口径 2026-10-07】被点词条的码——动作后按它整套落盘
            //（先取码，原位重排会动列表）。
            let act_code = host
                .session
                .candidates
                .get(abs)
                .map(|c| c.code.clone())
                .unwrap_or_default();
            let h: &mut Host = &mut host;
            // 【纯位次 2026-10-07】原位移动/删除：不 clear 不重查——
            // raw/页/选全保（尾巴态不翻转成组段态＝「编码冒出来+框
            // 挪窝」根除，trace 实锤动作前 raw='' 动作后 raw='uru'）。
            match action {
                "front" => {
                    let _ = h.engine.op_move_candidate_abs(&mut h.session, abs, 0);
                }
                "del" => {
                    let _ = h.engine.op_drop_candidate_abs(&mut h.session, abs);
                }
                "fwd" => {
                    if abs > 0 {
                        let _ = h.engine.op_move_candidate_abs(&mut h.session, abs, abs - 1);
                    }
                }
                "back" => {
                    if abs + 1 < total {
                        let _ = h.engine.op_move_candidate_abs(&mut h.session, abs, abs + 1);
                    }
                }
                "last" => {
                    if total >= 1 {
                        let _ = h.engine.op_move_candidate_abs(&mut h.session, abs, total - 1);
                    }
                }
                _ => {
                    return serde_json::json!({"ok": false, "why": "未知动作"});
                }
            }
            // 【整序落盘 2026-10-07】纯排序整套 pN（清陈旧 {添加}/{置顶}
            // 行）——不动会话（原位重排后的列表就是展示真值）。
            if !act_code.is_empty() {
                let _ = h.engine.adjust_set_order(&h.session, &act_code);
            }
            // 【占位符变号·显示同步 2026-10-09】同上：落盘已改号，活会话
            // 文本同步（raw/页/选不动，窗口立即显示 ④）。
            let _ = h.engine.sync_placeholder_texts(&mut h.session);
            let state = serde_json::to_value(host.engine.state(&host.session))
                .unwrap_or_else(|_| serde_json::json!({}));
            serde_json::json!({"ok": true, "state": state})
        }
        // 【按住Ctrl调频·对调 2026-11】候选窗模态调频的交换动作（Windows
        // DLL）：i/j=页内下标（与渲染同序），text_i/text_j=词文本（异步
        // 重排漂移校验：页内不符则按词全列表重找）。**原位重排**（纯位次
        // 口径：不 clear 不重查，raw/页/选全保——尾巴态不翻转），落盘走
        // adjust_set_order 整套 pN（两词同码一次、异码各一次），与右键
        // 菜单同管线。响应带刷新后的 state 供 DLL 即时重绘（+对调动效）。
        "cand_swap" => {
            let page_size = host.engine.config.candidates.page_size.max(1);
            let start = host.session.page * page_size;
            let get_page = |i: usize| {
                host.session
                    .candidates
                    .get(start + i)
                    .map(|c| c.text.as_str())
            };
            // 页内下标优先（词校验），漂移则按词页内重找→全列表兜底。
            let resolve = |want: usize, text: &str| -> Option<usize> {
                if let Some(t) = get_page(want) {
                    if text.is_empty() || t == text {
                        return Some(start + want);
                    }
                }
                if text.is_empty() {
                    return None;
                }
                (0..page_size)
                    .find(|&i| get_page(i) == Some(text))
                    .map(|i| start + i)
                    .or_else(|| {
                        host.session
                            .candidates
                            .iter()
                            .position(|c| c.text == text)
                    })
            };
            let i = req.get("i").and_then(|v| v.as_u64()).map(|v| v as usize);
            let j = req.get("j").and_then(|v| v.as_u64()).map(|v| v as usize);
            let ti = req.get("text_i").and_then(|v| v.as_str()).unwrap_or("");
            let tj = req.get("text_j").and_then(|v| v.as_str()).unwrap_or("");
            let (Some(pi), Some(pj)) = (i, j) else {
                return serde_json::json!({"ok": false, "why": "参数无效"});
            };
            // 页内渲染序对调才有意义：跨页/越界拒绝（模态只显示当前页）。
            if pi >= page_size || pj >= page_size || pi == pj {
                return serde_json::json!({"ok": false, "why": "下标无效"});
            }
            let (Some(a), Some(b)) = (resolve(pi, ti), resolve(pj, tj)) else {
                return serde_json::json!({"ok": false, "why": "候选不在当前页"});
            };
            let total = host.session.candidates.len();
            if a >= total || b >= total || a == b {
                return serde_json::json!({"ok": false, "why": "候选不在当前页"});
            }
            // 先取两词的码（原位交换会动列表），同码只整序一次。
            let code_a = host.session.candidates[a].code.clone();
            let code_b = host.session.candidates[b].code.clone();
            let h: &mut Host = &mut host;
            let _ = h.engine.op_swap_positions_abs(&mut h.session, a, b);
            if !code_a.is_empty() {
                let _ = h.engine.adjust_set_order(&h.session, &code_a);
            }
            if !code_b.is_empty() && code_b != code_a {
                let _ = h.engine.adjust_set_order(&h.session, &code_b);
            }
            // 【占位符变号·显示同步】落盘已改号，活会话文本同步（同右键
            // 菜单：窗口立即显示 ④，raw/页/选不动）。
            let _ = h.engine.sync_placeholder_texts(&mut h.session);
            let state = serde_json::to_value(host.engine.state(&host.session))
                .unwrap_or_else(|_| serde_json::json!({}));
            serde_json::json!({"ok": true, "state": state})
        }
        // 配置读取/写入（fcitx5 设置页用；Windows 前端不使用本 op）。
        // 读=全量 JSON；写=全量 JSON（客户端侧做「读-改-写」合并，避免
        // 部分字段缺省被 serde 默认值覆盖——与 Web 设置页同语义）。
        "config_get" => serde_json::json!({
            "config": serde_json::to_value(&host.engine.config)
                .unwrap_or(serde_json::Value::Null),
        }),
        "config_set" => {
            let v = req.get("config").cloned().unwrap_or(serde_json::Value::Null);
            match serde_json::from_value::<hufu_config::Config>(v) {
                Ok(cfg) => match host.apply_config(cfg) {
                    Ok((need_sentence, teardown)) => {
                        if need_sentence {
                            // 与 HTTP /api/config 同源（后台重建，不持锁载模型）
                            crate::reload_sentence_bg(teardown, true);
                        }
                        serde_json::json!({"ok": true})
                    }
                    Err(e) => serde_json::json!({"error": format!("应用失败: {e}")}),
                },
                Err(e) => serde_json::json!({"error": format!("配置无效: {e}")}),
            }
        }
        "state" => {
            // 先应用已到达的重排缓存：停顿期轮询（DLL poll_tick）拉 state
            // 时立即拿到换序后的新首选，用户无需按键即可看到候选窗刷新。
            {
                let h: &mut Host = &mut host;
                h.engine.refresh_rerank(&mut h.session);
            }
            let mut state = serde_json::to_value(host.engine.state(&host.session))
                .unwrap_or_else(|_| serde_json::json!({}));
            export_candidates(&mut host, req, &mut state);
            // 【皮肤版本】DLL poll 比对后强制重拉（连续调参即时生效）
            state["skin_ver"] = serde_json::json!(host.skin_ver);
            // 【方案变更兜底 2026-09-11】state 常带当前方案名：DLL poll
            // 比对后置皮肤失效——覆盖设置页/langbar 等非键路径切方案
            //（entrance_anim 等方案相关皮肤字段即时跟随）。
            state["current_schema"] = serde_json::json!(host.engine.config.schema.current);
            // 【实机预览锚点】有效期内携带：DLL 预览窗弹在设置窗中心
            if let Some(((x, y), until)) = host.preview_anchor {
                if until > std::time::Instant::now() {
                    state["preview_anchor"] = serde_json::json!({"x": x, "y": y});
                }
            }
            serde_json::json!({
                "state": state,
                "current_schema": host.engine.config.schema.current,
                "sentence_active": host.engine.sentence_active(),
            })
        }
        "reset" => {
            host.session = hufu_engine::Session::new(true);
            let state = host.engine.state(&host.session);
            serde_json::json!({"state": state})
        }
        "focus" => {
            // 焦点切换：v1 单会话，仅清空；换输入框 → 文章尾巴一并作废
            host.session.clear();
            host.session.tail_context.clear();
            let state = host.engine.state(&host.session);
            serde_json::json!({"state": state})
        }
        // 【UWP/开始菜单取图 2026-10-18】AppContainer 宿主（开始菜单搜索、
        // 设置、商店应用等）沙箱拒读数据目录——实测管道连通（act-*.txt
        // pipe=ok）而磁盘读图被拒 → 独立皮肤载图失败回退矢量皮。本 op
        // 供 DLL load_bitmap 读盘失败时经管道取图（b64，与 sound op 同
        // 形）。id/图名双重消毒（与 DLL normalize_image_name 同规），
        // 服务端只按自身数据根拼路径——不信任客户端路径。
        "ssf_image" => {
            let id = req.get("id").and_then(|x| x.as_str()).unwrap_or("");
            let image = req.get("image").and_then(|x| x.as_str()).unwrap_or("");
            let ok_name = |s: &str| {
                !s.is_empty()
                    && s.len() <= 128
                    && !s.contains("..")
                    && !s.chars().any(|c| {
                        matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
                            || c.is_control()
                    })
            };
            if !ok_name(id) || !ok_name(image) {
                return serde_json::json!({"error": "id/图名不合法"});
            }
            let p = crate::ssf::ssf_cache_root(&host.data_dir)
                .join(id)
                .join(image.to_lowercase());
            match std::fs::read(&p) {
                Ok(bytes) => match crate::b64::encode(&bytes) {
                    Ok(b) => serde_json::json!({"ok": true, "b64": b}),
                    Err(_) => serde_json::json!({"error": "b64 编码失败"}),
                },
                Err(e) => serde_json::json!({"error": format!("读图失败 {}: {e}", p.display())}),
            }
        }
        // 【渲染指定皮肤 2026-10-16】设置页实时框用：按 id 返回该 ssf
        // 皮肤的完整 payload（含其 overrides）——渲染「正在编辑的」皮
        // 肤而不是「当前上位」皮肤。仅供 dump 驱动进程调用；真实打字
        // 进程走 "skin"（op 面小、无副作用，不设白名单）。
        "skin_for_render" => {
            let id = req.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let resp = if !id.is_empty() {
                crate::ssf::skin_payload(&host.data_dir, &id)
                    .ok()
                    .map(|(mut sv, _)| {
                        // 实时框只画候选窗本体；挂件由设置页 UI 叠画
                        //（payload 顶层 overlay 剥掉，防双挂件）。
                        if let Some(o) = sv.as_object_mut() {
                            o.remove("overlay");
                        }
                        serde_json::json!({"skin": sv})
                    })
            } else {
                None
            };
            resp.unwrap_or_else(|| serde_json::json!({"error": "皮肤不可用"}))
        }
        "skin" => {
            let id = host.engine.config.appearance.skin.clone();
            // 【独立皮肤设置 2026-10-16】SSF 时序号开关可被 per-skin 覆盖
            //（载荷 layout.show_index 同步注入——DLL 双路可读）。
            let show_index = if host.engine.config.appearance.skin_kind == "ssf" {
                crate::ssf::SsfOverrides::load(&crate::ssf::ssf_cache_root(&host.data_dir).join(&id))
                    .show_index
                    .unwrap_or(host.engine.config.candidates.show_index)
            } else {
                host.engine.config.candidates.show_index
            };
            let delay_show_ms = host.engine.config.candidates.delay_show_ms;
            // 【SSF 皮肤 2026-12】kind=ssf → 位图皮肤：只传 cache_dir 路径
            // + 指纹（11MB 挂件教训：绝不内嵌大图字节），DLL 按需读盘。
            // anim 系列注入字段与 JSON 皮肤同形状（皮肤对象顶层）；
            // 颜色已转 "#RRGGBBAA"（DLL color_f 序）。解包失败 → 落到
            // 下方默认 JSON 皮路径（与 JSON 皮肤加载失败同款回退——
            // 坏皮绝不弄坏输入）。
            if host.engine.config.appearance.skin_kind == "ssf" {
                if let Ok((sv_src, src_fp)) = crate::ssf::skin_payload(&host.data_dir, &id) {
                    let anim = host.engine.config.appearance.anim;
                    let anim_speed = host.engine.config.appearance.anim_speed;
                    let anim_flash = host.engine.config.appearance.anim_flash;
                    // 指纹全覆盖（与 JSON 路径同口径）：源内容指纹 + 注入
                    // 字段全部参与——DLL have_fp 命中即 {"same":true} 微载荷。
                    let fp = format!(
                        "ssf|{id}|{src_fp}|{show_index}|{delay_show_ms}|{anim}|{anim_speed}|{anim_flash}|{}",
                        host.engine.config.schema.current
                    );
                    if let Some(h) = req.get("have_fp").and_then(|x| x.as_str()) {
                        if !h.is_empty() && h == fp {
                            return serde_json::json!({"same": true, "fp": fp});
                        }
                    }
                    let mut sv = sv_src;
                    if let Some(o) = sv.as_object_mut() {
                        o.insert("anim".into(), serde_json::json!(anim));
                        o.insert("anim_speed".into(), serde_json::json!(anim_speed));
                        o.insert("anim_flash".into(), serde_json::json!(anim_flash));
                        o.insert(
                            "first_show_slide".into(),
                            serde_json::json!(
                                !host.engine.config.schema.current.contains("整句")
                            ),
                        );
                    }
                    return serde_json::json!({
                        "skin": sv,
                        "show_index": show_index,
                        "delay_show_ms": delay_show_ms,
                        "fp": fp
                    });
                }
                eprintln!("ssf 皮肤 {id} 加载失败，候选窗回默认 JSON 皮肤");
            }
            let p = host.skins_dir().join(format!("{id}.json"));
            // 【皮肤指纹 2026-11】挂件大图皮肤 JSON 可达 11MB——DLL 每
            // 2.5s 例行重拉在打字线程全量解析（40-80ms 停顿，QQ 抖动
            // 观感的放大器，违「首键要快」）。响应内容=皮肤文件+注入
            // 字段，指纹全覆盖：id/文件 mtime/长度 + show_index/
            // delay_show_ms/anim/anim_speed/anim_flash/方案名（first_
            // show_slide 注入源）。DLL 带 have_fp 命中 → {"same":true}
            // 微载荷，跳过解析与 note_skin 全串重哈希。保存/重置/切换/
            // 手改文件/改配置全部改变指纹，无一漏网。
            let meta = std::fs::metadata(&p).ok();
            let fp = format!(
                "{id}|{:?}|{}|{show_index}|{delay_show_ms}|{}|{}|{}|{}",
                meta.as_ref().and_then(|m| m.modified().ok()),
                meta.as_ref().map(|m| m.len()).unwrap_or(0),
                host.engine.config.appearance.anim,
                host.engine.config.appearance.anim_speed,
                host.engine.config.appearance.anim_flash,
                host.engine.config.schema.current,
            );
            if let Some(h) = req.get("have_fp").and_then(|x| x.as_str()) {
                if !h.is_empty() && h == fp {
                    return serde_json::json!({"same": true, "fp": fp});
                }
            }
            // 【动效全局开关+速度 2026-09-11】注入皮肤对象顶层（DLL 读
            // /skin/anim 或顶层 anim——两形态都认）；设置页·皮肤页控件
            let anim = host.engine.config.appearance.anim;
            let anim_speed = host.engine.config.appearance.anim_speed;
            // 【选重暂留开关 2026-11】独立于总开关：数字选重上屏的确认
            // 暂留（高亮滑向选中项+~0.15s 收场）。DLL 闪帧门同源读法
            //（/skin/anim_flash 或顶层，缺省 true）。
            let anim_flash = host.engine.config.appearance.anim_flash;
            // 【二十四修·动效大瘦身 2026-10-09】入场/退场/上屏停留全线
            // 退役（用户拍板只留平移/尺寸/高亮滑动）——DLL 侧不再消费
            // entrance_anim/commit_hold，注入一并移除。
            match hufu_skin::Skin::load(&p) {
                Ok(s) => {
                    let mut sv = serde_json::to_value(s).unwrap_or_else(|_| serde_json::json!({}));
                    if let Some(o) = sv.as_object_mut() {
                        // 【特效退役 2026-09-22】commit_fx / stamp_font_scale
                        // 注入移除（DLL 消费端已删）。
                        o.insert("anim".into(), serde_json::json!(anim));
                        o.insert("anim_speed".into(), serde_json::json!(anim_speed));
                        o.insert("anim_flash".into(), serde_json::json!(anim_flash));
                        // 【虎娘对齐·首显滑动 2026-09-18 二修】显式 opt-in
                        // 门控：非整句方案才允许组段首显滑动。不用
                        // sentence_active()——无模型包里 sentence.is_none()
                        // 恒 false，门控形同虚设（实测整句也滑了）；方案名
                        // 判定与 engine 的 auto_enable 同源。DLL 侧缺键=
                        // 不滑（旧缓存皮肤安全退化瞬显）。
                        o.insert(
                            "first_show_slide".into(),
                            serde_json::json!(!host.engine.config.schema.current.contains("整句")),
                        );
                    }
                    serde_json::json!({"skin": sv, "show_index": show_index, "delay_show_ms": delay_show_ms, "fp": fp})
                }
                Err(e) => {
                    eprintln!("皮肤 {id} 加载失败，候选窗回默认: {e}");
                    let mut sv = serde_json::to_value(hufu_skin::Skin::default())
                        .unwrap_or_else(|_| serde_json::json!({}));
                    if let Some(o) = sv.as_object_mut() {
                        o.insert("anim".into(), serde_json::json!(anim));
                        o.insert("anim_speed".into(), serde_json::json!(anim_speed));
                        o.insert("anim_flash".into(), serde_json::json!(anim_flash));
                        o.insert(
                            "first_show_slide".into(),
                            serde_json::json!(!host.engine.config.schema.current.contains("整句")),
                        );
                    }
                    serde_json::json!({
                        "skin": sv,
                        "show_index": show_index,
                        "delay_show_ms": delay_show_ms,
                        "fp": fp
                    })
                }
            }
        }
        // 【滚轮缩放候选框】DLL 候选框 WM_MOUSEWHEEL 调用：当前皮肤
        // layout.font_point ±delta（clamp 10~36），写回皮肤文件持久化；
        // 返回新字号供 DLL 立即重绘。
        // 【2026-09-06 序号跟随】label_font_point（候选序号字级）按同比例
        // 缩放——用户实测滚轮放大时候选序号原大小不动。比例=序号/主字，
        // 放大缩小都保持视觉层级；clamp 4~40。
        // 【语义修正】label_font_point=0 是「0.78 倍正文自动跟随」
        // （candwin2 tf_label 回退 tf_small）——保持 0 不动，主字缩放
        // 时天然跟随（此前 0/主字=0 被 clamp 成 4pt 钉死）。
        // 【比例联动 2026-09-08】几何参数（内边距/间距/圆角/边框/阴影
        // 大小/最小宽）同比例放大——只放字不放垫「放大不好看」（用户
        // 实测）。逻辑在 hufu_skin::Layout::scale_geometry（与设置页
        // 改字号共用同一语义）。
        "skin_font_delta" => {
            let delta = req.get("delta").and_then(|x| x.as_i64()).unwrap_or(1) as i32;
            let id = host.engine.config.appearance.skin.clone();
            // 【SSF 滚轮字号 2026-10-16】独立皮肤无 .json 皮肤文件——旧
            // 路径 load 失败直接吞掉滚轮。SSF 分支：per-skin overrides
            // 的 font_pt ±1（基准=现值或 16），写回+指纹推进（skin_ver
            // +载荷 fp 含 overrides → DLL 下次重拉拿到新字号）。
            if host.engine.config.appearance.skin_kind == "ssf" {
                let cache_dir = crate::ssf::ssf_cache_root(&host.data_dir).join(&id);
                let mut ov = crate::ssf::SsfOverrides::load(&cache_dir);
                let np = ((ov.font_pt.unwrap_or(16.0) as i32) + delta).clamp(10, 36);
                ov.font_pt = Some(np as f32);
                return match ov.save(&cache_dir) {
                    Ok(()) => {
                        host.skin_ver += 1;
                        serde_json::json!({"font_point": np})
                    }
                    Err(e) => {
                        eprintln!("ssf 皮肤 {id} 字号保存失败: {e}");
                        serde_json::json!({"err": e.to_string()})
                    }
                };
            }
            let p = host.skins_dir().join(format!("{id}.json"));
            match hufu_skin::Skin::load(&p) {
                Ok(mut s) => {
                    let op = s.layout.font_point;
                    let np = (op as i32 + delta).clamp(10, 36);
                    if op > 0.0 {
                        let ratio = np as f32 / op;
                        if s.layout.label_font_point > 0.0 {
                            let r2 = s.layout.label_font_point / op;
                            s.layout.label_font_point = (np as f32 * r2).clamp(4.0, 40.0);
                        }
                        s.layout.scale_geometry(ratio);
                        // 【玻璃留白字号联动 2026-09-10】玻璃独立留白同
                        // 比例联动（此前缺位——毛玻璃放大时留白不变，
                        // 用户实测「内边距没有跟着变」）。
                        s.material.scale_glass_geometry(ratio);
                    }
                    s.layout.font_point = np as f32;
                    let nl = s.layout.label_font_point;
                    match s.save(&p) {
                        Ok(()) => {
                            host.skin_ver += 1;
                            serde_json::json!({"font_point": np, "label_font_point": nl})
                        }
                        Err(e) => {
                            eprintln!("皮肤 {id} 字号保存失败: {e}");
                            serde_json::json!({"err": e.to_string()})
                        }
                    }
                }
                Err(e) => {
                    eprintln!("皮肤 {id} 加载失败: {e}");
                    serde_json::json!({"err": e.to_string()})
                }
            }
        }
        // 输入法激活态上报（DLL Activate/Deactivate）：驱动托盘图标显隐
        "ime" => {
            let active = req.get("active").and_then(|v| v.as_bool()).unwrap_or(false);
            crate::platform::on_ime_state(active);
            serde_json::json!({"ok": true})
        }
        // 语言栏「中」按钮点击：开设置页（与托盘双击/Ctrl+Alt+H 同通道）
        "settings" => {
            crate::platform::open_settings();
            serde_json::json!({"ok": true})
        }
        // 语言栏「中/英」左键切换中英（语言指示牌语义：切换即放弃
        // 当前编码残留——无条件清空再切，防全局会话 raw 残留卡死切换）
        "toggle_lang" => {
            host.session.clear();
            host.session.chinese = !host.session.chinese;
            // 【切英文态清锁大写 2026-10-12】语言栏切换同口径复位。
            host.session.caps_lock = false;
            host.session.pair.reset();
            let state = host.engine.state(&host.session);
            serde_json::json!({"state": state})
        }
        // 【占位符记忆 2026-10-09】加词框「占位符」勾选态读写。沙箱宿主
        //（NTQQ 渲染进程）写不了 HKCU——记忆改由 server 落
        // 数据\addword-ph.json，DLL 对话框构建回读/勾选即写都走管道。
        "aw_ph_get" => serde_json::json!({"on": host.addword_ph}),
        "aw_ph_set" => {
            let on = req.get("on").and_then(|v| v.as_bool()).unwrap_or(false);
            host.addword_ph = on;
            let _ = std::fs::write(
                host.data_dir.join("addword-ph.json"),
                serde_json::json!({"on": on}).to_string(),
            );
            serde_json::json!({"ok": true, "on": on})
        }
        // compartment 对账用【设值】而非切换：全局 compartment 变化
        // 时多个后台进程会各自收到 OnChange——若各自 toggle 会把共享
        // 引擎连番翻转（实测：牌显英/打中文的奇偶错乱）。幂等设值让
        // 所有进程收敛到 compartment 指示的同一状态。
        "set_lang" => {
            let zh = req
                .get("chinese")
                .and_then(|v| v.as_bool())
                .unwrap_or(host.session.chinese);
            host.session.clear();
            host.session.chinese = zh;
            // 【锁大写复位 2026-10-12】compartment 幂等设值时锁大写一并
            // 复位（跨进程对账口径：语言态是全局单值，锁大写不入牌面）。
            host.session.caps_lock = false;
            host.session.pair.reset();
            let state = host.engine.state(&host.session);
            serde_json::json!({"state": state})
        }
        // 【旧版控制台真光标 2026-10-15】DLL 载在 conhost 进程内（它是
        // 服务端不是客户端——GetStdHandle/CONOUT$ 实测皆拒，MSAA 的
        // OBJID_CARET 也不实现 accLocation）。借本进程 AttachConsole
        // 到该控制台的**客户端**进程（= conhost 的父进程，Toolhelp 查）
        // 当客户端读光标格位+字体格度，换算屏幕像素 rect 返回。DLL 拿
        // rect 作候选锚——旧版模式候选贴真光标（X/Y 全跟）。Attach 是
        // 进程级状态：全局串行 + 用毕 FreeConsole 还原。
        "console_caret" => {
            let pid = req.get("pid").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            match console_caret_rect_via_attach(pid) {
                Some([l, t, r, b]) => serde_json::json!({ "rect": [l, t, r, b] }),
                None => {
                    let err = console_caret_last_err();
                    serde_json::json!({ "rect": null, "err": err })
                }
            }
        }
        // 语言栏「中/英」右键菜单数据：码表清单 + 当前方案。
        // 【死锁教训】dispatch 已持 host 锁——绝不能经 tray::
        // schema_snapshot 二次锁同一把 Mutex（曾死锁管道线程拖死全机
        // 打字）；这里按 HTTP GET /api/schemas 同源逻辑直算。
        "schemas" => {
            let dir = hufu_engine::Engine::resolve_data_sub(
                &host.data_dir,
                &host.engine.config.schema.dir,
            );
            let mut names: Vec<String> = std::fs::read_dir(&dir)
                .map(|rd| {
                    rd.flatten()
                        .filter(|e| e.path().is_dir())
                        .filter_map(|e| e.file_name().into_string().ok())
                        .collect()
                })
                .unwrap_or_default();
            names.sort();
            let current = host.engine.config.schema.current.clone();
            serde_json::json!({"schemas": names, "current": current})
        }
        // 语言栏菜单选码表（与 POST /api/schema 同逻辑：换方案 + 清
        // 会话 + 重建整句 + 落盘——同样在已持锁内直做）
        "set_schema" => {
            let name = req.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if !name.is_empty() && host.engine.switch_schema(name).is_ok() {
                host.session.clear();
                // 【卡死修复】后台重建整句（不持锁载 546MB 模型）
                crate::reload_sentence_bg(true, false);
                let _ = host.engine.config.save(&host.config_path);
            }
            let dir = hufu_engine::Engine::resolve_data_sub(
                &host.data_dir,
                &host.engine.config.schema.dir,
            );
            let mut names: Vec<String> = std::fs::read_dir(&dir)
                .map(|rd| {
                    rd.flatten()
                        .filter(|e| e.path().is_dir())
                        .filter_map(|e| e.file_name().into_string().ok())
                        .collect()
                })
                .unwrap_or_default();
            names.sort();
            let current = host.engine.config.schema.current.clone();
            serde_json::json!({"schemas": names, "current": current})
        }
        // 【2026-09-05·2026-11 演进】原语言栏右键「重载码表」入口已删
        //（Windows 改码表监视线程自动重载，见 main.rs）；本 op 保留：
        // Linux fcitx5 托盘「重载码表」与紧急手动通道。当前方案原样
        // 重载，与 set_schema 同源逻辑，name=当前方案名。
        "reload_schema" => {
            let name = host.engine.config.schema.current.clone();
            let ok = host.engine.switch_schema(&name).is_ok();
            if ok {
                host.session.clear();
                // 【卡死修复】后台重建整句（重读补充语料语义）
                crate::reload_sentence_bg(true, true);
            }
            serde_json::json!({"ok": ok, "current": name})
        }
        // 语言栏右键「打开方案文件夹」：explorer 打开当前方案码表目录
        "open_schema_dir" => {
            let name = host.engine.config.schema.current.clone();
            let dir = hufu_engine::Engine::resolve_data_sub(
                &host.data_dir,
                &host.engine.config.schema.dir,
            )
            .join(&name);
            if dir.is_dir() {
                crate::platform::open_path(&dir);
            }
            serde_json::json!({"ok": dir.is_dir(), "path": dir})
        }
        // 语言栏右键「导出码表」：导出当前方案（用户调整合并快照）并
        // explorer 打开导出子文件夹（码表导出\<方案名>\）。
        // 【导出格式记忆 2026-11】取当前方案的记忆格式（settings 导出
        // 下拉选过即记忆；无记忆 = Sentence 兼容旧行为）。
        "export_schema" => {
            let cur = host.engine.config.schema.current.clone();
            let fmt = host
                .engine
                .config
                .schema
                .export_formats
                .get(&cur)
                .cloned()
                .unwrap_or_default();
            match host.export_schema(None, &fmt) {
                Ok((path, n)) => {
                    let dir = std::path::Path::new(&path)
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_default();
                    if dir.is_dir() {
                        crate::platform::open_path(&dir);
                    }
                    serde_json::json!({"ok": true, "path": path, "lines": n})
                }
                Err(e) => serde_json::json!({"ok": false, "error": e}),
            }
        },
        // 语言栏菜单音效开关：读态 / 切换（落盘，热生效）
        "sound_state" => serde_json::json!({
            "enabled": host.engine.config.sound.enabled,
            "volume": host.engine.config.sound.volume,
        }),
        "sound_toggle" => {
            host.engine.config.sound.enabled = !host.engine.config.sound.enabled;
            let enabled = host.engine.config.sound.enabled;
            let _ = host.engine.config.save(&host.config_path);
            serde_json::json!({"enabled": enabled})
        }
        // 越进程候选窗（沉浸式宿主如开始菜单搜索：DLL 自绘窗被 DWM
        // cloaked、UIElement 被宿主拒绝 → server 代画【用户皮肤】）
        "cand" => {
            let items: Vec<(String, String)> = req
                .get("items")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .map(|c| {
                            (
                                c.get("text")
                                    .and_then(|x| x.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                                c.get("comment")
                                    .and_then(|x| x.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            let raw = req
                .get("raw")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let sel = req.get("selected").and_then(|x| x.as_u64()).unwrap_or(0) as usize;
            let x = req.get("x").and_then(|x| x.as_i64()).unwrap_or(100) as i32;
            let y = req.get("y").and_then(|x| x.as_i64()).unwrap_or(100) as i32;
            // 【皮肤缓存 2026-09-11】DLL 侧仅换肤/首推/重推时携带
            // "skin"（逐帧全量推送是 KB 级管道浪费——对齐 tsf.rs 的
            // srv_skin_ver_pushed 去重）；缺省沿用上帧皮肤。
            // 【大皮肤改造 2026-10-03】挂件动图入皮肤后 skin 可达数 MB，
            // 携带完整 skin 的 cand 请求会撞下方 1MB 请求闸（连接直接
            // 断，server 代画链失效=沉浸宿主回默认皮）——DLL 改推
            // skin_ver（8 字节），server 端按版本自取当前皮肤文件；
            // 完整 skin 字段保留兼容（小皮肤/旧 DLL 仍可用）。
            static LAST_SKIN: std::sync::Mutex<Option<(u64, serde_json::Value)>> =
                std::sync::Mutex::new(None);
            let skin = if let Some(v) = req.get("skin") {
                if !v.is_null() {
                    let v = v.clone();
                    if let Ok(mut c) = LAST_SKIN.lock() {
                        *c = Some((u64::MAX, v.clone()));
                    }
                    v
                } else {
                    // null=「按版本重取」：ver 与缓存一致用缓存，否则读盘
                    let want_ver = req.get("skin_ver").and_then(|x| x.as_u64()).unwrap_or(0);
                    let cached = LAST_SKIN
                        .lock()
                        .ok()
                        .and_then(|c| c.clone())
                        .filter(|(v, _)| *v == want_ver && want_ver != 0);
                    match cached {
                        Some((_, v)) => v,
                        None => {
                            let id = host.engine.config.appearance.skin.clone();
                            let p = host.skins_dir().join(format!("{id}.json"));
                            let v = hufu_skin::Skin::load(&p)
                                .map(|s| serde_json::to_value(s).unwrap_or_default())
                                .unwrap_or_default();
                            if let Ok(mut c) = LAST_SKIN.lock() {
                                *c = Some((want_ver, v.clone()));
                            }
                            v
                        }
                    }
                }
            } else {
                LAST_SKIN
                    .lock()
                    .ok()
                    .and_then(|c| c.clone())
                    .map(|(_, v)| v)
                    .unwrap_or(serde_json::Value::Null)
            };

            // 【二十九修·首显滑动】组段首帧带滑动起点（None=常规帧）
            // 【Linux 适配】候选窗代画收口到 platform::cand_show：Windows
            // 转发 candwin（保留上述首显滑动语义），Linux 由前端自绘（no-op）。
            crate::platform::cand_show(items, raw, sel, x, y, skin);
          
            serde_json::json!({"ok": true})
        }
        "cand_hide" => {
            crate::platform::cand_hide();
            serde_json::json!({"ok": true})
        }
        "sound" => {
            // tag → {data: base64 wav, volume}（文件缺失返回 404 语义 null）
            let tag = req.get("tag").and_then(|t| t.as_str()).unwrap_or("");
            let safe = ["key", "select", "commit", "page"];
            if !safe.contains(&tag) {
                return serde_json::json!({"error": "未知音效"});
            }
            let vol = host.engine.config.sound.volume;
            let p = host.data_dir.join("音效").join(format!("{tag}.wav"));
            match std::fs::read(&p) {
                Ok(bytes) => serde_json::json!({
                    "data": base64_encode(&bytes),
                    "volume": vol,
                }),
                Err(_) => serde_json::json!({"data": null, "volume": vol}),
            }
        }
        "clipboard" => {
            // {exe} → {text}：白名单校验 + 读剪贴板（Ctrl+Shift+V 剪贴板上屏）
            let cfg = host.engine.config.clipboard.clone();
            if !cfg.enabled {
                return serde_json::json!({"text": null, "reason": "disabled"});
            }
            // 【白名单反查 2026-09-11】旧实现信客户端自报 exe（任意进程
            // 谎报即过白名单）。优先用服务端反查的管道对端映像名。
            let claimed = req.get("exe").and_then(|t| t.as_str()).unwrap_or("");
            let exe_ref: &str = client_exe.unwrap_or(claimed);
            let exe = exe_ref.rsplit(['\\', '/']).next().unwrap_or(exe_ref);
            if !cfg.allows(exe) {
                return serde_json::json!({"text": null, "reason": "whitelist"});
            }
            #[cfg(windows)]
            let text = crate::clipboard::read_text();
            #[cfg(not(windows))]
            let text = String::new();
            serde_json::json!({"text": text})
        }
        op => serde_json::json!({"error": format!("未知操作: {op}")}),
    }
}

/// 标准 base64 编码（服务端自足实现，免新增依赖）。
fn base64_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(windows)]
mod imp {
    use super::*;

    // windows-sys 原型（保持零特性门）
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateNamedPipeW(
            name: *const u16,
            open_mode: u32,
            pipe_mode: u32,
            instances: u32,
            out_buf: u32,
            in_buf: u32,
            timeout: u32,
            sa: *const core::ffi::c_void,
        ) -> isize;
        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl: *const u16,
            revision: u32,
            sd: *mut *mut core::ffi::c_void,
            returned: *mut u32,
        ) -> i32;
        fn ConnectNamedPipe(pipe: isize, overlapped: *mut core::ffi::c_void) -> i32;
        fn PeekNamedPipe(
            h: isize,
            buf: *mut core::ffi::c_void,
            buf_size: u32,
            read: *mut u32,
            avail: *mut u32,
            left: *mut u32,
        ) -> i32;
        fn DisconnectNamedPipe(pipe: isize) -> i32;
        fn ReadFile(
            h: isize,
            buf: *mut u8,
            len: u32,
            read: *mut u32,
            overlapped: *mut core::ffi::c_void,
        ) -> i32;
        fn WriteFile(
            h: isize,
            buf: *const u8,
            len: u32,
            written: *mut u32,
            overlapped: *mut core::ffi::c_void,
        ) -> i32;
        fn CloseHandle(h: isize) -> i32;
    }

    const PIPE_ACCESS_DUPLEX: u32 = 0x0000_0003;
    const PIPE_TYPE_BYTE: u32 = 0x0000_0000;
    const PIPE_READMODE_BYTE: u32 = 0x0000_0000;
    const PIPE_WAIT: u32 = 0x0000_0000;
    const PIPE_UNLIMITED_INSTANCES: u32 = 255;
    const INVALID_HANDLE_VALUE: isize = -1;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetNamedPipeClientProcessId(pipe: isize, pid: *mut u32) -> i32;
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
        fn QueryFullProcessImageNameW(
            proc: isize,
            flags: u32,
            name: *mut u16,
            len: *mut u32,
        ) -> i32;
    }
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

    /// 管道对端进程映像名（服务端反查，客户端无法伪造）。失败返回 None。
    fn client_process_exe(h: isize) -> Option<String> {
        unsafe {
            let mut pid: u32 = 0;
            if GetNamedPipeClientProcessId(h, &mut pid) == 0 {
                return None;
            }
            let proc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if proc == 0 {
                return None;
            }
            let mut buf = [0u16; 512];
            let mut len: u32 = 512;
            let ok = QueryFullProcessImageNameW(proc, 0, buf.as_mut_ptr(), &mut len);
            CloseHandle(proc);
            if ok == 0 {
                return None;
            }
            let s = String::from_utf16_lossy(&buf[..len as usize]);
            let base = s.rsplit(['\\', '/']).next().unwrap_or(&s).to_string();
            Some(base)
        }
    }

    /// 【三十六修·对等防护】PeekNamedPipe 等数据到达：deadline 内有数
    /// 据=继续，管道断/超时=false。客户端侧（ipc.rs call_on）早有同款
    /// 轮询体系，服务端此前纯阻塞 ReadFile——建连后不发帧的僵尸连接
    /// （本机进程开 255 个静默连接占满 PIPE_UNLIMITED_INSTANCES）会让
    /// 监听线程 CreateNamedPipe 永败、合法 DLL 连不进。
    fn wait_avail(h: isize, deadline: Option<std::time::Instant>) -> bool {
        loop {
            let mut avail: u32 = 0;
            let ok = unsafe {
                PeekNamedPipe(
                    h,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    &mut avail,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                return false;
            }
            if avail > 0 {
                return true;
            }
            match deadline {
                Some(d) if std::time::Instant::now() >= d => return false,
                _ => {}
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    /// 读 n 字节。deadline=Some 时，数据未到位的等待受其约束（数据一
    /// 旦可见，阻塞 ReadFile 立即返回可用字节，不再受 deadline 影响）。
    fn read_exact_dl(
        h: isize,
        n: usize,
        deadline: Option<std::time::Instant>,
    ) -> std::io::Result<Vec<u8>> {
        let mut buf = vec![0u8; n];
        let mut done = 0usize;
        while done < n {
            if !wait_avail(h, deadline) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "等待管道数据超时/断开",
                ));
            }
            let mut got = 0u32;
            let ok = unsafe {
                ReadFile(
                    h,
                    buf.as_mut_ptr().add(done),
                    (n - done) as u32,
                    &mut got,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 || got == 0 {
                return Err(std::io::Error::new(ErrorKind::UnexpectedEof, "管道关闭"));
            }
            done += got as usize;
        }
        Ok(buf)
    }

    fn write_all(h: isize, buf: &[u8]) -> std::io::Result<()> {
        let mut done = 0usize;
        while done < buf.len() {
            let mut wrote = 0u32;
            let ok = unsafe {
                WriteFile(
                    h,
                    buf.as_ptr().add(done),
                    (buf.len() - done) as u32,
                    &mut wrote,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(std::io::Error::new(ErrorKind::BrokenPipe, "管道写入失败"));
            }
            done += wrote as usize;
        }
        Ok(())
    }

    /// 单连接处理：读帧 → 派发 → 写帧，直至断开。
    /// 【三十六修·读超时】首帧 10s / 帧头→帧体 2s：合法客户端按需建连、
    /// 建连即发帧且 head+body 一次写出——两个窗口只拦「建连不发帧」
    /// 与「只发帧头的半包」两类僵尸连接。既有通信史后的空闲等待不受限
    ///（keep-alive 常驻语义不变，deadline=None 阻塞如旧）。
    fn serve_conn(h: isize, host: &Mutex<Host>) {
        // 对端进程名：本连接生命周期内不变，一次反查全程使用
        let client_exe = client_process_exe(h);
        let connect_at = std::time::Instant::now();
        let mut first_frame = true;
        loop {
            let head = {
                let dl = if first_frame {
                    Some(connect_at + std::time::Duration::from_secs(10))
                } else {
                    None
                };
                match read_exact_dl(h, 4, dl) {
                    Ok(b) => b,
                    Err(_) => break,
                }
            };
            first_frame = false;
            let len = u32::from_le_bytes([head[0], head[1], head[2], head[3]]) as usize;
            if len == 0 || len > BUF {
                break;
            }
            let body = match read_exact_dl(
                h,
                len,
                Some(std::time::Instant::now() + std::time::Duration::from_secs(2)),
            ) {
                Ok(b) => b,
                Err(_) => break,
            };
            let req: serde_json::Value = match serde_json::from_slice(&body) {
                Ok(v) => v,
                // 【错误信封 2026-09-11】坏 JSON 不再当请求喂 dispatch 靠
                // op="" 巧合落错——直接回错误，连接保活
                Err(e) => {
                    let resp = serde_json::json!({"error": format!("无效 JSON: {e}")});
                    let out = serde_json::to_vec(&resp).unwrap_or_default();
                    if out.len() <= BUF {
                        let mut frame = (out.len() as u32).to_le_bytes().to_vec();
                        frame.extend_from_slice(&out);
                        if write_all(h, &frame).is_err() {
                            break;
                        }
                    }
                    continue;
                }
            };
            // 【panic 防护 2026-09-11】dispatch 内 panic（坏模型数据等）
            // 旧实现直接穿越 → 本 serve_conn 的 Disconnect/Close 被跳过
            //（管道实例泄漏，上限 255）。兜住：按错误响应写出，连接
            // 生命周期照常走完。主机线程不再被单帧毒死。
            let resp = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                dispatch(host, &req, client_exe.as_deref())
            })) {
                Ok(v) => v,
                Err(_) => serde_json::json!({"error": "服务内部错误（已恢复）"}),
            };
            let mut out = serde_json::to_vec(&resp).unwrap_or_else(|_| {
                // 【Zero-length frame fix 2026-09-11】Old implementation wrote a len=0 frame on serialization failure
                //(client interprets as disconnect) — write minimal error JSON instead
                b"{\"error\":\"resp serialize failed\"}".to_vec()
            });
            if out.len() > RESP_BUF {
                out = serde_json::json!({"error": "响应过大"})
                    .to_string()
                    .into_bytes();
            }
            let mut frame = (out.len() as u32).to_le_bytes().to_vec();
            frame.extend_from_slice(&out);
            if write_all(h, &frame).is_err() {
                break;
            }
        }
        unsafe {
            DisconnectNamedPipe(h);
            CloseHandle(h);
        }
    }

    /// 阻塞运行管道服务（每实例一线程）。
    pub fn run(host: std::sync::Arc<Mutex<Host>>) -> std::io::Result<()> {
        let name = wide(PIPE_NAME);
        // 管道 DACL（SDDL）：显式授 Everyone + ALL APPLICATION PACKAGES
        // 读写——默认 DACL（仅创建者/管理员）会拒绝 AppContainer 宿主
        //（开始菜单搜索 SearchHost 等 SystemApps）→ 搜索框里虎符取词
        // 失败、字母直通（2026-08-29 实测病灶之一）。
        #[repr(C)]
        #[allow(non_snake_case)] // Win32 原名（FFI 手写声明）
        struct SecurityAttributes {
            nLength: u32,
            lp_security_descriptor: *mut core::ffi::c_void,
            inherit_handle: i32,
        }
        let sddl: Vec<u16> = "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GWGR;;;WD)(A;;GWGR;;;S-1-15-2-1)(A;;GWGR;;;S-1-15-2-2)\0"
            .encode_utf16()
            .collect();
        let mut sd: *mut core::ffi::c_void = std::ptr::null_mut();
        let sd_ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1, // SDDL_REVISION_1
                &mut sd,
                std::ptr::null_mut(),
            )
        } != 0;
        // 诊断落盘：SDDL 是否成功转换（AppContainer 管道连通排查）
        let _ = std::fs::create_dir_all(r"C:\ProgramData\HuFu\diag");
        let _ = std::fs::write(
            r"C:\ProgramData\HuFu\diag\pipe-sddl.txt",
            format!("sd_ok={sd_ok} err={:?}\n", std::io::Error::last_os_error()),
        );
        let sa = SecurityAttributes {
            nLength: std::mem::size_of::<SecurityAttributes>() as u32,
            lp_security_descriptor: sd,
            inherit_handle: 0,
        };
        let sa_ptr: *const core::ffi::c_void = if sd_ok {
            &sa as *const SecurityAttributes as *const core::ffi::c_void
        } else {
            std::ptr::null()
        };
        loop {
            let h = unsafe {
                CreateNamedPipeW(
                    name.as_ptr(),
                    PIPE_ACCESS_DUPLEX,
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                    PIPE_UNLIMITED_INSTANCES,
                    64 * 1024,
                    64 * 1024,
                    0,
                    sa_ptr,
                )
            };
            if h == INVALID_HANDLE_VALUE {
                // 【生命线重试 2026-09-11】旧实现直接 return Err → 监听
                // 线程永久退出，进程活着但输入法瘫痪（与下方「绝不退出」
                // 注释自相矛盾）。退避重试（100ms 起、上限 5s），除非进程
                // 正在退出。
                let err = std::io::Error::last_os_error();
                eprintln!("管道创建失败: {err}，退避重试");
                std::thread::sleep(std::time::Duration::from_millis(100));
                continue;
            }
            if unsafe { ConnectNamedPipe(h, std::ptr::null_mut()) } == 0 {
                let err = std::io::Error::last_os_error();
                // ERROR_NO_DATA=232：客户端已断开，继续接受下一连接
                if err.raw_os_error() == Some(232) {
                    unsafe { CloseHandle(h) };
                    continue;
                }
                // ERROR_PIPE_CONNECTED=535：客户端在 Create 与 Connect 之间已连上（竞态），
                // 视为已连接，正常服务 —— 之前当致命错误退出，会把监听线程带崩。
                if err.raw_os_error() == Some(535) {
                    let host = host.clone();
                    std::thread::spawn(move || serve_conn(h, &host));
                    continue;
                }
                // 其他错误：日志 + 短歇再战，绝不退出（管道是输入法生命线，
                // 任何单次异常都不能杀掉监听）
                eprintln!("管道连接异常: {err}，50ms 后继续");
                unsafe { CloseHandle(h) };
                std::thread::sleep(std::time::Duration::from_millis(50));
                continue;
            }
            let host = host.clone();
            std::thread::spawn(move || serve_conn(h, &host));
        }
    }
}

#[cfg(windows)]
pub use imp::run as run_pipe;

#[cfg(not(windows))]
mod unix_imp {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;

    fn sock_path() -> std::path::PathBuf {
        std::env::var("XDG_RUNTIME_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"))
            .join("hufu-ime.sock")
    }

    fn serve_conn(mut stream: std::os::unix::net::UnixStream, host: &Mutex<Host>) {
        loop {
            let mut head = [0u8; 4];
            if stream.read_exact(&mut head).is_err() {
                break;
            }
            let len = u32::from_le_bytes(head) as usize;
            if len == 0 || len > BUF {
                break;
            }
            let mut body = vec![0u8; len];
            if stream.read_exact(&mut body).is_err() {
                break;
            }
            let req: serde_json::Value = match serde_json::from_slice(&body) {
                Ok(v) => v,
                Err(e) => serde_json::json!({"error": e.to_string()}),
            };
            let resp = dispatch(host, &req, None);
            let out = serde_json::to_vec(&resp).unwrap_or_default();
            let mut frame = (out.len() as u32).to_le_bytes().to_vec();
            frame.extend_from_slice(&out);
            if stream.write_all(&frame).is_err() {
                break;
            }
        }
    }

    pub fn run(host: std::sync::Arc<Mutex<Host>>) -> std::io::Result<()> {
        let path = sock_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path)?;
        // 仅本人可读写（与 Windows 命名管道 ACL 对齐）
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        eprintln!("HuFu unix socket: {}", path.display());
        for stream in listener.incoming() {
            match stream {
                Ok(s) => {
                    let host = host.clone();
                    std::thread::spawn(move || serve_conn(s, &host));
                }
                Err(_) => continue,
            }
        }
        Ok(())
    }
}

#[cfg(not(windows))]
pub use unix_imp::run as run_pipe;

/// 【旧版控制台真光标 2026-10-15】conhost_pid = DLL（在 conhost 内）自报
/// 进程号。conhost 的父进程 = 该控制台的客户端（cmd 等）→ AttachConsole
/// 挂上 → GetConsoleScreenBufferInfo 光标格 + GetCurrentConsoleFont 格度
/// + GetConsoleWindow/ClientToScreen 换算屏幕像素。用毕 FreeConsole。
/// 进程级全局串行（Attach 是进程状态）。失败任一步 → None（DLL 落标准
/// 链，不钉位）。客户端父进程可能已退（conhost 收尾期）→ Toolhelp 查
/// 不到 → None。
#[cfg(windows)]
static LAST_CARET_ERR: Mutex<Option<&'static str>> = Mutex::new(None);

#[cfg(windows)]
fn console_caret_rect_via_attach(conhost_pid: u32) -> Option<[i32; 4]> {
    use windows::Win32::Foundation::{POINT, RECT};
    use windows::Win32::System::Console::{
        AttachConsole, FreeConsole, GetConsoleScreenBufferInfo, GetConsoleWindow,
        GetCurrentConsoleFont, GetStdHandle, CONSOLE_FONT_INFO, CONSOLE_SCREEN_BUFFER_INFO,
        STD_OUTPUT_HANDLE,
    };
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
    use windows::Win32::Graphics::Gdi::ClientToScreen;

    static ATTACH_LOCK: Mutex<()> = Mutex::new(());
    let _g = ATTACH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let note = |s: &'static str| -> Option<[i32; 4]> {
        *LAST_CARET_ERR.lock().unwrap_or_else(|e| e.into_inner()) = Some(s);
        None
    };

    // conhost → 父进程（= 控制台客户端）
    let parent = unsafe {
        let snap = match CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
            Ok(h) => h,
            Err(_) => return note("snapshot_fail"),
        };
        let mut pe = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut found = None;
        if Process32FirstW(snap, &mut pe).is_ok() {
            loop {
                if pe.th32ProcessID == conhost_pid {
                    found = Some(pe.th32ParentProcessID);
                    break;
                }
                if Process32NextW(snap, &mut pe).is_err() {
                    break;
                }
            }
        }
        let _ = windows::Win32::Foundation::CloseHandle(snap);
        match found {
            Some(p) => p,
            None => return note("parent_not_found"),
        }
    };

    unsafe {
        // 挂载（本 server 是 GUI 子系统无控制台；万一被拉起时带了——
        // 先 Free 再 Attach，用毕也 Free，状态归零）
        if AttachConsole(parent).is_err() {
            let _ = FreeConsole();
            if AttachConsole(parent).is_err() {
                return note("attach_fail");
            }
        }
        let result = (|| -> Option<(u16, String, [i32; 4])> {
            let h = match GetStdHandle(STD_OUTPUT_HANDLE) {
                Ok(h) if !h.is_invalid() => h,
                _ => return None,
            };
            let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
            if GetConsoleScreenBufferInfo(h, &mut info).is_err() {
                return None;
            }
            let mut cfi = CONSOLE_FONT_INFO::default();
            if GetCurrentConsoleFont(h, false, &mut cfi).is_err() {
                return None;
            }
            let hwnd = GetConsoleWindow();
            if hwnd.0.is_null() {
                return None;
            }
            let mut cr = RECT::default();
            if GetClientRect(hwnd, &mut cr).is_err() {
                return None;
            }
            let mut org = POINT { x: 0, y: 0 };
            if !ClientToScreen(hwnd, &mut org).as_bool() {
                return None;
            }
            let vx = (info.dwCursorPosition.X - info.srWindow.Left) as i32;
            let vy = (info.dwCursorPosition.Y - info.srWindow.Top) as i32;
            if vx < 0 || vy < 0 {
                return None;
            }
            let cw = cfi.dwFontSize.X.max(4) as i32;
            let chh = cfi.dwFontSize.Y.max(4) as i32;
            let left = org.x + vx * cw;
            let top = org.y + vy * chh;
            Some((parent as u16, format!("cell=({vx},{vy})"), [left, top, left + cw, top + chh]))
        })();
        let _ = FreeConsole();
        match result {
            Some((_p, cell, rect)) => {
                *LAST_CARET_ERR.lock().unwrap_or_else(|e| e.into_inner()) = None;
                let _ = cell; // 细节诊断保留位
                Some(rect)
            }
            None => note("read_fail"),
        }
    }
}

/// console_caret 失败原因（诊断回传给 DLL）。
#[cfg(windows)]
fn console_caret_last_err() -> &'static str {
    LAST_CARET_ERR
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .unwrap_or("unknown")
}

#[cfg(not(windows))]
fn console_caret_rect_via_attach(_conhost_pid: u32) -> Option<[i32; 4]> {
    None
}

/// 非 Windows 无控制台光标探测：console_caret 回 rect=null + 原因，
/// DLL/前端按失败落标准链（不阻塞 Linux 构建）。
#[cfg(not(windows))]
fn console_caret_last_err() -> &'static str {
    "unsupported"
}

fn export_candidates(host: &mut Host, req: &serde_json::Value, state: &mut serde_json::Value) {
    if req
        .pointer("/candidate_ui/protocol")
        .and_then(|v| v.as_u64())
        != Some(1)
    {
        return;
    }
    let context = req
        .pointer("/candidate_ui/context")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let menu = if !context.is_empty() && context == host.candidate_export.owner {
        host.engine.candidate_menu(&host.session)
    } else {
        None
    };
    state["candidate_ui"] = host.candidate_export.response(req, menu);
}

#[cfg(test)]
mod candidate_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn candidate_dispatch_animation_pages_query_and_cost() {
        let root =
            std::env::temp_dir().join(format!("hufu-candidate-dispatch-{}", std::process::id()));
        std::fs::create_dir_all(root.join("码表/T")).unwrap();
        std::fs::create_dir_all(root.join("数据")).unwrap();
        std::fs::write(
            root.join("码表/T/main.txt"),
            "#hufu-dict v1 name=t\na\t甲\na\t乙\na\t丙\na\t𠀀\n",
        )
        .unwrap();
        std::fs::write(
            root.join("数据/config.json"),
            r#"{"schema":{"dir":"码表","current":"T"},"candidates":{"page_size":2}}"#,
        )
        .unwrap();
        let mut h = Host::new(&root.join("数据")).unwrap();
        h.engine.config.candidates.page_size = 2;
        let host = Mutex::new(h);
        let request = |key: &str| json!({"op":"key","key":key,"candidate_ui":{"protocol":1,"context":"field-a"}});
        let a = dispatch(&host, &request("a"), None);
        assert_eq!(a["state"]["candidate_ui"]["menu"]["pages"], json!([0, 2]));
        let query = json!({"op":"state","candidate_ui":{"protocol":1,"context":"field-a",
            "instance":a["state"]["candidate_ui"]["instance"],"version":a["state"]["candidate_ui"]["version"]}});
        let before = format!("{:?}", host.lock().unwrap().session);
        let q = dispatch(&host, &query, None);
        assert!(q["state"]["candidate_ui"].get("menu").is_none());
        assert_eq!(format!("{:?}", host.lock().unwrap().session), before);
        let foreign = dispatch(
            &host,
            &json!({"op":"state","candidate_ui":{"protocol":1,"context":"field-b"}}),
            None,
        );
        assert!(foreign["state"]["candidate_ui"]["menu"].is_null());
        assert!(dispatch(&host, &json!({"op":"state"}), None)["state"]
            .get("candidate_ui")
            .is_none());
        let p = dispatch(&host, &request("pagedown"), None);
        assert_eq!(p["state"]["candidate_ui"]["menu"]["page"], 1);
        let commit = dispatch(&host, &request("2"), None);
        assert!(commit["outcome"]["commit"]
            .as_str()
            .is_some_and(|s| !s.is_empty()));
        assert!(
            commit["state"]["candidate_ui"]["menu"].is_null(),
            "confirmation frame must never be selectable"
        );
        // Residual input and asynchronous ranking use the live session, never outcome.state.
        {
            let mut h = host.lock().unwrap();
            h.session.raw = "residual".into();
            h.session.page = 1;
            h.session.selected = 3;
            h.session.candidates = ["甲", "乙", "丙", "丁"]
                .iter()
                .map(|s| hufu_types::Candidate::new(*s, "r", hufu_types::CandidateKind::Sentence))
                .collect();
        }
        let r = dispatch(&host, &query, None);
        assert_eq!(r["state"]["candidate_ui"]["menu"]["page"], 1);
        {
            host.lock().unwrap().session.candidates.swap(2, 3);
        }
        let reordered = dispatch(&host, &query, None);
        assert_ne!(
            r["state"]["candidate_ui"]["version"],
            reordered["state"]["candidate_ui"]["version"]
        );
        {
            host.lock().unwrap().session.candidates[0].text = "x".repeat(1 << 20);
        }
        assert!(dispatch(&host, &query, None)["state"]["candidate_ui"]["menu"].is_null());
        dispatch(&host, &json!({"op":"reset"}), None);
        assert!(dispatch(&host, &query, None)["state"]["candidate_ui"]["menu"].is_null());
        // Frozen tiny dictionary; this measures isolated dispatch, not model/UI latency.
        let mut samples = [Vec::<u128>::new(), Vec::new()];
        for trial in 0..200 {
            for enabled in 0..2 {
                host.lock().unwrap().session.clear();
                let req = if enabled == 1 {
                    request("a")
                } else {
                    json!({"op":"key","key":"a"})
                };
                let t = std::time::Instant::now();
                let response = dispatch(&host, &req, None);
                let bytes = serde_json::to_vec(&response).unwrap().len();
                if trial >= 20 {
                    samples[enabled].push(t.elapsed().as_nanos());
                }
                if trial == 199 {
                    println!("candidate benchmark enabled={enabled} response_bytes={bytes}");
                }
            }
        }
        for (i, s) in samples.iter_mut().enumerate() {
            s.sort();
            println!(
                "candidate dispatch enabled={i} p50_ns={} p95_ns={}",
                s[s.len() / 2],
                s[s.len() * 95 / 100]
            );
        }
        println!(
            "candidate unchanged extension bytes={}",
            serde_json::to_vec(&q["state"]["candidate_ui"])
                .unwrap()
                .len()
        );
    }

    // 【按住Ctrl调频·对调 2026-11】cand_swap op：页内下标+词校验 →
    // 原位交换（raw/页/选保活）+ 整序落盘；漂移按词重找；同位/越界/
    // 不在页拒绝。
    #[test]
    fn cand_swap_op_inplace_and_drift() {
        let root = std::env::temp_dir().join(format!("hufu-cand-swap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("码表/T")).unwrap();
        std::fs::create_dir_all(root.join("数据")).unwrap();
        std::fs::write(
            root.join("码表/T/main.txt"),
            "#hufu-dict v1 name=t\na\t甲\na\t乙\na\t丙\n",
        )
        .unwrap();
        std::fs::write(
            root.join("数据/config.json"),
            r#"{"schema":{"dir":"码表","current":"T"},"candidates":{"page_size":5}}"#,
        )
        .unwrap();
        let mut h = Host::new(&root.join("数据")).unwrap();
        let host = Mutex::new(h);
        let key = json!({"op":"key","key":"a"});
        let r = dispatch(&host, &key, None);
        assert_eq!(
            r["state"]["candidates"][0]["text"],
            json!("甲"),
            "首选拍板: {r}"
        );
        // 1↔3（页内 0/2）：原位交换，raw/selected 保活
        {
            let mut g = host.lock().unwrap();
            g.session.selected = 1;
        }
        let sw = dispatch(
            &host,
            &json!({"op":"cand_swap","i":0,"j":2,"text_i":"甲","text_j":"丙"}),
            None,
        );
        assert_eq!(sw["ok"], json!(true), "对调成功: {sw}");
        let texts: Vec<String> = {
            let g = host.lock().unwrap();
            g.session.candidates.iter().map(|c| c.text.clone()).collect()
        };
        assert_eq!(texts, ["丙", "乙", "甲"], "原位交换: {texts:?}");
        {
            let g = host.lock().unwrap();
            assert_eq!(g.session.raw, "a", "raw 保活（尾巴态不翻转）");
            assert_eq!(g.session.selected, 1, "选中位保活");
        }
        assert_eq!(sw["state"]["candidates"][0]["text"], json!("丙"));
        // 落盘：整序标记 + 整套 pN（用户调整.txt 落方案目录）
        let log = std::fs::read_to_string(root.join("码表/T/用户调整.txt")).unwrap();
        assert!(log.contains("{整序}a"), "整序标记: {log}");
        assert!(log.contains("{添加}a\t丙\tp1"), "丙 p1: {log}");
        assert!(log.contains("{添加}a\t甲\tp3"), "甲 p3: {log}");
        // 漂移：异步重排把词挪位（模拟 0/1 互换）——旧下标+正确词文本
        // 仍按词解析成功
        {
            host.lock().unwrap().session.candidates.swap(0, 1);
        }
        let sw2 = dispatch(
            &host,
            &json!({"op":"cand_swap","i":0,"j":2,"text_i":"甲","text_j":"丙"}),
            None,
        );
        assert_eq!(sw2["ok"], json!(true), "漂移按词重找: {sw2}");
        let texts2: Vec<String> = {
            let g = host.lock().unwrap();
            g.session.candidates.iter().map(|c| c.text.clone()).collect()
        };
        // 漂移后按词对调：手动 swap(0,1) 后列表 [乙,丙,甲]——甲(位2)↔丙(位1)
        // → [乙,甲,丙]（词本位交换，与旧下标无关）
        assert_eq!(texts2, ["乙", "甲", "丙"], "漂移后对调: {texts2:?}");
        // 同位 / 越界 / 词不在页 → ok:false
        let same = dispatch(&host, &json!({"op":"cand_swap","i":1,"j":1}), None);
        assert_eq!(same["ok"], json!(false), "同位拒绝");
        let oob = dispatch(&host, &json!({"op":"cand_swap","i":0,"j":9}), None);
        assert_eq!(oob["ok"], json!(false), "页外拒绝");
        let miss = dispatch(
            &host,
            &json!({"op":"cand_swap","i":0,"j":2,"text_i":"不存在","text_j":"丙"}),
            None,
        );
        assert_eq!(miss["ok"], json!(false), "词不在列表拒绝");
        let _ = std::fs::remove_dir_all(&root);
    }
}
