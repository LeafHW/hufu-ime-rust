//! 引擎宿主：配置 + 引擎实例 + 会话 + 整句装配，供 HTTP API 与 IPC 共用。

use hufu_config::Config;
use hufu_engine::{Engine, Session};
use hufu_sentence::SentenceEngine;
use hufu_types::{KeyCode, KeyInput, Modifiers};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

/// 重排任务：key=committed_raw+raw（结果只对同 key 生效）
struct RerankJob {
    key: String,
    ctx: String,
    cands: Vec<String>,
}

pub struct Host {
    pub engine: Engine,
    pub session: Session,
    pub data_dir: PathBuf,
    pub config_path: PathBuf,
    /// 神经重排：任务发送端（None=未启用/模型缺失）
    rerank_tx: Option<mpsc::Sender<RerankJob>>,
}

impl Host {
    pub fn new(data_dir: &Path) -> std::io::Result<Host> {
        let config_path = data_dir.join("config.json");
        let config = if config_path.exists() {
            Config::load(&config_path)?
        } else {
            Config::default()
        };
        let engine = Engine::new(data_dir, config)?;
        let mut host = Host {
            engine,
            session: Session::new(true),
            data_dir: data_dir.to_path_buf(),
            config_path,
            rerank_tx: None,
        };
        host.install_official_skins();
        host.setup_rerank();
        // 注意：整句模型不在此时同步加载——由 main.rs 的后台线程
        // 装载后热挂（见 sentence_load_plan）。启动只载词典（秒级），
        // 管道/设置页即刻可用；装完立即可打字，整句能力稍后自动就位。
        Ok(host)
    }

    /// 官方皮肤自愈落盘：内嵌皮肤（official-skins/，随 git 与二进制分发）
    /// 缺失时写入数据目录 skins/。已存在的不覆盖——用户在设置页的定制优先。
    fn install_official_skins(&mut self) {
        /// 嵌入的官方皮肤（编译期打包，数据目录损坏/清空也能恢复全套）
        const OFFICIAL: &[(&str, &str)] = &[
            ("hufu-frost-h.json", include_str!("../official-skins/hufu-frost-h.json")),
            ("hufu-moyan.json", include_str!("../official-skins/hufu-moyan.json")),
            ("hufu-qingkong.json", include_str!("../official-skins/hufu-qingkong.json")),
            ("hufu-yingxiong.json", include_str!("../official-skins/hufu-yingxiong.json")),
        ];
        let dir = self.skins_dir();
        let _ = std::fs::create_dir_all(&dir);
        for (file, body) in OFFICIAL {
            let p = dir.join(file);
            if !p.exists() {
                if let Err(e) = std::fs::write(&p, body) {
                    eprintln!("官方皮肤 {file} 落盘失败: {e}");
                }
            }
        }
    }

    /// 后台加载整句模型的「装载计划」：快照判定条件与所需所有权
    /// （词典 Arc/补充语料/权重），真正的重活（ngram 载入）交给调用
    /// 线程**不持锁**执行——期间按键请求照常被词典模式服务。
    pub fn sentence_load_plan(
        &self,
    ) -> Option<(
        PathBuf,
        std::sync::Arc<hufu_dict::dict::Dict>,
        hufu_dict::supplement::Supplement,
        hufu_config::SentenceWeights,
    )> {
        if !self.engine.config.schema.current.contains("整句") {
            return None;
        }
        let path = self.data_dir.join(&self.engine.config.sentence.ngram_path);
        if !(self.engine.config.sentence.enabled && path.exists()) {
            return None;
        }
        Some((
            path,
            self.engine.schema.dict.clone(),
            self.engine.schema.supplement.clone(),
            self.engine.config.sentence.weights.clone(),
        ))
    }

