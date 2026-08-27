use hufu_rerank::gguf::f16_to_f32;

#[test]
fn f16_known_values() {
    // 半精度位模式手工构造：exp/mant 展开（b0=低字节）
    let mk = |e: i32, m: i32, sign: i32| -> (u8, u8) {
        let bits: u16 = ((sign << 15) | (e << 10) | m) as u16;
        (bits as u8, (bits >> 8) as u8)
    };
    // exp=15,m=0 → 1.0；exp=16,m=0 → 2.0；exp=14 → 0.5；exp=15,m=512 → 1.5
    for (e, m, want) in [(15, 0, 1.0f32), (16, 0, 2.0), (14, 0, 0.5), (15, 512, 1.5), (24, 308, 666.0), (1, 0, 2.0e-14_f32 / 2.0e-14_f32 * (2f32).powi(-14)), (25, 0, 65536.0 / 2.0)] {
        let (b0, b1) = mk(e, m, 0);
        let got = f16_to_f32(b0, b1);
        let expect = match (e, m) {
            (1, 0) => (2f32).powi(-14),
            (25, 0) => 1024.0,
            _ => want,
        };
        assert!((got - expect).abs() < expect.abs() * 1e-3 + 1e-9, "e={e} m={m}: got {got} want {expect}");
    }
    // subnormal 最小值 0x0001 = 2^-24
    assert!((f16_to_f32(0x01, 0x00) - 5.9604645e-8).abs() < 1e-14);
    // 负数
    let (b0, b1) = mk(15, 0, 1);
    assert!((f16_to_f32(b0, b1) + 1.0).abs() < 1e-6);
}
