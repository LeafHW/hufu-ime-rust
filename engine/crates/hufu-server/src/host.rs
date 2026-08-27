//! 引擎宿主：配置 + 引擎实例 + 会话 + 整句装配，供 HTTP API 与 IPC 共用。

use hufu_config::Config;
use hufu_engine::{Engine, Session};
use hufu_sentence::SentenceEngine;
use hufu_types::{KeyCode, KeyInput, Modifiers};
use std::path::{Path, PathBuf};

pub struct Host {
    pub engine: Engine,
    pub session: Session,
    pub data_dir: PathBuf,
    pub config_path: PathBuf,
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
        };
        host.setup_sentence();
        Ok(host)
    }

    /// 依据配置与磁盘可用性装配整句解码器。
    pub fn setup_sentence(&mut self) {
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

    /// 按键 → (结果, 状态快照)。
    pub fn process_key(&mut self, key: KeyInput) -> serde_json::Value {
        let outcome = self.engine.process_key(&mut self.session, key);
        let state = self.engine.state(&self.session);
        serde_json::json!({ "outcome": outcome, "state": state })
    }

    /// 应用新配置：落盘 + 热更新 + 必要时重装整句。
    pub fn apply_config(&mut self, cfg: Config) -> std::io::Result<()> {
        let sentence_changed = cfg.sentence != self.engine.config.sentence;
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
        Ok(())
    }

    /// 皮肤目录。
    pub fn skins_dir(&self) -> PathBuf {
        self.data_dir.join("skins")
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
