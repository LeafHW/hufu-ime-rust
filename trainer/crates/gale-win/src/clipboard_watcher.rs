//! 群文监听（剪贴板轮询，蓝本：疾风1代 watcher.rs —— 每 700ms 检查剪贴板，
//! 出现「-----第N段」格式赛文自动载入）。QQ 窗口直读通道在 M6 评估（QQEnhancement 方案），
//! 剪贴板通道对所有 QQ 版本天然兼容。

use gale_core::segment::parse_group_text;
use std::sync::mpsc::Sender;
use std::time::Duration;

/// 监听器捕获的内容
#[derive(Debug, Clone)]
pub enum WatchEvent {
    /// 命中跟打码（含解析结果标题）
    GroupText {
        title: String,
        mark: String,
        raw_len: usize,
    },
    /// 剪贴板变化但不是跟打码
    Other,
}

/// 纯逻辑判定：文本是否为群文/跟打码（测试覆盖，与 Win32 剪贴板解耦）
pub fn classify_clipboard_text(text: &str) -> Option<WatchEvent> {
    let parsed = parse_group_text(text);
    if let Some(seg) = parsed.segments.first() {
        // 有段号解析成功才算群文
        if !parsed.segments.is_empty() && text.contains("-----") {
            return Some(WatchEvent::GroupText {
                title: parsed.title.clone(),
                mark: seg.mark.clone(),
                raw_len: text.chars().count(),
            });
        }
    }
    Some(WatchEvent::Other)
}

/// 轮询器（Win32 剪贴板读取由平台桥注入；这里只做节流与去重）
pub struct Watcher<S: FnMut() -> Option<String>> {
    pub interval: Duration,
    pub read_clipboard: S,
    last_len: usize,
    last_hash: u64,
}

fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

impl<S: FnMut() -> Option<String>> Watcher<S> {
    pub fn new(read_clipboard: S) -> Self {
        Self {
            interval: Duration::from_millis(700),
            read_clipboard,
            last_len: 0,
            last_hash: 0,
        }
    }

    /// 单步轮询（由后台线程按 interval 调用）；内容未变返回 None
    pub fn poll_once(&mut self) -> Option<WatchEvent> {
        let text = (self.read_clipboard)();
        let text = text?;
        let h = fnv1a(&text);
        if h == self.last_hash {
            return None; // 内容没变（同长度同哈希）
        }
        self.last_hash = h;
        self.last_len = text.chars().count();
        classify_clipboard_text(&text)
    }
}

/// 后台线程主循环（发事件到 UI 线程 channel）
pub fn spawn_loop<S>(mut read_clipboard: S, tx: Sender<WatchEvent>)
where
    S: FnMut() -> Option<String> + Send + 'static,
{
    std::thread::spawn(move || {
        let mut w = Watcher::new(&mut read_clipboard);
        loop {
            if let Some(ev) = w.poll_once() {
                if tx.send(ev).is_err() {
                    break; // UI 端关闭
                }
            }
            std::thread::sleep(w.interval);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_group_text() {
        let ev = classify_clipboard_text("-----第1段-----\n今天天气很好。").unwrap();
        match ev {
            WatchEvent::GroupText { mark, raw_len, .. } => {
                assert_eq!(mark, "1");
                assert!(raw_len > 0);
            }
            _ => panic!("应是群文"),
        }
    }

    #[test]
    fn classify_other() {
        assert!(matches!(
            classify_clipboard_text("随便复制的文本"),
            Some(WatchEvent::Other)
        ));
        assert!(matches!(
            classify_clipboard_text(""),
            Some(WatchEvent::Other)
        ));
    }

    #[test]
    fn dedup_same_content() {
        let count = std::cell::Cell::new(0);
        let mut w = Watcher::new(|| {
            count.set(count.get() + 1);
            if count.get() <= 2 {
                Some("-----第1段-----\n甲".into())
            } else {
                None
            }
        });
        assert!(w.poll_once().is_some());
        assert!(w.poll_once().is_none(), "相同内容不应重复触发");
    }
}
