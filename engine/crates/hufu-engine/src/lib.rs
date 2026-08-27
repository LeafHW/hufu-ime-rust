//! hufu-engine —— 平台无关的输入法会话引擎。
//!
//! 状态机：按键 →（切换键 / 标点 / 反查 / 命令 / 编码追加与顶功 /
//! 候选生成 / 选重翻页）→ KeyOutcome。

pub mod dynamic;
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
    /// 单次按键内的提示音标签提示（select/page 覆盖默认 key/commit）
    sound_hint: Option<&'static str>,
    /// OpenCC 转换表（opencc.enabled 时懒加载）
    opencc: Option<hufu_dict::OpenCc>,
    opencc_emoji: Option<hufu_dict::OpenCc>,
    opencc_loaded: bool,
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
            sound_hint: None,
            opencc: None,
            opencc_emoji: None,
            opencc_loaded: false,
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
            sound_hint: None,
            opencc: None,
            opencc_emoji: None,
            opencc_loaded: false,
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
        self.sound_hint = None;
        let mut out = self.process_key_inner(session, key);
        // 提示音标签（前端按数据目录 sounds/<tag>.wav 播放）
        if self.config.sound.enabled && out.consumed {
            let tag = self.sound_hint.unwrap_or(if out.commit.is_some() {
                "commit"
            } else {
                "key"
            });
            out.sound = Some(tag.to_string());
        }
        out
    }

    fn process_key_inner(&mut self, session: &mut Session, key: KeyInput) -> KeyOutcome {
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
                // Ctrl+Shift+数字：置顶当前页第 N 候选
                if m.shift {
                    if let Some(n) = c.to_digit(10) {
                        let idx = if n == 0 { 9 } else { (n - 1) as usize };
                        return self.op_pin_candidate(session, idx);
                    }
                }
            }
            // Ctrl+Delete：软删当前页首选
            if key.key == KeyCode::Delete && self.config.user.allow_delete_word {
                return self.op_hide_candidate(session, 0);
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

        // 整句方案：超过最大码长后由整句解码器接管（不顶功、不清屏）；
        // 整句模式下死路同样不顶屏——编码留在缓冲区交给解码器组句
        let sentence_mode = self.sentence_active();
        let sentence_takeover = sentence_mode && len > max_len;

        // 顶功：超长（第 max+1 码）或死路（新码无任何延续）
        let dead_end = session.candidates.is_empty() && !self.has_continuation(&raw);
        let over_length = len > max_len;
        if (over_length || dead_end) && !sentence_mode && self.config.input.auto_push && !has_upper
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

        // 空码自动清屏（既无精确也无前缀，且未开启顶功短路；整句模式保留缓冲）
        if dead_end && !sentence_mode && self.config.input.auto_clear_empty && !has_upper {
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
            self.sound_hint = Some("select");
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
        self.sound_hint = Some("page");
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
            // 数字选重 / 翻页（与普通模式一致；反查候选多时翻页查看）
            _ if c.is_ascii_digit() && c != '0' => {
                let page_size = self.config.candidates.page_size.max(1);
                let start = session.page * page_size;
                let pick = session.candidates.get(start + (c as usize - '1' as usize)).cloned();
                match pick {
                    Some(cand) => {
                        let text = cand.commit_text().to_string();
                        session.clear();
                        KeyOutcome::commit(text, self.state(session))
                    }
                    None => KeyOutcome::consumed(self.state(session)),
                }
            }
            _ if self.config.candidates.paging_keys.contains(c) => {
                let dir = if self.config.candidates.paging_keys.find(c)
                    >= Some(self.config.candidates.paging_keys.len() / 2)
                {
                    1
                } else {
                    -1
                };
                self.on_page(session, dir)
            }
            _ => {
                session.clear();
                KeyOutcome::consumed(self.state(session))
            }
        }
    }

    /// `\` 命令模式：动态变量（含 \n数字 → 中文）与工具命令。
    fn on_command_char(&mut self, session: &mut Session, c: char) -> KeyOutcome {
        if c == ' ' || c == '\\' {
            return self.select_first(session);
        }
        // 命令空间收任意非空白字符（calc 表达式符号 / \w 造词中文）；
        // 上限 18 字符
        if !c.is_whitespace() && session.raw.chars().count() < 18 {
            session.raw.push(c);
            self.refresh_candidates(session);
            return KeyOutcome::consumed(self.state(session));
        }
        session.clear();
        KeyOutcome::consumed(self.state(session))
    }

    /// 置顶当前页第 idx 候选（Ctrl+Shift+N / 设置界面）。持久化到用户调整日志。
    pub fn op_pin_candidate(&mut self, session: &mut Session, idx: usize) -> KeyOutcome {
        let page_size = self.config.candidates.page_size.max(1);
        let start = session.page * page_size;
        let pick = session.candidates.get(start + idx).cloned();
        let Some(cand) = pick else {
            return KeyOutcome::consumed(self.state(session));
        };
        self.adjust_pin(&cand.code, &cand.text);
        let keep_raw = session.raw.clone();
        session.clear();
        session.raw = keep_raw;
        self.refresh_candidates(session);
        KeyOutcome::consumed(self.state(session))
    }

    /// 软删当前页第 idx 候选（Ctrl+Delete / 设置界面）。
    pub fn op_hide_candidate(&mut self, session: &mut Session, idx: usize) -> KeyOutcome {
        let page_size = self.config.candidates.page_size.max(1);
        let start = session.page * page_size;
        let pick = session.candidates.get(start + idx).cloned();
        let Some(cand) = pick else {
            return KeyOutcome::consumed(self.state(session));
        };
        self.adjust_hide(&cand.code, &cand.text);
        let keep_raw = session.raw.clone();
        session.clear();
        session.raw = keep_raw;
        self.refresh_candidates(session);
        KeyOutcome::consumed(self.state(session))
    }

    /// 按 code+word 置顶（内存 + 追加日志）。
    pub fn adjust_pin(&mut self, code: &str, word: &str) {
        self.schema.adjust.pin(code, word);
        self.append_adjust_log("{置顶}", code, word);
    }

    /// 按 code+word 软删（内存 + 追加日志）。
    pub fn adjust_hide(&mut self, code: &str, word: &str) {
        self.schema.adjust.remove(code, word);
        self.append_adjust_log("{删除}", code, word);
    }

    /// 追加一行到 用户调整.txt。
    fn append_adjust_log(&self, op: &str, code: &str, word: &str) {
        use std::io::Write;
        let path = self.schema.dir.join("用户调整.txt");
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{op}{code}\t{word}");
        }
    }

    /// OpenCC 滤镜：为前若干候选追加繁体/emoji 变体（Rime simplifier 语义）。
    fn apply_opencc(&mut self, session: &mut Session) {
        let cfg = self.config.opencc.clone();
        if !cfg.enabled {
            return;
        }
        if !self.opencc_loaded {
            let dir = self.data_dir.join("opencc");
            // 本数据集只有台版单字表 STCharacters_Tu（无标准 STCharacters，缺文件自动跳过）
            let t = if cfg.to_traditional {
                hufu_dict::OpenCc::load_dir(
                    &dir,
                    &["STPhrases", "STCharacters", "STCharacters_Tu"],
                )
            } else {
                hufu_dict::OpenCc::load_dir(&dir, &["TSPhrases", "TSCharacters"])
            };
            self.opencc = if t.is_empty() { None } else { Some(t) };
            let em = hufu_dict::OpenCc::load_dir_full(&dir, &["emoji"]);
            self.opencc_emoji = if em.is_empty() { None } else { Some(em) };
            self.opencc_loaded = true;
        }
        let base: Vec<Candidate> = session.candidates.iter().take(3).cloned().collect();
        for cand in &base {
            if cfg.to_traditional {
                if let Some(t) = &self.opencc {
                    let conv = t.convert(&cand.text);
                    if conv != cand.text {
                        let mut c = cand.clone();
                        c.text = conv;
                        c.comment = "⚑繁".into();
                        session.candidates.push(c);
                    }
                }
            }
            if cfg.emoji {
                if let Some(em) = &self.opencc_emoji {
                    let v = em.convert(&cand.text);
                    if v != cand.text && v.chars().count() > cand.text.chars().count() {
                        let mut c = cand.clone();
                        c.text = v;
                        c.comment = "😊".into();
                        session.candidates.push(c);
                    }
                }
            }
        }
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
            self.apply_opencc(session);
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

    /// 命令命名空间候选：动态变量（真实值）与工具命令。
    fn command_candidates(&self, raw: &str) -> Vec<Candidate> {
        let name = raw.trim_start_matches('\\');
        let mut out = Vec::new();

        // \n<数字> → 中文数字（小写）；\N<数字> → 大写
        if let Some(num) = name.strip_prefix('n').or_else(|| name.strip_prefix('N')) {
            if let Some(cn) = dynamic::number_to_chinese(num, name.starts_with('N')) {
                out.push(Candidate::new(
                    cn,
                    format!("\\{name}"),
                    CandidateKind::Command,
                ));
            }
        }

        let commands: Vec<(&str, String)> = vec![
            ("date", dynamic::date_string()),
            ("date2", dynamic::date_string_iso()),
            ("time", dynamic::time_string()),
            ("time2", dynamic::time_short()),
            ("week", dynamic::week_string()),
        ];
        for (k, v) in &commands {
            if k.starts_with(name) {
                out.push(Candidate::new(v.clone(), format!("\\{k}"), CandidateKind::Command));
            }
        }

        // \calc<表达式> → 实时求值
        if let Some(expr) = name.strip_prefix("calc") {
            if expr.is_empty() {
                out.push(Candidate::new(
                    "＝计算器：\\calc(1+2)*3".to_string(),
                    "\\calc".to_string(),
                    CandidateKind::Command,
                ));
            } else if let Some(v) = dynamic::calc(expr) {
                let shown = format!("＝{}", dynamic::fmt_num(v));
                let mut c = Candidate::new(shown, format!("\\calc{expr}"), CandidateKind::Command);
                c.commit_override = Some(dynamic::fmt_num(v));
                out.push(c);
            } else {
                out.push(Candidate::new(
                    "＝表达式无效".to_string(),
                    format!("\\calc{expr}"),
                    CandidateKind::Command,
                ));
            }
        } else if name == "c" {
            // calc 前缀提示
            out.push(Candidate::new(
                "＝计算器：\\calc(1+2)*3".to_string(),
                "\\calc".to_string(),
                CandidateKind::Command,
            ));
        }

        // \w<词> → Rime encoder 规则造词（构码 + 注释显示编码）
        if let Some(word) = name.strip_prefix('w') {
            if !word.is_empty() {
                if let Some(code) = self.encode_word(word) {
                    let mut c =
                        Candidate::new(word.to_string(), format!("\\w{word}"), CandidateKind::Command);
                    c.comment = code.clone();
                    c.commit_override = Some(word.to_string());
                    // 直接给候选码，选词时 learn() 自动入用户词库
                    c.code = code;
                    out.push(c);
                } else {
                    out.push(Candidate::new(
                        "造词失败：字无编码或无匹配规则".to_string(),
                        format!("\\w{word}"),
                        CandidateKind::Command,
                    ));
                }
            }
        }
        out
    }

    /// Rime encoder 造词：formula 形如 `AaAbBaBb`（大写=第几个字，Z=末字；
    /// 小写=该字第几码）。返回第一个完全可构的规则结果。
    fn encode_word(&self, word: &str) -> Option<String> {
        let chars: Vec<char> = word.chars().collect();
        if self.schema.encoder_rules.is_empty() || chars.is_empty() {
            return None;
        }
        // 每字首选码
        let codes: Vec<Option<String>> = chars
            .iter()
            .map(|c| self.schema.best_code_of(&c.to_string()))
            .collect();
        for rule in &self.schema.encoder_rules {
            if chars.len() < rule.min_len || chars.len() > rule.max_len {
                continue;
            }
            let mut code = String::new();
            let mut ok = true;
            let f: Vec<char> = rule.formula.chars().collect();
            let mut i = 0;
            while i + 1 < f.len() {
                let (up, low) = (f[i], f[i + 1]);
                let idx = if up == 'Z' || up == 'z' {
                    chars.len() - 1
                } else {
                    (up as u8 - b'A') as usize
                };
                let code_pos = (low as u8 - b'a') as usize;
                match codes.get(idx).and_then(|c| c.as_ref()) {
                    Some(cc) => match cc.chars().nth(code_pos) {
                        Some(ch) => code.push(ch),
                        None => {
                            ok = false;
                            break;
                        }
                    },
                    None => {
                        ok = false;
                        break;
                    }
                }
                i += 2;
            }
            if ok && !code.is_empty() {
                return Some(code);
            }
        }
        None
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

#[cfg(test)]
mod tests {
    use super::*;
    use hufu_types::Modifiers;

    fn key(c: char) -> KeyInput {
        KeyInput {
            key: KeyCode::Char(c),
            modifiers: Modifiers::default(),
            is_press: true,
        }
    }

    fn ctrl_shift(c: char) -> KeyInput {
        KeyInput {
            key: KeyCode::Char(c),
            modifiers: Modifiers {
                ctrl: true,
                shift: true,
                ..Default::default()
            },
            is_press: true,
        }
    }

    fn test_engine(tag: &str) -> (Engine, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("hufu-eng-dyn-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("main.txt"),
            "#hufu-dict v1 name=t\na\t啊\naa\t阿\njd\t就\njd\t到的\njd\t加\n",
        )
        .unwrap();
        let cfg = hufu_config::Config::default();
        let eng = Engine::with_schema_dir(&dir, cfg).unwrap();
        (eng, dir)
    }

    #[test]
    fn dynamic_date_week() {
        let (mut eng, _dir) = test_engine("date");
        let mut s = Session::new(true);
        eng.process_key(&mut s, key('\\'));
        eng.process_key(&mut s, key('d'));
        let _ = eng.process_key(&mut s, key('a'));
        let snap = eng.state(&s); let texts: Vec<String> = snap.candidates.iter().map(|c| c.text.clone()).collect();
        assert!(texts.iter().any(|t| t.contains('年') && t.contains('月')), "{texts:?}");
        // 星期
        let (mut eng2, _d2) = test_engine("week");
        let mut s2 = Session::new(true);
        eng2.process_key(&mut s2, key('\\'));
        eng2.process_key(&mut s2, key('w'));
        let _ = eng2.process_key(&mut s2, key('e'));
        let snap = eng2.state(&s2);
        let texts: Vec<String> = snap.candidates.iter().map(|c| c.text.clone()).collect();
        assert!(texts.iter().any(|t| t.starts_with("星期")), "{texts:?}");
    }

    #[test]
    fn dynamic_number() {
        let (mut eng, _dir) = test_engine("num");
        let mut s = Session::new(true);
        eng.process_key(&mut s, key('\\'));
        for c in "n12345".chars() {
            eng.process_key(&mut s, key(c));
        }
        let snap = eng.state(&s); let texts: Vec<String> = snap.candidates.iter().map(|c| c.text.clone()).collect();
        assert!(texts.iter().any(|t| t == &"一万二千三百四十五".to_string()), "{texts:?}");
        // 上屏
        let out = eng.process_key(&mut s, key(' '));
        assert_eq!(out.commit.unwrap(), "一万二千三百四十五");

        // 大写
        let (mut eng2, _d2) = test_engine("num2");
        let mut s2 = Session::new(true);
        eng2.process_key(&mut s2, key('\\'));
        for c in "N1234".chars() {
            eng2.process_key(&mut s2, key(c));
        }
        let snap = eng2.state(&s2); let texts: Vec<String> = snap.candidates.iter().map(|c| c.text.clone()).collect();
        assert!(texts.iter().any(|t| t == &"壹仟贰佰叁拾肆".to_string()), "{texts:?}");
    }

    #[test]
    fn pin_and_hide_via_keys() {
        let (mut eng, _dir) = test_engine("pin");
        let mut s = Session::new(true);
        // jd → 就/到的/加；Ctrl+Shift+2 置顶第 2 个（到的）
        eng.process_key(&mut s, key('j'));
        eng.process_key(&mut s, key('d'));
        let st = eng.state(&s);
        assert_eq!(st.candidates[0].text, "就");

        let _ = eng.process_key(&mut s, ctrl_shift('2'));
        let st = eng.state(&s);
        assert_eq!(st.candidates[0].text, "到的", "置顶后『到的』应在首位");

        // 日志落盘 + 回放等价
        let dir = eng.schema.dir.clone();
        let log = std::fs::read_to_string(dir.join("用户调整.txt")).unwrap();
        assert!(log.contains("{置顶}jd\t到的"), "{log}");
        let adj = hufu_dict::user::UserAdjust::parse(
            &log.lines().map(|l| l.to_string()).collect::<Vec<_>>(),
        );
        let base = eng.schema.dict.lookup("jd").into_iter().cloned().collect::<Vec<_>>();
        let out = adj.apply("jd", &base);
        assert_eq!(out[0].text, "到的");

        // Ctrl+Delete 软删首选（到的）→ 首选回到 就
        let mut s3 = Session::new(true);
        eng.process_key(&mut s3, key('j'));
        eng.process_key(&mut s3, key('d'));
        let del = KeyInput {
            key: KeyCode::Delete,
            modifiers: Modifiers {
                ctrl: true,
                ..Default::default()
            },
            is_press: true,
        };
        let _ = eng.process_key(&mut s3, del);
        let st = eng.state(&s3);
        assert_eq!(st.candidates[0].text, "就", "软删后『就』应回到首位");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn calc_command() {
        let (mut eng, dir) = test_engine("calc");
        let mut s = Session::new(true);
        eng.process_key(&mut s, key('\\'));
        for c in "calc(1+2)*3".chars() {
            eng.process_key(&mut s, key(c));
        }
        let snap = eng.state(&s);
        assert!(snap.candidates.iter().any(|c| c.text.contains('9')), "{:?}",
            snap.candidates.iter().map(|c| c.text.clone()).collect::<Vec<_>>());
        // 上屏是纯数值
        let o = eng.process_key(&mut s, key(' '));
        assert_eq!(o.commit.unwrap(), "9");
        // 无效表达式
        let mut s2 = Session::new(true);
        eng.process_key(&mut s2, key('\\'));
        for c in "calc1+".chars() {
            eng.process_key(&mut s2, key(c));
        }
        let snap = eng.state(&s2);
        assert!(snap.candidates.iter().any(|c| c.text.contains("无效")), "提示无效");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn word_making_encoder() {
        // Rime encoder：length_equal 2 → formula AaBa（两字词=各取首码）
        let dir = std::env::temp_dir().join(format!("hufu-eng-wm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("tiger.dict.yaml"),
            "---\nname: t\nsort: by_weight\nencoder:\n  rules:\n    - length_equal: 2\n      formula: AaBa\n...\n就\tjd\n不\tbh\n",
        )
        .unwrap();
        let cfg = hufu_config::Config::default();
        let mut eng = Engine::with_schema_dir(&dir, cfg).unwrap();
        let mut s = Session::new(true);
        eng.process_key(&mut s, key('\\'));
        for c in "w就就".chars() {
            eng.process_key(&mut s, key(c));
        }
        let snap = eng.state(&s);
        let cand = snap.candidates.iter().find(|c| c.commit_override.is_some());
        assert!(cand.is_some(), "应有造词候选");
        let c = cand.unwrap();
        assert_eq!(c.comment, "jj", "两字各取首码: {}", c.comment);
        // 选中 → 上屏词、编码=构码（learn 入库）
        let o = eng.process_key(&mut s, key(' '));
        assert_eq!(o.commit.unwrap(), "就就");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn opencc_variants() {
        // fixture：ST 词组/单字 + emoji 表
        let dir = std::env::temp_dir().join(format!("hufu-eng-oc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let schema = dir.join("schema");
        std::fs::create_dir_all(&schema).unwrap();
        std::fs::create_dir_all(dir.join("opencc")).unwrap();
        std::fs::write(schema.join("main.txt"), "#hufu-dict v1 name=t\nh\t后\nhq\t后来\n").unwrap();
        std::fs::write(dir.join("opencc").join("STPhrases.txt"), "后来\t後來\n").unwrap();
        std::fs::write(dir.join("opencc").join("STCharacters.txt"), "后\t後\n来\t來\n").unwrap();
        std::fs::write(dir.join("opencc").join("emoji.txt"), "后\t后 👑\n").unwrap();
        let mut cfg = hufu_config::Config::default();
        cfg.opencc.enabled = true;
        cfg.opencc.to_traditional = true;
        let mut eng = Engine::with_schema_dir(&schema, cfg).unwrap();

        // 单字：后 → 後 变体
        let mut s = Session::new(true);
        eng.process_key(&mut s, key('h'));
        let snap = eng.state(&s);
        let texts: Vec<String> = snap.candidates.iter().map(|c| c.text.clone()).collect();
        assert!(texts.contains(&"後".to_string()), "繁体变体: {texts:?}");

        // 词组：后来 → 後來
        let mut s2 = Session::new(true);
        eng.process_key(&mut s2, key('h'));
        eng.process_key(&mut s2, key('q'));
        let snap = eng.state(&s2);
        let texts: Vec<String> = snap.candidates.iter().map(|c| c.text.clone()).collect();
        assert!(texts.contains(&"後來".to_string()), "词组繁体: {texts:?}");

        // emoji 变体
        eng.config.opencc.emoji = true;
        eng.opencc_loaded = false; // 重载表
        let mut s3 = Session::new(true);
        eng.process_key(&mut s3, key('h'));
        let snap = eng.state(&s3);
        let texts: Vec<String> = snap.candidates.iter().map(|c| c.text.clone()).collect();
        assert!(texts.iter().any(|t| t.contains('👑')), "emoji 变体: {texts:?}");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn sound_tags() {
        let (mut eng, dir) = test_engine("snd");
        eng.config.sound.enabled = true;
        let mut s = Session::new(true);
        let o = eng.process_key(&mut s, key('a'));
        assert_eq!(o.sound.as_deref(), Some("key"));
        let o = eng.process_key(&mut s, key(' '));
        assert_eq!(o.sound.as_deref(), Some("select"), "空格首选上屏");
        let mut s2 = Session::new(true);
        eng.process_key(&mut s2, key('a'));
        let o = eng.process_key(&mut s2, key('1'));
        assert_eq!(o.sound.as_deref(), Some("select"), "数字选词");
        // 关闭 → 无标签
        eng.config.sound.enabled = false;
        let mut s4 = Session::new(true);
        let o = eng.process_key(&mut s4, key('a'));
        assert_eq!(o.sound, None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
