//! E2E 段链路验证：parse_group_text → Comparer 完整输入 → is_done。
//! （e2e-ime.ps1 无头注入通道的纯逻辑对照——段解析与比对推进必须先在
//! 确定性环境证明正确，RichEdit 注入层的失败才可归因到窗口层。）

#[test]
fn e2e_segment_parse_and_complete() {
    let raw = "-----第1段-----\n天青色等烟雨 而我在等你";
    let p = gale_core::segment::parse_group_text(raw);
    assert_eq!(p.segments.len(), 1, "segments: {:?}", p.segments);
    let seg = &p.segments[0];
    assert_eq!(seg.mark, "1");
    let text = seg.text.clone();
    eprintln!("seg text = {:?} ({} chars)", text, text.chars().count());
    // 模拟完整输入（WM_CHAR 逐字符注入的纯逻辑等价）
    let mut c = gale_core::compare::Comparer::new(&text);
    for ch in "天青色等烟雨 而我在等你".chars() {
        c.feed(
            &gale_core::compare::InputEvent::Commit {
                chars: ch.to_string(),
            },
            100,
        );
    }
    eprintln!("pos={} wrong={} done={}", c.pos, c.wrong, c.is_done());
    assert!(c.is_done());
    assert_eq!(c.wrong, 0);
}
