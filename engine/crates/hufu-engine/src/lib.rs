//! hufu-engine —— 平台无关的输入法会话引擎。
//!
//! 状态机：按键 →（切换键 / 标点 / 反查 / 命令 / 编码追加与顶功 /
//! 候选生成 / 选重翻页）→ KeyOutcome。

pub mod punct;
pub mod session;

pub use punct::PairState;
pub use session::Session;

use hufu_config::Config;
use hufu_dict::entry::DictEntry;
use hufu_dict::schema::Schema;
use hufu_types::{
    Candidate, CandidateKind, InputMode, KeyCode, KeyInput, KeyOutcome, SessionState,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 整句解码器接口（由 hufu-sentence 实现，可注入替换）。
pub trait SentenceDecoder: Send + Sync {
    /// 组句：raw（含选重后缀）→ 已排序候选。
    fn decode(&self, raw: &str) -> Vec<Candidate>;
    /// 提前上屏提案：返回（建议上屏文本, 消耗的 raw 长度）。无提案返回 None。
    fn early_commit_proposal(&self, raw: &str) -> Option<(String, usize)>;
}

/// 引擎：配置 + 当前方案 + 可选整句解码器。
pub struct Engine {
    pub config: Config,
    pub schema: Schema,
    /// 可用方案名列表
    pub schemas: Vec<String>,
    /// 用户数据目录
    pub data_dir: PathBuf,
    sentence: Option<Arc<dyn SentenceDecoder>>,
}

impl Engine {
    pub fn new(data_dir: &Path, config: Config) -> std::io::Result<Engine> {
        let dict_root = data_dir.join(&config.schema.dir);
        let current = dict_root.join(&config.schema.current);
        let schema = Schema::load(&current)?;
        let mut schemas = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&dict_root) {
            for e in rd.flatten() {
                if e.path().is_dir() {
                    if let Some(n) = e.file_name().to_str() {
                        schemas.push(n.to_string());
                    }
                }
            }
        }
        Ok(Engine {
            config,
            schema,
            schemas,
            data_dir: data_dir.to_path_buf(),
            sentence: None,
        })
    }

    /// 直接从方案目录构建引擎（CLI / 测试用，无 dictionaries/ 包装）。
    pub fn with_schema_dir(schema_dir: &Path, config: Config) -> std::io::Result<Engine> {
        let schema = Schema::load(schema_dir)?;
        Ok(Engine {
            config,
            schemas: vec![schema.name.clone()],
            data_dir: schema_dir
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf(),
            schema,
            sentence: None,
        })
    }

    /// 注入整句解码器。
    pub fn set_sentence_decoder(&mut self, dec: Option<Arc<dyn SentenceDecoder>>) {
        self.sentence = dec;
    }

    pub fn sentence_decoder(&self) -> Option<&Arc<dyn SentenceDecoder>> {
        self.sentence.as_ref()
    }

    /// 切换方案。
    pub fn switch_schema(&mut self, name: &str) -> std::io::Result<()> {
        let dir = self.data_dir.join(&self.config.schema.dir).join(name);
        self.schema = Schema::load(&dir)?;
        self.config.schema.current = name.to_string();
        Ok(())
    }

    /// 重新加载用户数据（用户词/用户调整），码表主体不重载。
    pub fn reload_user_data(&mut self) {
        let dir = self.schema.dir.clone();
        let uw = dir.join("用户词.txt");
        if let Ok(ud) = hufu_dict::user::UserDict::load(&uw) {
            self.schema.user_dict = ud;
        }
        let adj = dir.join("用户调整.txt");
        if let Ok(a) = hufu_dict::user::UserAdjust::load(&adj) {
            self.schema.adjust = a;
        }
    }

    /// 方案是否启用整句。
    pub fn sentence_active(&self) -> bool {
        if !self.config.sentence.enabled || self.sentence.is_none() {
            return false;
        }
        if self.config.sentence.auto_enable {
            self.schema.name.contains("整句")
        } else {
            true
        }
    }

    /// 处理一次按键。
    pub fn process_key(&mut self, session: &mut Session, key: KeyInput) -> KeyOutcome {
        if !key.is_press {
            return KeyOutcome::passthrough();
        }
        let m = key.modifiers;
        if m.ctrl && !m.alt {
            if let Some(c) = key.key.as_char() {
                if c == ' ' && self.config.general.ctrl_space_switch {
                    session.chinese = !session.chinese;
                    session.clear();
                    return KeyOutcome::consumed(self.state(session));
                }
                if c == 'm' && !m.shift && self.config.general.switch_recent_schema {
                    if let Some((a, b)) = self.config.schema.recent_pair.clone() {
                        let target = if self.config.schema.current == a { b } else { a };
                        if target != self.config.schema.current {
                            let _ = self.switch_schema(&target);
                            session.clear();
                            return KeyOutcome::consumed(self.state(session));
                        }
                    }
                }
            }
            return KeyOutcome::passthrough();
        }
        if m.alt || m.meta {
            return KeyOutcome::passthrough();
        }

        // Caps
        if key.key == KeyCode::CapsLock {
            match self.config.general.caps_action {
                hufu_config::CapsAction::Clear => {
                    if !session.raw.is_empty() {
                        session.clear();
                        return KeyOutcome::consumed(self.state(session));
                    }
                }
                hufu_config::CapsAction::Switch => {
                    session.chinese = !session.chinese;
                    session.clear();
                    return KeyOutcome::consumed(self.state(session));
                }
                hufu_config::CapsAction::None => {}
            }
            return KeyOutcome::passthrough();
        }

        // Shift 单击切换中英（有编码时不处理）
        if matches!(key.key, KeyCode::ShiftLeft | KeyCode::ShiftRight)
            && self.config.general.shift_switch
        {
            if session.raw.is_empty() {
                session.chinese = !session.chinese;
                session.pair.reset();
                return KeyOutcome::consumed(self.state(session));
            }
            return KeyOutcome::passthrough();
        }

        match key.key {
            KeyCode::Backspace => self.on_backspace(session),
            KeyCode::Escape => {
                if !session.raw.is_empty() || session.mode != InputMode::Normal {
                    session.clear();
                    KeyOutcome::consumed(self.state(session))
                } else {
                    KeyOutcome::passthrough()
                }
            }
            KeyCode::Enter => {
                if session.raw.is_empty() {
                    return KeyOutcome::passthrough();
                }
                if self.config.input.enter_clear {
                    session.clear();
                    KeyOutcome::consumed(self.state(session))
                } else {
                    let raw = std::mem::take(&mut session.raw);
                    session.candidates.clear();
                    KeyOutcome::commit(raw, self.state(session))
                }
            }
            KeyCode::Tab => {
                if session.raw.is_empty() {
                    return KeyOutcome::passthrough();
                }
                if self.config.input.tab_clear {
                    session.clear();
                    KeyOutcome::consumed(self.state(session))
                } else {
                    self.on_page(session, 1)
                }
            }
            KeyCode::Up => {
                if session.raw.is_empty() {
                    KeyOutcome::passthrough()
                } else {
                    self.on_page(session, -1)
                }
            }
            KeyCode::Down | KeyCode::PageDown => {
                if session.raw.is_empty() {
                    KeyOutcome::passthrough()
                } else {
                    self.on_page(session, 1)
                }
            }
            KeyCode::PageUp => {
                if session.raw.is_empty() {
                    KeyOutcome::passthrough()
                } else {
                    self.on_page(session, -1)
                }
            }
            KeyCode::Char(c) => self.on_char(session, c, m.shift),
            KeyCode::Space => self.on_char(session, ' ', false),
            KeyCode::Left | KeyCode::Right | KeyCode::Home | KeyCode::End | KeyCode::Delete => {
                if !session.raw.is_empty() {
                    KeyOutcome::consumed(self.state(session))
                } else {
                    KeyOutcome::passthrough()
                }
            }
            _ => KeyOutcome::passthrough(),
        }
    }

    fn on_backspace(&mut self, session: &mut Session) -> KeyOutcome {
        if session.raw.is_empty() {
            return KeyOutcome::passthrough();
        }
        session.raw.pop();
        if session.raw.is_empty() {
            session.clear();
        } else {
            self.refresh_candidates(session);
        }
        KeyOutcome::consumed(self.state(session))
    }

    fn on_char(&mut self, session: &mut Session, c: char, shift: bool) -> KeyOutcome {
        if !session.chinese {
            return KeyOutcome::passthrough();
        }

        // 反查模式
        if session.mode == InputMode::Reverse {
            return self.on_reverse_char(session, c);
        }

        // 命令模式
        if session.mode == InputMode::Command {
            return self.on_command_char(session, c);
        }

        if session.raw.is_empty() {
            // 反查引导
            if c == self.config.reverse.prefix && self.config.reverse.enabled {
                session.mode = InputMode::Reverse;
                return KeyOutcome::consumed(self.state(session));
            }
            // 命令命名空间
            if c == '\\' {
                session.mode = InputMode::Command;
                session.raw = "\\".into();
                self.refresh_candidates(session);
                return KeyOutcome::consumed(self.state(session));
            }
            // '/' 符号命名空间（首选顿号，继续输入进入 /xx 符号）
            if c == '/'
                && (self.config.input.slash_dunhao || self.has_continuation_prefix("/"))
            {
                session.raw.push(c);
                self.refresh_candidates(session);
                return KeyOutcome::consumed(self.state(session));
            }
            // 编码字符
            if self.config.input.is_alphabet_char(c) && !shift {
                session.raw.push(c);
                self.after_append(session);
                return self.take_or_state(session);
            }
            // 大写字母：混合输入缓冲（不限长混输）
            if self.config.input.mixed_input && c.is_ascii_uppercase() {
                session.raw.push(c);
                self.refresh_candidates(session);
                return KeyOutcome::consumed(self.state(session));
            }
            // 标点
            if let Some(text) = self.punct_output(session, c) {
                return KeyOutcome::commit(text, self.state(session));
            }
            return KeyOutcome::passthrough();
        }

        // —— 有编码态 ——
        let extends = self.has_continuation_prefix(&format!("{}{c}", session.raw));
        // 选重键（不构成编码延续时才作为选重）
        if !extends {
            if c == self.config.candidates.second_select {
                return self.select_candidate(session, 1);
            }
            if c == self.config.candidates.third_select {
                return self.select_candidate(session, 2);
            }
        }
        // 编码字符（含死路：交给 after_append 处理顶功/清屏）
        if self.config.input.is_alphabet_char(c) && !shift {
            session.raw.push(c);
            self.after_append(session);
            return self.take_or_state(session);
        }
        // 混合输入：大写继续追加
        if self.config.input.mixed_input && c.is_ascii_uppercase() {
            session.raw.push(c);
            self.refresh_candidates(session);
            return KeyOutcome::consumed(self.state(session));
        }
        // 选重键
        if c == self.config.candidates.second_select {
            return self.select_candidate(session, 1);
        }
        if c == self.config.candidates.third_select {
            return self.select_candidate(session, 2);
        }
        // 翻页键
        if self.config.candidates.paging_keys.contains(c) {
            let idx = self
                .config
                .candidates
                .paging_keys
                .chars()
                .position(|x| x == c)
                .unwrap_or(0);
            let dir = if idx == 0 { -1 } else { 1 };
            return self.on_page(session, dir);
        }
        // 数字选重
        if let Some(n) = c.to_digit(10) {
            let idx = if n == 0 { 9 } else { (n - 1) as usize };
            return self.select_candidate(session, idx);
        }
        // 空格首选
        if c == ' ' {
            return self.select_first(session);
        }
        // 编码态标点：顶字（提交首选后输出标点）
        if let Some(punct) = self.punct_output(session, c) {
            if !session.candidates.is_empty() {
                let first = session.candidates[0].commit_text().to_string();
                session.clear();
                return KeyOutcome::commit(format!("{first}{punct}"), self.state(session));
            }
            session.clear();
            return KeyOutcome::commit(punct, self.state(session));
        }
        if c.is_ascii_alphanumeric() {
            return KeyOutcome::consumed(self.state(session));
        }
        KeyOutcome::passthrough()
    }

    /// 标点输出（全角/半角/引号配对）。
    fn punct_output(&mut self, session: &mut Session, c: char) -> Option<String> {
        if !c.is_ascii_punctuation() {
            return None;
        }
        if self.config.input.ascii_punct {
            return Some(c.to_string());
        }
        if self.config.punct.pair_brackets {
            if let Some(q) = session.pair.quote(c) {
                return Some(q.to_string());
            }
        } else if c == '\'' || c == '"' {
            return Some(c.to_string());
        }
        punct::to_full_width_punct(c)
    }

    /// 编码追加后的顶功 / 自动上屏 / 快符唯一上屏判定。
    fn after_append(&mut self, session: &mut Session) {
        let prev_cands = session.candidates.clone(); // 追加前 raw 的候选
        let c = session.raw.chars().last().unwrap_or(' ');
        self.refresh_candidates(session);
        let raw = session.raw.clone();
        let len = raw.chars().count();
        let max_len = self.config.input.max_code_length;
        let has_upper = raw.chars().any(|x| x.is_ascii_uppercase());

        // 快符 / 符号：唯一候选立即上屏（auto_select_pattern ^;\w+ 语义，至少两码）
        if (raw.starts_with(';') || raw.starts_with('/'))
            && len >= 2
            && session.candidates.len() == 1
        {
            self.commit_first_inline(session);
            return;
        }

        // 满码唯一上屏
        if len == max_len
            && self.config.input.auto_select_unique
            && session.candidates.len() == 1
        {
            self.commit_first_inline(session);
            return;
        }

        // 整句方案：超过最大码长后由整句解码器接管（不顶功、不清屏）
        let sentence_takeover = self.sentence_active() && len > max_len;

        // 顶功：超长（第 max+1 码）或死路（新码无任何延续）
        let dead_end = session.candidates.is_empty() && !self.has_continuation(&raw);
        let over_length = len > max_len;
        if (over_length || dead_end) && !sentence_takeover && self.config.input.auto_push && !has_upper
        {
            if let Some(first) = prev_cands.first().cloned() {
                // 提交追加前 raw 的首选，新 raw 从刚输入的字符重新开始
                self.learn(&first);
                session.clear();
                session.raw = c.to_string();
                self.refresh_candidates(session);
                session.pending_commit = Some(first.commit_text().to_string());
                return;
            }
            // 追加前也无候选：空码处理
            if dead_end && self.config.input.auto_clear_empty {
                session.clear();
            }
            return;
        }

        // 空码自动清屏（既无精确也无前缀，且未开启顶功短路）
        if dead_end && !sentence_takeover && self.config.input.auto_clear_empty && !has_upper {
            session.clear();
        }
    }

    /// raw 是否还有编码延续（前缀树或符号表）。
    fn has_continuation(&self, raw: &str) -> bool {
        if !self.schema.dict.completions(raw, 1).is_empty() {
            return true;
        }
        if raw.starts_with(';') || raw.starts_with('/') {
            let map = self.schema.symbols.merge_code_map();
            return map.keys().any(|k| k.starts_with(raw));
        }
        false
    }

    /// 某串是否为某编码（或符号码）的前缀。
    fn has_continuation_prefix(&self, s: &str) -> bool {
        if !self.schema.dict.completions(s, 1).is_empty() {
            return true;
        }
        if s.starts_with(';') || s.starts_with('/') {
            let map = self.schema.symbols.merge_code_map();
            return map.keys().any(|k| k.starts_with(s) || k == s);
        }
        false
    }

    /// 内联提交首选（顶功 / 唯一上屏）：置 pending_commit，由 take_or_state 消费。
    fn commit_first_inline(&mut self, session: &mut Session) {
        if session.candidates.is_empty() {
            return;
        }
        let first = session.candidates[0].clone();
        self.learn(&first);
        let text = first.commit_text().to_string();
        session.clear();
        session.pending_commit = Some(text);
    }

    /// 消费内联上屏，组装 KeyOutcome。
    /// 整句模式：提前上屏提案跟踪。同一提案连续 3 键稳定则自动上屏前缀，
    /// 剩余 raw 从消耗位置重新开始（TigerClaw「稳3键」语义）。
    fn track_early_commit(&mut self, session: &mut Session) {
        if !self.config.sentence.early_commit {
            session.early_streak = None;
            return;
        }
        let dec = match &self.sentence {
            Some(d) => d.clone(),
            None => return,
        };
        let proposal = dec.early_commit_proposal(&session.raw);
        match proposal {
            Some((text, consumed)) if consumed > 0 && consumed < session.raw.chars().count() => {
                let key = (text, consumed);
                let streak = match session.early_streak.take() {
                    Some((k, n)) if k == key => n + 1,
                    _ => 1,
                };
                if streak >= 3 {
                    let (text, consumed) = key;
                    let rest: String = session.raw.chars().skip(consumed).collect();
                    session.raw = rest;
                    session.early_streak = None;
                    if session.raw.is_empty() {
                        session.candidates.clear();
                        session.pending_commit = Some(text);
                    } else {
                        // 剩余编码重新走常规候选
                        self.refresh_candidates_inner(session);
                        session.pending_commit = Some(text);
                    }
                } else {
                    session.early_streak = Some((key, streak));
                }
            }
            _ => session.early_streak = None,
        }
    }

    /// refresh_candidates 的无早屏递归体。
    fn refresh_candidates_inner(&mut self, session: &mut Session) {
        let entries = self.schema.candidates(&session.raw);
        if !entries.is_empty() {
            session.candidates = entries.iter().map(|e| self.entry_to_candidate(e)).collect();
            return;
        }
        let map = self.schema.symbols.merge_code_map();
        if let Some(list) = map.get(&session.raw) {
            session.candidates = list
                .iter()
                .map(|s| {
                    let mut c =
                        Candidate::new(s.text.clone(), s.code.clone(), CandidateKind::Symbol);
                    c.weight = s.weight;
                    c
                })
                .collect();
        }
    }

    fn take_or_state(&mut self, session: &mut Session) -> KeyOutcome {
        if let Some(text) = session.pending_commit.take() {
            KeyOutcome::commit(text, self.state(session))
        } else {
            KeyOutcome::consumed(self.state(session))
        }
    }

    /// 空格首选 / 空码处理。
    fn select_first(&mut self, session: &mut Session) -> KeyOutcome {
        if !session.candidates.is_empty() {
            return self.select_candidate(session, 0);
        }
        // 空码：大写混合输入直接上屏原串，否则清屏
        if session.raw.chars().any(|c| c.is_ascii_uppercase()) {
            let raw = std::mem::take(&mut session.raw);
            session.clear();
            return KeyOutcome::commit(raw, self.state(session));
        }
        session.clear();
        KeyOutcome::consumed(self.state(session))
    }

    /// 选第 idx 个候选（当前页内，0 起）。
    fn select_candidate(&mut self, session: &mut Session, idx: usize) -> KeyOutcome {
        let page_size = self.config.candidates.page_size.max(1);
        let start = session.page * page_size;
        let pick = session.candidates.get(start + idx).cloned();
        if let Some(cand) = pick {
            self.learn(&cand);
            let text = cand.commit_text().to_string();
            session.clear();
            return KeyOutcome::commit(text, self.state(session));
        }
        KeyOutcome::consumed(self.state(session))
    }

    fn on_page(&mut self, session: &mut Session, dir: i32) -> KeyOutcome {
        let page_size = self.config.candidates.page_size.max(1);
        let pages = (session.candidates.len() + page_size - 1) / page_size;
        if pages <= 1 {
            return KeyOutcome::consumed(self.state(session));
        }
        let cur = session.page as i32;
        let next = if dir < 0 {
            if cur == 0 {
                pages - 1
            } else {
                (cur - 1) as usize
            }
        } else {
            ((cur + 1) as usize) % pages
        };
        session.page = next;
        KeyOutcome::consumed(self.state(session))
    }

    /// 反查模式按键。
    fn on_reverse_char(&mut self, session: &mut Session, c: char) -> KeyOutcome {
        if session.raw.is_empty() {
            if c.is_ascii_lowercase() || c == '\'' {
                session.raw.push(c);
                self.refresh_candidates(session);
                return KeyOutcome::consumed(self.state(session));
            }
            if c == ' ' || c == self.config.reverse.prefix {
                return KeyOutcome::consumed(self.state(session));
            }
            session.mode = InputMode::Normal;
            return KeyOutcome::consumed(self.state(session));
        }
        match c {
            ' ' => self.select_first(session),
            _ if c.is_ascii_lowercase() || c == '\'' => {
                session.raw.push(c);
                self.refresh_candidates(session);
                KeyOutcome::consumed(self.state(session))
            }
            _ => {
                session.clear();
                KeyOutcome::consumed(self.state(session))
            }
        }
    }

    /// `\` 命令模式：动态变量与工具命令。
    fn on_command_char(&mut self, session: &mut Session, c: char) -> KeyOutcome {
        if c == ' ' || c == '\\' {
            return self.select_first(session);
        }
        if c.is_ascii_lowercase() {
            session.raw.push(c);
            self.refresh_candidates(session);
            return KeyOutcome::consumed(self.state(session));
        }
        session.clear();
        KeyOutcome::consumed(self.state(session))
    }

    /// 用户学习：自动调频。
    fn learn(&mut self, cand: &Candidate) {
        if self.config.user.auto_frequency {
            self.schema.user_dict.add_word(&cand.code, &cand.text);
        }
    }

    /// 重建候选列表（含整句模式切换）。
    fn refresh_candidates(&mut self, session: &mut Session) {
        session.candidates.clear();
        session.page = 0;
        if session.raw.is_empty() {
            return;
        }
        // 命令模式：动态变量候选
        if session.mode == InputMode::Command {
            session.candidates = self.command_candidates(&session.raw);
            return;
        }
        // 反查模式
        if session.mode == InputMode::Reverse {
            session.candidates = self.reverse_candidates(&session.raw);
            return;
        }

        let raw_len = session.raw.chars().count();
        let sentence_mode =
            self.sentence_active() && raw_len > self.config.input.max_code_length;
        if sentence_mode {
            if let Some(dec) = &self.sentence {
                session.candidates = dec.decode(&session.raw);
                self.track_early_commit(session);
                return;
            }
        }

        // 常规：精确码候选
        let entries = self.schema.candidates(&session.raw);
        if !entries.is_empty() {
            session.candidates = entries.iter().map(|e| self.entry_to_candidate(e)).collect();
            return;
        }
        // 符号表回退
        let map = self.schema.symbols.merge_code_map();
        if let Some(list) = map.get(&session.raw) {
            session.candidates = list
                .iter()
                .map(|s| {
                    let mut c =
                        Candidate::new(s.text.clone(), s.code.clone(), CandidateKind::Symbol);
                    c.weight = s.weight;
                    c
                })
                .collect();
        }
        // 空态标点首选：/ → 顿号、; → 全角分号、' → 左单引号
        if session.candidates.is_empty() {
            let fallback = match session.raw.as_str() {
                "/" if self.config.input.slash_dunhao => Some("、"),
                ";" => Some("；"),
                "'" => Some("‘"),
                _ => None,
            };
            if let Some(t) = fallback {
                session
                    .candidates
                    .push(Candidate::new(t, session.raw.clone(), CandidateKind::Symbol));
            }
        }
    }

    /// 命令命名空间候选（\date \time \week \calc …）。
    fn command_candidates(&self, raw: &str) -> Vec<Candidate> {
        let commands: &[(&str, &str)] = &[
            ("date", "{日期}"),
            ("time", "{时分}"),
            ("week", "{星期}"),
            ("calc", "{计算器}"),
            ("n", "{数字转中文}"),
            ("addword", "{加词}"),
            ("export", "{导出码表}"),
        ];
        let name = raw.trim_start_matches('\\');
        commands
            .iter()
            .filter(|(k, _)| k.starts_with(name))
            .map(|(k, v)| Candidate::new(*v, format!("\\{k}"), CandidateKind::Command))
            .collect()
    }

    /// 反查候选（反查表 + 主码注释）。
    fn reverse_candidates(&self, raw: &str) -> Vec<Candidate> {
        let Some(rev) = &self.schema.reverse else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for word in rev.lookup(raw).iter().take(20) {
            let mut c = Candidate::new(word.clone(), raw.to_string(), CandidateKind::Reverse);
            if let Some(code) = self.schema.best_code_of(word) {
                c.comment = code;
            }
            out.push(c);
        }
        out
    }

    /// DictEntry → Candidate（含注释）。
    fn entry_to_candidate(&self, e: &DictEntry) -> Candidate {
        let mut c = Candidate::new(e.text.clone(), e.code.clone(), CandidateKind::Dict);
        c.weight = e.weight;
        c.pinned = e.pinned;
        c.commit_override = e.commit_override.clone();
        c.comment = self.annotate(&e.text);
        c
    }

    /// 注释：拆分 / 拼音 / Unicode 分区（按配置与可用性）。
    pub fn annotate(&self, word: &str) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.config.candidates.show_split {
            if let Some(sp) = &self.schema.split {
                let s = sp.annotate_word(word, 2);
                if !s.is_empty() {
                    parts.push(s);
                }
            }
        } else if self.config.candidates.show_comment {
            if let Some(py) = &self.schema.pinyin {
                let s = py.annotate_word(word, 2);
                if !s.is_empty() {
                    parts.push(s);
                }
            }
        }
        if let Some(uni) = &self.schema.unicode_block {
            if let Some(fc) = word.chars().next() {
                if let Some(u) = uni.get(fc) {
                    if u != "基本" {
                        parts.push(format!("[{u}]"));
                    }
                }
            }
        }
        parts.join(" ")
    }

    /// 会话状态快照。
    pub fn state(&self, session: &Session) -> SessionState {
        let page_size = self.config.candidates.page_size.max(1);
        let pages = (session.candidates.len() + page_size - 1) / page_size;
        let start = (session.page * page_size).min(session.candidates.len());
        let end = (start + page_size).min(session.candidates.len());
        let mut preedit = session.raw.clone();
        if !self.config.input.code_disguise.is_empty() && !preedit.is_empty() {
            preedit = format!("{}{}", self.config.input.code_disguise, preedit);
        }
        SessionState {
            raw: session.raw.clone(),
            preedit,
            candidates: session.candidates[start..end].to_vec(),
            page: session.page,
            page_count: pages,
            aux: match session.mode {
                InputMode::Reverse => "〔反查〕".into(),
                InputMode::Command => "〔命令〕".into(),
                _ => String::new(),
            },
            mode: session.mode,
            chinese: session.chinese,
            full_shape: self.config.punct.full_shape,
            ascii_punct: self.config.input.ascii_punct,
            reverse_mode: session.mode == InputMode::Reverse,
        }
    }
}
