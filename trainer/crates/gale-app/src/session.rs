//! Per-segment round state. All durations use the caller's monotonic milliseconds.
use gale_core::compare::{normalize_newlines, Comparer, InputEvent};
use gale_core::rules::{
    clean_body, fingerprint, CleaningPreset, RoundMetadata, SessionRules, TrainingMode,
};
use gale_core::score::ScoreRecord;
use gale_core::segment::{split_smart, ParsedGroupText, Segment};
use gale_core::single::SingleSession;
use gale_core::stats::{SegmentStats, StatsEngine};
use serde::{Deserialize, Serialize};
static SNAPSHOT_IO: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    Idle,
    Typing,
    Done,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionSource {
    Group,
    Library(String),
    Single,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub title: String,
    pub source: SessionSource,
    pub segments: Vec<Segment>,
    pub index: usize,
    pub comparer: Comparer,
    pub stats: StatsEngine,
    pub phase: Phase,
    pub started_ms: Option<u64>,
    pub finished_ms: Option<u64>,
    pub preedit: String,
    pub composing: bool,
    pub recorded: bool,
    pub last_stats: Option<SegmentStats>,
    pub done_chars: u64,
    pub done_keys: u64,
    pub done_bs: u64,
    pub done_wrong: u64,
    pub done_ms: u64,
    pub rules: SessionRules,
    pub frozen_rules: Option<SessionRules>,
    pub raw_segments: Vec<Segment>,
    pub results: Vec<Option<ScoreRecord>>,
    pub cleaned: bool,
    pub pasted: bool,
    default_seg_chars: usize,
    frozen_text_hash: String,
    #[serde(default)]
    frozen_raw_text_hash: String,
    last_clock_ms: u64,
    paused_at: Option<u64>,
    paused_ms: u64,
}
impl Session {
    pub fn new(seg_chars: usize) -> Self {
        let seg = Segment {
            mark: "1".into(),
            text: String::new(),
        };
        Self {
            title: String::new(),
            source: SessionSource::Custom,
            segments: vec![seg.clone()],
            index: 0,
            comparer: Comparer::new(""),
            stats: StatsEngine::new(),
            phase: Phase::Idle,
            started_ms: None,
            finished_ms: None,
            preedit: String::new(),
            composing: false,
            recorded: false,
            last_stats: None,
            done_chars: 0,
            done_keys: 0,
            done_bs: 0,
            done_wrong: 0,
            done_ms: 0,
            rules: SessionRules::default(),
            frozen_rules: None,
            raw_segments: vec![seg],
            results: vec![None],
            cleaned: false,
            pasted: false,
            default_seg_chars: seg_chars.clamp(10, 10000),
            frozen_text_hash: String::new(),
            frozen_raw_text_hash: String::new(),
            last_clock_ms: 0,
            paused_at: None,
            paused_ms: 0,
        }
    }
    fn loaded(&mut self) {
        self.index = 0;
        self.raw_segments = self.segments.clone();
        self.normalize_working_newlines();
        self.results = vec![None; self.segments.len()];
        self.cleaned = false;
        self.recount();
        self.reset_current();
    }
    fn normalize_working_newlines(&mut self) {
        for seg in &mut self.segments {
            if seg.text.contains('\r') {
                seg.text = normalize_newlines(&seg.text).into_owned();
            }
        }
    }
    pub fn load_group(&mut self, parsed: ParsedGroupText) {
        if parsed.segments.is_empty() {
            return;
        }
        self.title = parsed.title;
        self.source = SessionSource::Group;
        self.segments = parsed.segments;
        self.loaded();
    }
    pub fn load_book(&mut self, title: &str, text: &str) {
        self.segments = split_smart(text, self.default_seg_chars);
        if self.segments.is_empty() {
            self.segments.push(Segment {
                mark: "1".into(),
                text: text.into(),
            });
        }
        self.title = title.into();
        self.source = SessionSource::Library(title.into());
        self.loaded();
    }
    pub fn load_single(&mut self, s: SingleSession) {
        self.title = format!("练单字 · {}", s.title);
        self.source = SessionSource::Single;
        self.segments = (0..s.total_groups())
            .map(|i| {
                let mut group = s.clone();
                group.group_index = i;
                group.current_segment()
            })
            .collect();
        if self.segments.is_empty() {
            self.segments.push(s.current_segment());
        }
        self.loaded();
    }
    fn reset_current(&mut self) {
        self.comparer = Comparer::new(&self.current_segment().text);
        self.comparer.quote_rules = self.rules.quotes.clone();
        self.stats = StatsEngine::new();
        self.phase = Phase::Idle;
        self.started_ms = None;
        self.finished_ms = None;
        self.preedit.clear();
        self.composing = false;
        self.recorded = false;
        self.last_stats = None;
        self.frozen_rules = None;
        self.frozen_text_hash.clear();
        self.frozen_raw_text_hash.clear();
        self.paused_at = None;
        self.paused_ms = 0;
        self.pasted = false;
        self.last_clock_ms = 0;
    }
    fn recount(&mut self) {
        let a = self.aggregate();
        self.done_chars = a.chars;
        self.done_keys = a.keys;
        self.done_bs = a.backspaces;
        self.done_wrong = a.wrong;
        self.done_ms = a.time_ms;
    }
    pub fn aggregate(&self) -> gale_core::report::RaceSummary {
        gale_core::report::RaceSummary::from_records(self.results.iter().flatten())
    }
    pub fn current_segment(&self) -> &Segment {
        self.segments.get(self.index).unwrap_or(&self.segments[0])
    }
    pub fn current_index(&self) -> usize {
        self.index
    }
    pub fn total_segments(&self) -> usize {
        self.segments.len()
    }
    pub fn jump(&mut self, idx: usize) {
        if idx >= self.segments.len() {
            return;
        }
        self.index = idx;
        self.results[idx] = None;
        self.recount();
        self.reset_current();
    }
    pub fn next_segment(&mut self) {
        if self.index + 1 < self.segments.len() {
            self.jump(self.index + 1);
        }
    }
    pub fn is_segment_done(&self) -> bool {
        self.comparer.is_done()
    }
    pub fn segment_recorded(&self) -> bool {
        self.recorded
    }
    pub fn mark_sent(&mut self) {
        self.recorded = true;
    }
    pub fn set_rules(&mut self, rules: SessionRules) -> Result<(), String> {
        if self.frozen_rules.is_some() {
            return Err("本轮规则已冻结，请先重打再修改".into());
        }
        let rules = rules.normalized();
        if self.rules != rules {
            self.results.fill(None);
            self.recount();
        }
        self.rules = rules;
        self.comparer.quote_rules = self.rules.quotes.clone();
        Ok(())
    }
    pub fn apply_cleaning(&mut self, preset: &CleaningPreset) -> Result<(), String> {
        if self.frozen_rules.is_some() {
            return Err("开打后不能修改正文，请先重打".into());
        }
        // Transform all bodies transactionally; one failing rule leaves every original intact.
        let mut updated = self.raw_segments.clone();
        let mut changed = false;
        for seg in &mut updated {
            let out = clean_body(&seg.text, preset, self.rules.mode)?;
            changed |= out.cleaned_text != seg.text;
            seg.text = out.cleaned_text;
        }
        self.segments = updated;
        self.normalize_working_newlines();
        self.cleaned = changed;
        self.results.fill(None);
        self.recount();
        self.reset_current();
        Ok(())
    }
    pub fn restore_raw(&mut self) -> Result<(), String> {
        if self.frozen_rules.is_some() {
            return Err("开打后不能还原正文，请先重打".into());
        }
        self.segments = self.raw_segments.clone();
        self.normalize_working_newlines();
        self.cleaned = false;
        self.results.fill(None);
        self.recount();
        self.reset_current();
        Ok(())
    }
    fn clock(&mut self, now: u64) -> u64 {
        self.last_clock_ms = self.last_clock_ms.max(now);
        self.last_clock_ms
    }
    fn begin(&mut self, now: u64) {
        if self.phase != Phase::Idle || self.current_segment().text.is_empty() {
            return;
        }
        self.rules = self.rules.clone().normalized();
        self.comparer.quote_rules = self.rules.quotes.clone();
        self.frozen_rules = Some(self.rules.clone());
        self.frozen_text_hash = fingerprint(self.comparer.text.as_bytes());
        self.frozen_raw_text_hash = fingerprint(self.raw_segments[self.index].text.as_bytes());
        self.phase = Phase::Typing;
        self.started_ms = Some(now);
        self.comparer.started_at = Some(now);
    }
    pub fn feed_input(&mut self, ev: InputEvent, now_ms: u64) {
        if self.phase == Phase::Done || self.paused_at.is_some() {
            return;
        }
        let now = self.clock(now_ms);
        let meaningful = matches!(&ev,InputEvent::Commit {chars} if !chars.is_empty())
            || matches!(ev, InputEvent::Backspace) && self.comparer.pos > 0;
        if !meaningful {
            return;
        }
        self.begin(now);
        // Event intervals use the same active-time axis as the headline duration.
        // This also preserves active time before blur, including the first commit.
        let active_now = now.saturating_sub(self.paused_ms);
        let out = self.comparer.feed(&ev, active_now);
        if matches!(ev, InputEvent::Commit { .. }) && !out.is_empty() {
            self.stats.on_commit(out.len(), active_now);
            self.stats.end_composition();
            self.composing = false;
            self.preedit.clear();
        }
        if self.comparer.is_done() {
            self.finished_ms = Some(now);
            self.phase = Phase::Done;
        }
    }
    pub fn on_key(&mut self, vk: u32, now_ms: u64) {
        if self.phase == Phase::Done || self.paused_at.is_some() || !StatsEngine::is_stat_key(vk) {
            return;
        }
        let now = self.clock(now_ms);
        self.begin(now);
        self.stats
            .on_key_timed(vk, self.composing, now.saturating_sub(self.paused_ms));
    }
    pub fn set_preedit(&mut self, p: String) {
        self.preedit = p;
    }
    pub fn set_composing(&mut self, c: bool) {
        if c && !self.composing {
            self.stats.begin_composition();
        }
        self.composing = c;
    }
    /// Hosts call this only for an explicitly cancelled composition, never for a normal
    /// END notification which may arrive before its commit notification.
    pub fn cancel_composition(&mut self) {
        self.stats.cancel_composition();
        self.composing = false;
        self.preedit.clear();
    }
    pub fn feed_paste(&mut self, text: &str, now_ms: u64) -> Result<(), String> {
        let rules = self.frozen_rules.as_ref().unwrap_or(&self.rules);
        if rules.mode == TrainingMode::Race || !rules.allow_paste {
            return Err("本轮禁止粘贴输入".into());
        }
        self.pasted = true;
        self.feed_input(InputEvent::Commit { chars: text.into() }, now_ms);
        Ok(())
    }
    pub fn set_paused(&mut self, paused: bool, now_ms: u64) {
        let rules = self.frozen_rules.as_ref().unwrap_or(&self.rules);
        if rules.mode == TrainingMode::Race || !rules.pause_on_blur || self.phase != Phase::Typing {
            return;
        }
        let now = self.clock(now_ms);
        if paused {
            if self.paused_at.is_none() {
                self.paused_at = Some(now);
            }
        } else if let Some(start) = self.paused_at.take() {
            self.paused_ms = self.paused_ms.saturating_add(now.saturating_sub(start));
        }
    }
    pub fn stop(&mut self) {
        self.stop_at(self.last_clock_ms);
    }
    pub fn stop_at(&mut self, now_ms: u64) {
        let now = self.clock(now_ms);
        if self.phase == Phase::Typing {
            self.finished_ms = Some(now);
            self.phase = Phase::Done;
        }
    }
    pub fn finalize_current(&mut self, now_ms: u64) -> Option<ScoreRecord> {
        let unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.finalize_current_at(now_ms, unix)
    }
    pub fn finalize_current_at(&mut self, now_ms: u64, unix_secs: u64) -> Option<ScoreRecord> {
        if self.recorded {
            return None;
        }
        let now = self.clock(now_ms);
        let elapsed = self.elapsed_ms(now);
        if elapsed == 0 || self.comparer.pos == 0 || self.stats.keys == 0 {
            return None;
        }
        self.finished_ms.get_or_insert(now);
        self.phase = Phase::Done;
        let mut st = self.stats.finalize(&self.comparer, elapsed);
        st.para_index_hint = Some(self.index);
        self.last_stats = Some(st.clone());
        let label = match self.source {
            SessionSource::Group => "群文",
            SessionSource::Library(_) => "文库",
            SessionSource::Single => "练单字",
            SessionSource::Custom => "自定义",
        };
        let mut rec = ScoreRecord::from_stats(
            &st,
            &self.title,
            label,
            &self.current_segment().mark,
            self.index,
            unix_secs,
        );
        rec.accuracy = st.accuracy_pro;
        let rules = self.frozen_rules.as_ref().unwrap_or(&self.rules);
        rec.round = Some(RoundMetadata {
            mode: rules.mode,
            text_hash: self.frozen_text_hash.clone(),
            raw_text_hash: self.frozen_raw_text_hash.clone(),
            rules_hash: rules.fingerprint(),
            quote_relaxed: self.comparer.quote_relaxed,
            cleaned: self.cleaned,
            pasted: self.pasted,
            complete: self.comparer.is_done(),
        });
        self.results[self.index] = Some(rec.clone());
        self.recorded = true;
        self.recount();
        Some(rec)
    }
    pub fn idle_hint(&self) -> bool {
        matches!(self.phase, Phase::Idle | Phase::Done) || self.paused_at.is_some()
    }
    pub fn typed_len(&self) -> usize {
        self.comparer.pos
    }
    pub fn typed_any(&self) -> bool {
        self.comparer.pos != 0
    }
    pub fn elapsed_ms(&self, now_ms: u64) -> u64 {
        let end = self.finished_ms.unwrap_or(now_ms.max(self.last_clock_ms));
        let end = self.paused_at.map_or(end, |paused| end.min(paused));
        self.started_ms
            .map(|s| end.saturating_sub(s).saturating_sub(self.paused_ms))
            .unwrap_or(0)
    }
    pub fn live_stats(&self, now_ms: u64) -> LiveStats {
        let st = self.stats.finalize(&self.comparer, self.elapsed_ms(now_ms));
        let in_para = !self.recorded;
        let chars = self.done_chars + if in_para { st.chars as u64 } else { 0 };
        let keys = self.done_keys + if in_para { st.keys as u64 } else { 0 };
        let bs = self.done_bs + if in_para { st.backspaces as u64 } else { 0 };
        let corr: u64 = self
            .results
            .iter()
            .flatten()
            .map(|r| r.corrections as u64)
            .sum::<u64>()
            + if in_para { st.corrections as u64 } else { 0 };
        let ms = self.done_ms + if in_para { st.elapsed_ms } else { 0 };
        let seconds = ms as f64 / 1000.0;
        let k = keys as f64;
        let c = chars as f64;
        let cr = corr as f64;
        LiveStats {
            total_speed: if ms > 0 { c * 60000.0 / ms as f64 } else { 0.0 },
            para_speed: st.speed,
            para_accuracy: st.accuracy_pro,
            para_keys_per_sec: st.strokes_per_sec,
            para_code_len: st.code_len,
            accuracy: if k > 0.0 && c + cr > 0.0 {
                ((k - cr - bs as f64 * 2.0) / (c + cr) * c / k * 100.0).clamp(0.0, 100.0)
            } else {
                100.0
            },
            keys_per_sec: if seconds > 0.0 { k / seconds } else { 0.0 },
            code_len: if c > 0.0 { k / c } else { 0.0 },
            done_chars: chars,
        }
    }
    /// Serialize through a bounded writer, before allocation can exceed the limit.
    pub fn snapshot_bytes_bounded(&self) -> std::io::Result<Vec<u8>> {
        use std::io::Write;
        struct Bounded(Vec<u8>);
        impl Write for Bounded {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if self.0.len().saturating_add(bytes.len()) > 8 * 1024 * 1024 {
                    return Err(std::io::Error::other("恢复快照超过 8 MiB"));
                }
                self.0.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut writer = Bounded(Vec::new());
        serde_json::to_writer(&mut writer, self).map_err(std::io::Error::other)?;
        writer.0.push(b'\n');
        Ok(writer.0)
    }
    /// Bounded journal, with a synced recoverable previous generation on rotation.
    pub fn save_snapshot(&self, path: &std::path::Path) -> std::io::Result<()> {
        let _guard = SNAPSHOT_IO
            .lock()
            .map_err(|_| std::io::Error::other("恢复记录写入锁异常"))?;
        let bytes = self.snapshot_bytes_bounded()?;
        let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if len.saturating_add(bytes.len() as u64) > 16 * 1024 * 1024 {
            gale_core::score::durable_replace(path, &bytes)
        } else {
            gale_core::score::append_json_line(path, self)
        }
    }
    fn valid_snapshot(&self) -> bool {
        let n = self.comparer.text.chars().count();
        if self.segments.is_empty()
            || self.segments.len() > 10000
            || self.index >= self.segments.len()
            || self.results.len() != self.segments.len()
            || self.raw_segments.len() != self.segments.len()
            || n > 100000
            || self.comparer.states.len() != n
            || self.comparer.pos > n
            || self.comparer.typed.len() != self.comparer.pos
            || self.comparer.text != self.segments[self.index].text
            || self.segments.iter().any(|s| s.text.contains('\r'))
        {
            return false;
        }
        let expected: Vec<char> = self.comparer.text.chars().collect();
        let mut relaxed = 0;
        for (i, state) in self.comparer.states.iter().enumerate() {
            use gale_core::compare::CharState;
            if i >= self.comparer.pos {
                if *state != CharState::Pending {
                    return false;
                }
                continue;
            }
            let typed = self.comparer.typed[i];
            let correct = self.comparer.quote_rules.equivalent(typed, expected[i]);
            if *state
                != if correct {
                    CharState::Correct
                } else {
                    CharState::Wrong
                }
            {
                return false;
            }
            relaxed += usize::from(correct && typed != expected[i]);
        }
        if relaxed != self.comparer.quote_relaxed {
            return false;
        }
        if self.comparer.wrong
            != self
                .comparer
                .states
                .iter()
                .filter(|s| **s == gale_core::compare::CharState::Wrong)
                .count()
        {
            return false;
        }
        if self
            .segments
            .iter()
            .chain(self.raw_segments.iter())
            .any(|s| s.text.chars().count() > 100000)
        {
            return false;
        }
        if self
            .started_ms
            .zip(self.finished_ms)
            .is_some_and(|(a, b)| b < a)
        {
            return false;
        }
        if self.phase == Phase::Typing && (self.started_ms.is_none() || self.frozen_rules.is_none())
        {
            return false;
        }
        if self.rules.clone().normalized() != self.rules
            || self
                .frozen_rules
                .as_ref()
                .is_some_and(|r| r.clone().normalized() != *r)
        {
            return false;
        }
        for (idx, result) in self.results.iter().enumerate() {
            if let Some(r) = result {
                if r.para_index != idx
                    || [
                        r.speed,
                        r.accuracy,
                        r.code_len,
                        r.effective_speed,
                        r.strokes_per_sec,
                        r.accuracy_pro,
                        r.jj,
                    ]
                    .iter()
                    .any(|v| !v.is_finite())
                {
                    return false;
                }
            }
        }
        if let Some(st) = &self.last_stats {
            if [
                st.speed,
                st.accuracy,
                st.accuracy_pro,
                st.code_len,
                st.effective_speed,
                st.strokes_per_sec,
                st.jj,
                st.zz_speed,
                st.zz_speed2,
                st.zz_mc,
            ]
            .iter()
            .any(|v| !v.is_finite())
            {
                return false;
            }
        }
        true
    }
    pub fn restore_snapshot(path: &std::path::Path) -> Result<Self, String> {
        use std::io::{BufRead, BufReader, Read};
        let mut last = None;
        for candidate in [path.to_path_buf(), gale_core::score::backup_path(path)] {
            let Ok(file) = std::fs::File::open(&candidate) else {
                continue;
            };
            if file.metadata().map_err(|e| e.to_string())?.len() > 16 * 1024 * 1024 {
                continue;
            }
            // Bound the reader before lines can allocate. Even a file changed during
            // the read cannot exceed this process's restore budget.
            let reader = BufReader::new(file).take(16 * 1024 * 1024 + 1);
            for line in reader.split(b'\n') {
                let line = line.map_err(|e| e.to_string())?;
                if line.len() > 8 * 1024 * 1024 {
                    continue;
                }
                if let Ok(s) = serde_json::from_slice::<Self>(&line) {
                    if s.valid_snapshot() {
                        last = Some(s);
                    }
                }
            }
            if last.is_some() {
                break;
            }
        }
        let mut s = last.ok_or("没有可恢复的完整记录")?;
        // A new process has a new monotonic epoch: resume the article, never manufacture
        // continuation time across shutdown. Finished result slots remain intact.
        if s.phase != Phase::Done {
            s.reset_current();
        }
        s.recount();
        Ok(s)
    }
}
#[derive(Debug, Clone, Copy, Default)]
pub struct LiveStats {
    /// Settled segments plus the current segment, without counting it twice.
    pub total_speed: f64,
    /// Current segment only, including its final values until the next reset.
    pub para_speed: f64,
    pub para_accuracy: f64,
    pub para_keys_per_sec: f64,
    pub para_code_len: f64,
    /// Whole-session Pro key efficiency; use para_accuracy for current-segment UI.
    pub accuracy: f64,
    /// Whole-session keystrokes / elapsed seconds.
    pub keys_per_sec: f64,
    /// Whole-session keystrokes / committed characters.
    pub code_len: f64,
    pub done_chars: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn session() -> Session {
        let mut s = Session::new(100);
        s.load_group(ParsedGroupText {
            title: "test".into(),
            difficulty: None,
            segments: vec![
                Segment {
                    mark: "1".into(),
                    text: "甲乙".into(),
                },
                Segment {
                    mark: "2".into(),
                    text: "丙丁".into(),
                },
            ],
        });
        s
    }
    fn round(s: &mut Session, start: u64, end: u64, text: &str) {
        s.on_key(0x41, start);
        s.on_key(0x42, start + 100);
        s.feed_input(InputEvent::Commit { chars: text.into() }, end);
    }
    #[test]
    fn single_drill_keeps_all_groups_and_can_advance() {
        let mut sess = Session::new(100);
        let items: Vec<String> = "一二三四五六七八九十甲乙丙丁戊己庚辛壬癸子"
            .chars()
            .map(|c| c.to_string())
            .collect();
        sess.load_single(SingleSession {
            title: "test".into(),
            group_size: 10,
            items,
            group_index: 0,
            seed: 0,
        });
        assert_eq!(sess.total_segments(), 3);
        assert_eq!(sess.current_segment().text, "一二三四五六七八九十");
        sess.next_segment();
        assert_eq!(sess.current_segment().text, "甲乙丙丁戊己庚辛壬癸");
        sess.next_segment();
        assert_eq!(sess.current_segment().text, "子");
    }