    /// 依据配置与磁盘可用性装配整句解码器。
    /// **整句只属于「整句系」方案**（方案名含「整句」，与设置页的
    /// 整句标签同约定）：其余方案一律不启用整句引擎——码表类方案
    /// 与整句模型词典不匹配，混装只会空转耗内存。
    pub fn setup_sentence(&mut self) {
        let cur = self.engine.config.schema.current.clone();
        if !cur.contains("整句") {
            self.engine.set_sentence_decoder(None);
            return;
        }
        let path = self.data_dir.join(&self.engine.config.sentence.ngram_path);
        if self.engine.config.sentence.enabled && path.exists() {
            match SentenceEngine::load(
                &path,
                self.engine.schema.dict.clone(),
                &self.engine.schema.supplement,
                self.engine.config.sentence.weights.clone(),
            ) {
                Ok(dec) => {
                    self.engine.set_sentence_decoder(Some(std::sync::Arc::new(dec)));
                    eprintln!("整句引擎已加载: {}", path.display());
                    return;
                }
                Err(e) => eprintln!("整句模型加载失败: {e}"),
            }
        }
        self.engine.set_sentence_decoder(None);
    }

    /// 模型路径解析：配置值（相对→数据目录）→ 数据目录 models/ 自动探测。
    fn resolve_rerank_model(&self) -> Option<PathBuf> {
        let cfg = &self.engine.config.sentence.rerank;
        if !cfg.enabled {
            return None;
        }
        let mut candidates: Vec<PathBuf> = Vec::new();
        let p = PathBuf::from(&cfg.model_path);
        if p.is_absolute() {
            candidates.push(p);
        } else if !cfg.model_path.is_empty() {
            candidates.push(self.data_dir.join(&cfg.model_path));
        }
        // 自动探测：数据目录「模型」下任意 .gguf（排除 ngram bin）
        if let Ok(rd) = std::fs::read_dir(self.data_dir.join("模型")) {
            let mut ggufs: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().map(|x| x == "gguf").unwrap_or(false))
                .collect();
            ggufs.sort();
            candidates.extend(ggufs);
        }
        candidates.into_iter().find(|p| p.exists())
    }

    /// 装配神经重排工作线程（旧线程随发送端 Drop 退出）。
    pub fn setup_rerank(&mut self) {
        // 关旧：Drop Sender 使 recv 返回 Err → 线程自然退出
        self.rerank_tx = None;
        let Some(model) = self.resolve_rerank_model() else {
            eprintln!("神经重排：未启用或模型缺失");
            return;
        };
        let debounce = self.engine.config.sentence.rerank.debounce_ms;
        let cache = self.engine.rerank_cache.clone();
        let model_path = model.to_string_lossy().into_owned();
        let (tx, rx) = mpsc::channel::<RerankJob>();
        std::thread::Builder::new()
            .name("hufu-rerank".into())
            .spawn(move || {
                // ── 双引擎：优先虎爪 llama.cpp 原生（81ms/2cand），失败回落纯 Rust ──
                // 两引擎同 GGUF 文件、同判序语义（native 侧 ctx 拼串整句概率，
                // 句首/句中/成语三案实测全判对）。
                let mut native: Option<hufu_rerank::native::NativeScorer> = None;
                let mp = std::path::PathBuf::from(&model_path);
                if let Some(ns) = hufu_rerank::native::NativeScorer::try_new(&[], &mp) {
                    let _ = ns.score("。", &["预热".to_string()]);
                    let _ = std::fs::create_dir_all(r"C:\ProgramData\HuFu\diag");
                    let _ = std::fs::write(
                        r"C:\ProgramData\HuFu\diag\rerank-engine.txt",
                        "native(llama.cpp)",
                    );
                    native = Some(ns);
                } else {
                    let _ = std::fs::create_dir_all(r"C:\ProgramData\HuFu\diag");
                    let _ = std::fs::write(
                        r"C:\ProgramData\HuFu\diag\rerank-engine.txt",
                        "rust(fallback)",
                    );
                }
                let mut model: Option<hufu_rerank::Reranker> = None;
                let mut model_failed = false;
                if native.is_none() {
                    // 预热：server 启动即后台加载模型并空跑一次前向（触页），
                    // 避免用户首次停顿重排等冷读盘 ~10s（体验即「重排没生效」）。
                    // GGUF_LAZY 页缓存机制不变——私有内存仍由页缓存承载。
                    {
                        std::env::set_var("GGUF_LAZY", "1");
                        match hufu_rerank::Reranker::load(&model_path) {
                            Ok(r) => {
                                let _ = r.score("。", &["预热".to_string()]);
                                model = Some(r);
                            }
                            Err(_e) => {
                                model_failed = true;
                            }
                        }
                    }
                }
                while let Ok(job) = rx.recv() {
                    if model_failed {
                        continue;
                    }
                    // 去抖：停顿期间新任务覆盖旧任务
                    std::thread::sleep(std::time::Duration::from_millis(debounce));
                    let mut cur = job;
                    while let Ok(j) = rx.try_recv() {
                        cur = j;
                    }
                    if cur.cands.len() < 2 {
                        continue;
                    }
                    // 打分（native 优先；native 缺席时纯 Rust 懒加载兜底）
                    let t_score = std::time::Instant::now();
                    let scores: Option<Vec<f64>> = if let Some(ns) = &native {
                        Some(ns.score(&cur.ctx, &cur.cands))
                    } else {
                        if model.is_none() && !model_failed {
                            std::env::set_var("GGUF_LAZY", "1");
                            match hufu_rerank::Reranker::load(&model_path) {
                                Ok(r) => model = Some(r),
                                Err(_e) => {
                                    model_failed = true;
                                    continue;
                                }
                            }
                        }
                        let Some(r) = &model else { continue };
                        Some(std::thread::scope(|s| {
                            let handles: Vec<_> = cur
                                .cands
                                .iter()
                                .map(|c| {
                                    let ctx = &cur.ctx;
                                    s.spawn(move || {
                                        r.score(ctx, std::slice::from_ref(c))[0]
                                    })
                                })
                                .collect();
                            handles
                                .into_iter()
                                .map(|h| h.join().unwrap_or(f64::NEG_INFINITY))
                                .collect()
                        }))
                    };
                    let Some(scores) = scores else { continue };
                    let elapsed = t_score.elapsed().as_millis();
                    let mut order: Vec<(f64, String)> = scores
                        .into_iter()
                        .zip(cur.cands.iter().cloned())
                        .collect();
                    order.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
                    let texts: Vec<String> = order.into_iter().map(|(_, t)| t).collect();
                    // GUI 子系统 eprintln 无人见——重排计时落文件（性能排查生命线）
                    let _ = std::fs::create_dir_all(r"C:\ProgramData\HuFu\diag");
                    let _ = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(r"C:\ProgramData\HuFu\diag\rerank.log")
                        .and_then(|mut f| {
                            use std::io::Write;
                            f.write_all(
                                format!(
                                    "[{}] key={} score={}ms/{}cand → {}\n",
                                    std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|d| d.as_secs())
                                        .unwrap_or(0),
                                    cur.key,
                                    elapsed,
                                    cur.cands.len(),
                                    texts.join(" ")
                                )
                                .as_bytes(),
                            )
                        });
                    if let Ok(mut c) = cache.lock() {
                        if c.len() > 64 {
                            c.clear();
                        }
                        c.insert(cur.key, texts);
                    }
                }
            })
            .ok();
        eprintln!("神经重排线程就绪（模型 {}，去抖 {debounce}ms）", model.display());
        self.rerank_tx = Some(tx);
    }

    /// 输入法操作后钩子：有整句候选时派发重排任务（非阻塞）。
    pub fn after_ime_op(&mut self) {
        let Some(tx) = &self.rerank_tx else { return };
        if let Some((key, ctx, cands)) = self.engine.rerank_request(&self.session) {
            let _ = tx.send(RerankJob { key, ctx, cands });
        }
    }

    /// 按键 → (结果, 状态快照)。
    pub fn process_key(&mut self, key: KeyInput) -> serde_json::Value {
        // 通知重排 gemm：前台有按键，15ms 内让键（BelowNormal 池 + 让键双保险）
        hufu_rerank::note_foreground();
        let outcome = self.engine.process_key(&mut self.session, key);
        // 跨句文章尾巴：上屏文本滚动进 tail_context（截尾 32 字），
        // 供下一句句首的神经重排作真实语境（空 ctx 时 Qwen 会乱序）。
        if outcome.back > 0 {
            // 回删替换（数字后 1. 再按 . → 。）：tail 同步回退再滚动新文
            for _ in 0..outcome.back {
                self.session.tail_context.pop();
            }
        }
        if let Some(c) = outcome.commit.as_deref() {
            if !c.is_empty() {
                self.session.tail_context.push_str(c);
                let n = self.session.tail_context.chars().count();
                if n > 32 {
                    let skip = n - 32;
                    self.session.tail_context = self
                        .session
                        .tail_context
                        .chars()
                        .skip(skip)
                        .collect();
                }
            }
        }
        let state = self.engine.state(&self.session);
        serde_json::json!({ "outcome": outcome, "state": state })
    }

    /// 应用新配置：落盘 + 热更新 + 必要时重装整句/重排。
    pub fn apply_config(&mut self, cfg: Config) -> std::io::Result<()> {
        let sentence_changed = cfg.sentence != self.engine.config.sentence;
        let rerank_changed = sentence_changed;
        let schema_changed = cfg.schema.current != self.engine.config.schema.current
            || cfg.schema.dir != self.engine.config.schema.dir;
        cfg.save(&self.config_path)?;
        self.engine.config = cfg;
        if schema_changed {
            let name = self.engine.config.schema.current.clone();
            if let Err(e) = self.engine.switch_schema(&name) {
                return Err(e);
            }
            self.session.clear();
        }
        if sentence_changed || schema_changed {
            self.setup_sentence();
        }
        if rerank_changed {
            self.setup_rerank();
        }
        Ok(())
    }

    /// 皮肤目录。
    pub fn skins_dir(&self) -> PathBuf {
        self.data_dir.join("皮肤")
    }

    pub fn list_skins(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(self.skins_dir()) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().map(|x| x == "json").unwrap_or(false) {
                    let id = p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
                    let name = hufu_skin::Skin::load(&p)
                        .map(|s| s.name)
                        .unwrap_or_else(|_| id.clone());
                    out.push((id, name));
                }
            }
        }
        if out.is_empty() {
            out.push(("hufu-default".into(), "迷雾（内置）".into()));
        }
        out.sort();
        out
    }
}