    #[test]
    fn first_physical_key_and_single_sentence_commit() {
        let mut s = session();
        round(&mut s, 100, 1100, "甲乙");
        let r = s.finalize_current_at(1100, 1234567890).unwrap();
        assert_eq!(r.time_ms, 1000);
        assert_eq!(r.chars, 2);
        assert_eq!(r.keys, 2);
        assert_eq!(r.commit_groups, 1);
        assert_eq!(s.stats.commit_attributions[0].keys, 2);
        assert_eq!(r.ts, 1234567890);
        assert_eq!(s.live_stats(1100).done_chars, 2);
        assert!(s.finalize_current(1200).is_none());
    }
    #[test]
    fn current_segment_metrics_do_not_inherit_previous_segments() {
        let mut s = session();
        round(&mut s, 100, 1100, "甲乙");
        s.finalize_current_at(1100, 1).unwrap();
        s.next_segment();

        let idle = s.live_stats(1500);
        assert_eq!(idle.para_speed, 0.);
        assert_eq!(idle.para_accuracy, 100.);
        assert_eq!(idle.para_keys_per_sec, 0.);
        assert_eq!(idle.para_code_len, 0.);
        assert_eq!(idle.total_speed, 120.);
        assert_eq!(idle.keys_per_sec, 2.);
        assert_eq!(idle.code_len, 1.);

        s.on_key(0x41, 2000);
        s.feed_input(
            InputEvent::Commit {
                chars: "丙".into()
            },
            2500,
        );
        s.on_key(0x08, 3000);
        s.feed_input(InputEvent::Backspace, 3000);
        s.on_key(0x42, 4000);
        s.feed_input(
            InputEvent::Commit {
                chars: "丙".into()
            },
            4500,
        );
        s.on_key(0x43, 5000);
        s.feed_input(
            InputEvent::Commit {
                chars: "丁".into()
            },
            6000,
        );

        let before = s.live_stats(6000);
        assert_eq!(before.para_speed, 30.);
        assert_eq!(before.para_accuracy, 50.);
        assert_eq!(before.para_keys_per_sec, 1.);
        assert_eq!(before.para_code_len, 2.);
        assert_eq!(before.total_speed, 48.);
        assert!((before.accuracy - 200. / 3.).abs() < 1e-9);
        assert_eq!(before.keys_per_sec, 1.2);
        assert_eq!(before.code_len, 1.5);

        let record = s.finalize_current_at(6000, 2).unwrap();
        let after = s.live_stats(10000);
        assert_eq!(after.para_accuracy, record.accuracy);
        assert_eq!(after.para_keys_per_sec, record.strokes_per_sec);
        assert_eq!(after.para_code_len, record.code_len);
        assert_eq!(after.total_speed, before.total_speed);
        assert_eq!(after.accuracy, before.accuracy);
        assert_eq!(after.keys_per_sec, before.keys_per_sec);
        assert_eq!(after.code_len, before.code_len);

        s.jump(1);
        let retry = s.live_stats(11000);
        assert_eq!(retry.para_speed, 0.);
        assert_eq!(retry.para_accuracy, 100.);
        assert_eq!(retry.para_keys_per_sec, 0.);
        assert_eq!(retry.para_code_len, 0.);
        assert_eq!(retry.total_speed, 120.);
        assert_eq!(retry.keys_per_sec, 2.);
        assert_eq!(retry.code_len, 1.);
    }
    #[test]
    fn current_keystroke_rate_advances_before_first_commit() {
        let mut s = session();
        round(&mut s, 100, 1100, "甲乙");
        s.finalize_current_at(1100, 1).unwrap();
        s.next_segment();
        s.on_key(0x41, 2000);
        let live = s.live_stats(4000);
        assert_eq!(s.comparer.pos, 0);
        assert_eq!(s.elapsed_ms(4000), 2000);
        assert_eq!(live.para_speed, 0.);
        assert_eq!(live.para_keys_per_sec, 0.5);
        assert_eq!(live.para_code_len, 0.);
        assert_eq!(live.total_speed, 40.);
        assert_eq!(live.keys_per_sec, 1.);
        assert_eq!(live.code_len, 1.5);
    }
    #[test]
    fn cross_segment_retries_replace_only_their_slot() {
        let mut s = session();
        round(&mut s, 100, 1100, "甲乙");
        s.finalize_current(1100).unwrap();
        s.next_segment();
        round(&mut s, 2000, 4000, "丙丁");
        s.finalize_current(4000).unwrap();
        assert_eq!(s.aggregate().chars, 4);
        assert_eq!(s.aggregate().time_ms, 3000);
        s.jump(0);
        assert_eq!(s.aggregate().chars, 2);
        round(&mut s, 5000, 7000, "甲乙");
        s.finalize_current(7000).unwrap();
        assert_eq!(s.aggregate().chars, 4);
        assert_eq!(s.aggregate().time_ms, 4000);
        assert!((s.aggregate().speed - 60.0).abs() < 1e-9);
    }
    #[test]
    fn partial_round_does_not_claim_untyped_chars() {
        let mut s = session();
        s.on_key(0x41, 100);
        s.feed_input(
            InputEvent::Commit {
                chars: "甲".into()
            },
            600,
        );
        s.stop_at(1100);
        let r = s.finalize_current(1100).unwrap();
        assert_eq!(r.chars, 1);
        assert_eq!(r.time_ms, 1000);
        assert!(!r.round.unwrap().complete);
    }
    #[test]
    fn rules_and_body_frozen_until_restart() {
        let mut s = session();
        s.on_key(0x41, 10);
        assert!(s.set_rules(SessionRules::default()).is_err());
        assert!(s.apply_cleaning(&CleaningPreset::default()).is_err());
        s.jump(0);
        let mut r = SessionRules::default();
        r.mode = TrainingMode::Race;
        s.set_rules(r).unwrap();
        assert!(!s.rules.word_hints);
        s.on_key(0x41, 100);
        s.set_paused(true, 200);
        s.feed_input(
            InputEvent::Commit {
                chars: "甲乙".into(),
            },
            1100,
        );
        assert_eq!(s.elapsed_ms(1100), 1000);
        assert!(s.feed_paste("甲", 1200).is_err());
    }
    #[test]
    fn backward_clock_never_reduces_elapsed_and_quotes_are_correct() {
        let mut s = session();
        s.segments[0].text = "“甲”".into();
        s.raw_segments = s.segments.clone();
        s.reset_current();
        s.on_key(0x41, 100);
        s.on_key(0x41, 400);
        s.feed_input(
            InputEvent::Commit {
                chars: "”甲“".into(),
            },
            300,
        );
        assert_eq!(s.elapsed_ms(1), 300);
        assert_eq!(s.comparer.wrong, 0);
        assert_eq!(s.comparer.quote_relaxed, 2);
    }
    #[test]
    fn damaged_snapshot_tail_and_invalid_positions_do_not_restore() {
        let path = std::env::temp_dir().join(format!("hufu-session-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut s = session();
        round(&mut s, 100, 1100, "甲乙");
        s.finalize_current_at(1100, 123456).unwrap();
        s.save_snapshot(&path).unwrap();
        let mut bad = s.clone();
        bad.comparer.pos = 999;
        bad.save_snapshot(&path).unwrap();
        bad = s.clone();
        bad.comparer.pos = 1;
        bad.comparer.typed.pop(); // In-range cursor with an impossible completed suffix.
        bad.save_snapshot(&path).unwrap();
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(b"{\"bad\":").unwrap();
        drop(file);
        let restored = Session::restore_snapshot(&path).unwrap();
        assert_eq!(restored.comparer.pos, 2);
        assert_eq!(restored.aggregate().chars, 2);
        std::fs::remove_file(path).ok();
    }
    #[test]
    fn practice_pause_not_counted() {
        let mut s = session();
        s.on_key(0x41, 100);
        s.set_paused(true, 200);
        s.set_paused(false, 1200);
        s.on_key(0x42, 1300);
        s.feed_input(
            InputEvent::Commit {
                chars: "甲乙".into(),
            },
            2100,
        );
        assert_eq!(s.elapsed_ms(2100), 1000);
        assert_eq!(s.comparer.timing_events[0].dt_ms, 1000);
        assert_eq!(s.stats.per_key[0].1, 200);

        let mut partial = session();
        partial.on_key(0x41, 100);
        partial.feed_input(
            InputEvent::Commit {
                chars: "甲".into()
            },
            200,
        );
        partial.set_paused(true, 300);
        partial.stop_at(1300);
        assert_eq!(partial.elapsed_ms(1500), 200);
        assert_eq!(partial.finalize_current_at(1500, 123).unwrap().time_ms, 200);
    }

    #[test]
    fn raw_newlines_survive_while_working_coordinates_and_hashes_match() {
        let mut s = Session::new(100);
        let raw = "甲\r\n\r乙  ";
        s.load_book("raw", raw);
        assert_eq!(s.raw_segments[0].text, raw);
        assert_eq!(s.current_segment().text, "甲\n\n乙  ");
        assert!(!s.cleaned);
        round(&mut s, 100, 1100, "甲\r\n\r乙  ");
        let r = s.finalize_current_at(1100, 123).unwrap();
        assert_eq!(r.chars, 6);
        assert_eq!(r.wrong, 0);
        let meta = r.round.unwrap();
        assert_eq!(meta.text_hash, fingerprint("甲\n\n乙  ".as_bytes()));
        assert_eq!(meta.raw_text_hash, fingerprint(raw.as_bytes()));
        assert_ne!(meta.text_hash, meta.raw_text_hash);
        s.jump(0);
        s.restore_raw().unwrap();
        assert_eq!(s.current_segment().text, "甲\n\n乙  ");
        assert_eq!(s.raw_segments[0].text, raw);
        assert!(s.valid_snapshot());
    }

    #[test]
    fn host_late_start_cancel_then_new_key_keeps_batches_separate() {
        let mut s = session();
        s.on_key(0x41, 100); // The key that starts composition is initially idle.
        s.set_composing(true);
        s.set_preedit("ab".into());
        s.on_key(0x42, 200);
        s.set_composing(false); // END alone never means cancellation.
        assert_eq!(s.stats.waste_codes, 0);
        s.cancel_composition(); // Stable no-commit snapshot plus explicit physical cancel.
        assert_eq!(s.stats.waste_keys, 2);
        assert_eq!(s.stats.composing_keys, 2);
        s.on_key(0x43, 300); // Delivered after Cancelled by the host.
        s.feed_input(
            InputEvent::Commit {
                chars: "甲".into()
            },
            400,
        );
        assert_eq!(s.stats.commit_attributions[0].keys, 1);
        assert_eq!(s.stats.keys, 3);
        assert_eq!(s.stats.finalize(&s.comparer, 300).backspaces, 2);
    }

    #[test]
    fn cancelling_with_backspace_does_not_charge_the_same_key_twice() {
        let mut s = session();
        s.on_key(0x41, 100);
        s.set_composing(true);
        s.on_key(0x08, 200);
        s.set_composing(false);
        s.cancel_composition();
        assert_eq!(s.stats.keys, 2);
        assert_eq!(s.stats.waste_keys, 2);
        assert_eq!(s.stats.finalize(&s.comparer, 100).backspaces, 2);
        s.on_key(0x42, 300);
        s.feed_input(
            InputEvent::Commit {
                chars: "甲".into()
            },
            400,
        );
        assert_eq!(s.stats.commit_attributions[0].keys, 1);
    }

    #[test]
    fn committed_and_no_change_backspaces_are_excluded_from_new_commit() {
        let mut s = session();
        s.on_key(0x41, 100);
        s.feed_input(
            InputEvent::Commit {
                chars: "甲".into()
            },
            200,
        );
        s.on_key(0x08, 300);
        s.feed_input(InputEvent::Backspace, 300);
        s.on_key(0x08, 400);
        s.feed_input(InputEvent::Backspace, 400); // Already empty: no body diff.
        s.on_key(0x42, 500);
        s.set_composing(true);
        s.cancel_composition();
        assert_eq!(s.stats.waste_keys, 1);
        s.on_key(0x43, 600);
        s.feed_input(
            InputEvent::Commit {
                chars: "甲".into()
            },
            700,
        );
        assert_eq!(s.stats.commit_attributions[1].keys, 1);
        assert_eq!(s.comparer.corrections, 1);
        assert_eq!(s.stats.keys, 5);
    }

    #[test]
    fn end_before_commit_and_top_code_continuation_keep_real_batches() {
        let mut s = session();
        s.on_key(0x41, 100);
        s.set_composing(true);
        s.on_key(0x42, 200);
        s.set_composing(false); // A normal END may precede its commit.
        s.feed_input(
            InputEvent::Commit {
                chars: "甲".into()
            },
            300,
        );
        assert_eq!(s.stats.waste_codes, 0);
        s.set_composing(true); // Host restores the still-active next preedit.
        s.set_preedit("c".into());
        assert!(s.composing);
        assert_eq!(s.preedit, "c");
        s.on_key(0x43, 400);
        s.set_composing(false);
        s.feed_input(
            InputEvent::Commit {
                chars: "乙".into()
            },
            500,
        );
        assert_eq!(
            s.stats
                .commit_attributions
                .iter()
                .map(|c| c.keys)
                .collect::<Vec<_>>(),
            vec![2, 1]
        );
        assert_eq!(s.stats.keys, 3);
        assert_eq!(s.stats.composing_keys, 3);
        assert!(s.preedit.is_empty());
    }

    #[test]
    fn focus_ascii_commit_uses_only_physical_keys_without_composition() {
        let mut s = Session::new(100);
        s.load_book("ASCII", "ab");
        s.on_key(0x41, 100);
        s.feed_input(InputEvent::Commit { chars: "a".into() }, 200);
        s.set_paused(true, 200); // Focus-loss flush completes before pausing the round.
        s.set_paused(false, 1200);
        s.on_key(0x42, 1300);
        s.feed_input(InputEvent::Commit { chars: "b".into() }, 1400);
        assert_eq!(s.stats.composing_keys, 0);
        assert_eq!(s.stats.waste_codes, 0);
        assert_eq!(
            s.stats
                .commit_attributions
                .iter()
                .map(|c| c.keys)
                .collect::<Vec<_>>(),
            vec![1, 1]
        );
        assert_eq!(s.elapsed_ms(1400), 300);
    }
}