/// 前端按键描述 → KeyInput：{"key":"a"|"space"|...,"shift":bool,...}
pub fn parse_key(v: &serde_json::Value) -> Option<KeyInput> {
    let s = v.get("key")?.as_str()?;
    let key = match s {
        "space" => KeyCode::Space,
        "enter" => KeyCode::Enter,
        "backspace" | "bs" => KeyCode::Backspace,
        "tab" => KeyCode::Tab,
        "escape" | "esc" => KeyCode::Escape,
        "delete" => KeyCode::Delete,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "capslock" => KeyCode::CapsLock,
        "shift" | "shiftleft" => KeyCode::ShiftLeft,
        "shiftright" => KeyCode::ShiftRight,
        "ctrl" | "ctrlleft" => KeyCode::CtrlLeft,
        "ctrlright" => KeyCode::CtrlRight,
        "alt" | "altleft" => KeyCode::AltLeft,
        "altright" => KeyCode::AltRight,
        _ => {
            let c = s.chars().next()?;
            KeyCode::Char(c)
        }
    };
    let m = v.get("modifiers").cloned().unwrap_or(serde_json::Value::Null);
    let modifiers = Modifiers {
        shift: m.get("shift").and_then(|x| x.as_bool()).unwrap_or(false),
        ctrl: m.get("ctrl").and_then(|x| x.as_bool()).unwrap_or(false),
        alt: m.get("alt").and_then(|x| x.as_bool()).unwrap_or(false),
        meta: m.get("meta").and_then(|x| x.as_bool()).unwrap_or(false),
        caps: m.get("caps").and_then(|x| x.as_bool()).unwrap_or(false),
    };
    Some(KeyInput {
        key,
        modifiers,
        is_press: true,
    })
}

/// 供管道线程共享的宿主句柄。
pub type SharedHost = Arc<Mutex<Host>>;
