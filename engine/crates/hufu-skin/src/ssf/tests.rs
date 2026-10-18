//! 【SSF M1】单测：合成三代容器往返、编码/颜色/边距解析、变体回退、
//! 恶意样本拒绝、真实语料（env HUFU_SSF_CORPUS 指向皮肤目录时启用，
//! 本机语料：E:\DSH-KF\.refs\tigirl\皮肤 与 Downloads 的搜狗商店皮肤）。

use super::*;

// ---------- 构造工具 ----------

fn inner_bytes(files: &[(&str, &[u8])]) -> Vec<u8> {
    let table = (files.len() * 4) as u32;
    let mut offsets = Vec::new();
    let mut entries = Vec::new();
    let mut cursor = 8 + table as usize;
    for (name, content) in files {
        offsets.push(cursor as u32);
        let name16: Vec<u16> = name.encode_utf16().collect();
        let mut e = Vec::new();
        e.extend_from_slice(&((name16.len() * 2) as u32).to_le_bytes());
        for u in name16 {
            e.extend_from_slice(&u.to_le_bytes());
        }
        e.extend_from_slice(&(content.len() as u32).to_le_bytes());
        e.extend_from_slice(content);
        cursor += e.len();
        entries.extend_from_slice(&e);
    }
    let mut out = Vec::new();
    out.extend_from_slice(&(cursor as u32).to_le_bytes()); // 自尺寸（u32！）
    out.extend_from_slice(&table.to_le_bytes());
    for o in offsets {
        out.extend_from_slice(&o.to_le_bytes());
    }
    out.extend_from_slice(&entries);
    out
}

fn zlib(data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

fn deflate(data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

const V3_KEY: [u8; 32] = [
    0x52, 0x36, 0x46, 0x1a, 0xd3, 0x85, 0x03, 0x66, 0x90, 0x45, 0x16, 0x28, 0x79, 0x03, 0x36, 0x23,
    0xdd, 0xbe, 0x6f, 0x03, 0xff, 0x04, 0xe3, 0xca, 0xd5, 0x7f, 0xfc, 0xa3, 0x50, 0xe4, 0x9e, 0xd9,
];
const V3_IV: [u8; 16] = [
    0xe0, 0x7a, 0xad, 0x35, 0xe0, 0x90, 0xaa, 0x03, 0x8a, 0x51, 0xfd, 0x05, 0xdf, 0x8c, 0x5d, 0x0f,
];

fn aes_padded(plain: &[u8]) -> Vec<u8> {
    use aes::cipher::{BlockEncryptMut, KeyIvInit};
    type Enc = cbc::Encryptor<aes::Aes256>;
    Enc::new(&V3_KEY.into(), &V3_IV.into())
        .encrypt_padded_vec_mut::<aes::cipher::block_padding::Pkcs7>(plain)
}

/// 合成 Skin-v3（PKCS7）。
fn skin_v3(files: &[(&str, &[u8])]) -> Vec<u8> {
    let inner = inner_bytes(files);
    let expanded = inner.len() as u32;
    let mut plain = expanded.to_le_bytes().to_vec();
    plain.extend_from_slice(&zlib(&inner));
    let mut out = b"Skin".to_vec();
    out.extend_from_slice(&3u32.to_le_bytes());
    out.extend_from_slice(&aes_padded(&plain));
    out
}

/// 合成 Skin-v3（零填充变体）。
fn skin_v3_zero_pad(files: &[(&str, &[u8])]) -> Vec<u8> {
    use aes::cipher::{BlockEncryptMut, KeyIvInit};
    type Enc = cbc::Encryptor<aes::Aes256>;
    let inner = inner_bytes(files);
    let mut plain = (inner.len() as u32).to_le_bytes().to_vec();
    plain.extend_from_slice(&zlib(&inner));
    while plain.len() % 16 != 0 {
        plain.push(0);
    }
    let mut out = b"Skin".to_vec();
    out.extend_from_slice(&3u32.to_le_bytes());
    out.extend_from_slice(
        &Enc::new(&V3_KEY.into(), &V3_IV.into())
            .encrypt_padded_vec_mut::<aes::cipher::block_padding::NoPadding>(&plain),
    );
    out
}

/// 合成 Skin-v1（明文内层）。
fn skin_v1(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = b"Skin".to_vec();
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&inner_bytes(files));
    out
}

/// 极简合法 skin.ini（UTF-16LE + BOM）。
fn sample_ini() -> Vec<u8> {
    let text = "\u{feff}[General]\r\nskin_name=测试皮肤\r\nskin_author=虎符\r\npreview_comp=skin1.png\r\n\
        [Display]\r\nfont_size=17\r\nfont_ch=微软雅黑\r\npinyin_color=0x79513b\r\nzhongwen_first_color=0x44678e\r\n\
        candidate_spacing=6\r\n[Scheme_H1]\r\npic=skin1.png\r\nlayout_horizontal=0,91,122\r\n\
        layout_vertical=0,70,54\r\npinyin_marge=61,4,99,140\r\nzhongwen_marge=15,3,99,140\r\n\
        [StatusBar]\r\npic=bar.png\r\nflash_cnt=1\r\n";
    let mut out = vec![0xFF, 0xFE];
    for u in text.encode_utf16() {
        out.extend_from_slice(&u.to_le_bytes());
    }
    out
}

const FAKE_PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3, 4];

fn sample_files() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("skin.ini", sample_ini()),
        ("skin1.png", FAKE_PNG.to_vec()),
        ("bar.png", FAKE_PNG.to_vec()),
    ]
}

fn borrowed<'a>(files: &'a [(&'static str, Vec<u8>)]) -> Vec<(&'a str, &'a [u8])> {
    files.iter().map(|(n, v)| (*n, v.as_slice())).collect()
}

// ---------- 容器往返 ----------

#[test]
fn v3_roundtrip() {
    let files = sample_files();
    let bytes = skin_v3(&borrowed(&files));
    let parsed = decode_archive(&bytes).expect("v3 解码失败");
    assert_eq!(parsed.get("skin.ini"), Some(sample_ini().as_slice()));
    assert_eq!(parsed.get("skin1.png"), Some(FAKE_PNG));
    let definition = parse_definition(&parsed).expect("定义解析失败");
    assert_eq!(definition.name, "测试皮肤");
    assert_eq!(definition.author, "虎符");
    assert_eq!(definition.preview, "skin1.png");
    // 0x79513b(BGR) → R=0x3B G=0x51 B=0x79 → 0xFF3B5179
    assert_eq!(definition.display.code_color, 0xFF3B_5179);
    assert_eq!(definition.display.first_color, 0xFF8E_6744); // 0x44678e → R=8E G=67 B=44
    assert_eq!(definition.display.candidate_spacing, 6.0);
    let h1 = definition.schemes.horizontal.as_ref().expect("H1 缺失");
    assert_eq!(h1.image, "skin1.png");
    assert_eq!(
        h1.horizontal,
        SsfStretch {
            tile: false,
            before: 91,
            after: 122
        }
    );
    assert_eq!(
        h1.vertical,
        SsfStretch {
            tile: false,
            before: 70,
            after: 54
        }
    );
    assert_eq!(
        h1.code,
        SsfInsets {
            top: 61,
            bottom: 4,
            left: 99,
            right: 140
        }
    );
    // StatusBar 被忽略且无告警、无错误
    assert!(
        definition.warnings.is_empty(),
        "warnings: {:?}",
        definition.warnings
    );
}

#[test]
fn v3_zero_padding_variant() {
    let files = sample_files();
    let bytes = skin_v3_zero_pad(&borrowed(&files));
    let parsed = decode_archive(&bytes).expect("零填充 v3 解码失败");
    assert!(parsed.get("skin.ini").is_some());
}

#[test]
fn v1_roundtrip() {
    let files = sample_files();
    let bytes = skin_v1(&borrowed(&files));
    let parsed = decode_archive(&bytes).expect("v1 解码失败");
    assert_eq!(parsed.len(), 3);
}

#[test]
fn zip_roundtrip_stored_and_deflated() {
    let files = sample_files();
    let ini = &files[0].1;
    let png = &files[1].1;
    let bar = &files[2].1;
    let mut zip = Vec::new();
    let mut central = Vec::new();
    let mut locals = Vec::new();
    struct Ent {
        name: &'static str,
        packed: Vec<u8>,
        method: u16,
        crc: u32,
        usize_: u32,
    }
    let ents = [
        Ent {
            name: "skin.ini",
            packed: ini.clone(),
            method: 0,
            crc: container::crc32(ini),
            usize_: ini.len() as u32,
        },
        Ent {
            name: "skin1.png",
            packed: deflate(png),
            method: 8,
            crc: container::crc32(png),
            usize_: png.len() as u32,
        },
        Ent {
            name: "bar.png",
            packed: deflate(bar),
            method: 8,
            crc: container::crc32(bar),
            usize_: bar.len() as u32,
        },
    ];
    let mut offsets = Vec::new();
    for e in &ents {
        offsets.push(locals.len() as u32);
        locals.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        locals.extend_from_slice(&20u16.to_le_bytes()); // version
        locals.extend_from_slice(&0u16.to_le_bytes()); // flags
        locals.extend_from_slice(&e.method.to_le_bytes());
        locals.extend_from_slice(&0u32.to_le_bytes()); // time
        locals.extend_from_slice(&e.crc.to_le_bytes());
        locals.extend_from_slice(&(e.packed.len() as u32).to_le_bytes());
        locals.extend_from_slice(&e.usize_.to_le_bytes());
        locals.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
        locals.extend_from_slice(&0u16.to_le_bytes()); // extra
        locals.extend_from_slice(e.name.as_bytes());
        locals.extend_from_slice(&e.packed);
    }
    for (i, e) in ents.iter().enumerate() {
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // made by
        central.extend_from_slice(&20u16.to_le_bytes()); // needed
        central.extend_from_slice(&0u16.to_le_bytes()); // flags
        central.extend_from_slice(&e.method.to_le_bytes());
        central.extend_from_slice(&0u32.to_le_bytes());
        central.extend_from_slice(&e.crc.to_le_bytes());
        central.extend_from_slice(&(e.packed.len() as u32).to_le_bytes());
        central.extend_from_slice(&e.usize_.to_le_bytes());
        central.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
        central.extend_from_slice(&8u16.to_le_bytes()); // extra："SD" 私有字段（应被忽略）
        central.extend_from_slice(&0u16.to_le_bytes()); // comment
        central.extend_from_slice(&0u16.to_le_bytes()); // disk
        central.extend_from_slice(&0u16.to_le_bytes()); // int attrs
        central.extend_from_slice(&0u32.to_le_bytes()); // ext attrs
        central.extend_from_slice(&offsets[i].to_le_bytes());
        central.extend_from_slice(e.name.as_bytes());
        central.extend_from_slice(b"SD\x04\x00\xd0\x00\x00\x00"); // SD extra data
    }
    let cd_offset = locals.len() as u32;
    zip.extend_from_slice(&locals);
    zip.extend_from_slice(&central);
    let cd_size = zip.len() as u32 - cd_offset;
    // EOCD + 注释（验证注释长度吻合的尾部扫描）
    zip.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    zip.extend_from_slice(&0u16.to_le_bytes());
    zip.extend_from_slice(&0u16.to_le_bytes());
    zip.extend_from_slice(&(ents.len() as u16).to_le_bytes());
    zip.extend_from_slice(&(ents.len() as u16).to_le_bytes());
    zip.extend_from_slice(&cd_size.to_le_bytes());
    zip.extend_from_slice(&cd_offset.to_le_bytes());
    zip.extend_from_slice(&2u16.to_le_bytes());
    zip.extend_from_slice(b"hi");
    let parsed = decode_archive(&zip).expect("ZIP 解码失败");
    assert_eq!(parsed.get("skin.ini"), Some(ini.as_slice()));
    assert_eq!(parsed.get("skin1.png"), Some(png.as_slice()));
    let definition = parse_definition(&parsed).unwrap();
    assert_eq!(definition.name, "测试皮肤");
}

// ---------- 模型细节 ----------

#[test]
fn choose_fallback_chain() {
    let mut schemes = SsfSchemes::default();
    assert!(schemes.choose(false, true, true).is_none());
    schemes.horizontal_candidates = Some(SsfLayout {
        image: "a.png".into(),
        horizontal: SsfStretch::FULL,
        vertical: SsfStretch::FULL,
        code: Default::default(),
        candidates: Default::default(),
        decorations: vec![],
    });
    // 只有候选窗变体：任何请求都回退到它。
    assert!(schemes.choose(false, true, true).is_some());
    assert!(schemes.choose(true, true, true).is_some());
    assert!(schemes.choose(false, false, false).is_some());
}

#[test]
fn bad_fields_degrade_to_warnings() {
    let ini = "[General]\nskin_name=x\n[Scheme_H1]\npic=skin1.png\nlayout_horizontal=9,1,1\n\
        layout_vertical=0,1\npinyin_marge=1,2,3\n[Scheme_H2]\npinyin_pic=skin1.png\ncomp_custom_cnt=1\n";
    let files = [("skin.ini", ini.as_bytes()), ("skin1.png", FAKE_PNG)];
    let parsed = decode_archive(&skin_v3(&files)).unwrap();
    let definition = parse_definition(&parsed).unwrap();
    // mode=9、layout 两值、marge 三值 → 全部降级 + 告警
    assert!(
        definition.warnings.iter().any(|w| w.contains("伸缩参数")),
        "{:?}",
        definition.warnings
    );
    assert!(
        definition.warnings.iter().any(|w| w.contains("边距")),
        "{:?}",
        definition.warnings
    );
    // comp_custom_cnt=1 但 comp_custom0 缺 → 跳过装饰告警
    assert!(
        definition.warnings.iter().any(|w| w.contains("未定义装饰")),
        "{:?}",
        definition.warnings
    );
    let h1 = definition.schemes.horizontal.as_ref().unwrap();
    assert_eq!(h1.horizontal, SsfStretch::FULL); // 无效 → 全拉伸
    assert_eq!(h1.code, SsfInsets::default());
    assert_eq!(h1.decorations.len(), 0);
}

#[test]
fn missing_image_drops_variant_with_warning() {
    let ini = "[General]\nskin_name=x\n[Scheme_V1]\npic=ghost.png\n[Scheme_H1]\npic=skin1.png\n";
    let files = [("skin.ini", ini.as_bytes()), ("skin1.png", FAKE_PNG)];
    let parsed = decode_archive(&skin_v3(&files)).unwrap();
    let definition = parse_definition(&parsed).unwrap();
    assert!(definition.schemes.vertical.is_none());
    assert!(definition.warnings.iter().any(|w| w.contains("ghost.png")));
    assert!(definition.schemes.horizontal.is_some());
}

#[test]
fn no_scheme_is_error() {
    let ini = "[General]\nskin_name=x\n";
    let files = [("skin.ini", ini.as_bytes())];
    let parsed = decode_archive(&skin_v3(&files)).unwrap();
    assert!(parse_definition(&parsed).is_err());
}

#[test]
fn margin_alias_and_duplicate_key() {
    let ini = "[General]\nskin_name=x\n[Scheme_H1]\npic=skin1.png\npinyin_margin=1,2,3,4\npinyin_marge=5,6,7,8\npinyin_marge=9,10,11,12\n";
    let files = [("skin.ini", ini.as_bytes()), ("skin1.png", FAKE_PNG)];
    let parsed = decode_archive(&skin_v3(&files)).unwrap();
    let definition = parse_definition(&parsed).unwrap();
    let h1 = definition.schemes.horizontal.unwrap();
    assert_eq!(h1.code.top, 9); // 重复键取后者
    assert!(definition.warnings.iter().any(|w| w.contains("重复键")));
}

#[test]
fn gbk_ini_decoded() {
    let (gbk, _, _) = encoding_rs::GBK.encode("汉仪中等线简");
    let text = decode_text(gbk.as_ref()).unwrap();
    assert_eq!(text, "汉仪中等线简");
}

#[test]
fn color_bgr_variants() {
    // 0x79513B → 0xFF3B5179；8 位（带被忽略的 alpha 字节）同值
    assert_eq!(model::color_bgr("0x79513b"), Some(0xFF3B_5179));
    assert_eq!(model::color_bgr("0xff79513b"), Some(0xFF3B_5179));
    assert_eq!(model::color_bgr("79513b"), None); // 无 0x 前缀
}

// ---------- 恶意/畸形拒绝 ----------

fn expect_err(bytes: &[u8], needle: &str) {
    let err = decode_archive(bytes).err().expect("应当拒绝").to_string();
    assert!(err.contains(needle), "错误信息不符：{err}");
}

#[test]
fn rejects_bad_inputs() {
    expect_err(b"", "过短");
    expect_err(b"NOTAMAGIC", "签名");
    expect_err(b"Skin\x02\x00\x00\x00zzzzzzzzzzzz", "版本");
    let good = skin_v3(&borrowed(&sample_files()));
    let mut truncated = good.clone();
    truncated.truncate(truncated.len() - 5);
    expect_err(&truncated, "");
    // 路径穿越条目
    let evil = inner_bytes(&[("../evil.txt", b"x"), ("skin.ini", b"[General]\n")]);
    let mut plain = (evil.len() as u32).to_le_bytes().to_vec();
    plain.extend_from_slice(&zlib(&evil));
    let mut bytes = b"Skin".to_vec();
    bytes.extend_from_slice(&3u32.to_le_bytes());
    bytes.extend_from_slice(&aes_padded(&plain));
    let err = decode_archive(&bytes)
        .err()
        .expect("穿越名应被拒")
        .to_string();
    assert!(err.contains("路径"), "{err}");
    // zip 炸弹：stored 声明 100MB 实际 4 字节
    let mut zip = Vec::new();
    zip.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
    zip.extend_from_slice(&[0u8; 26]);
    zip.extend_from_slice(&0u32.to_le_bytes()); // crc
    zip.extend_from_slice(&4u32.to_le_bytes()); // csize
    zip.extend_from_slice(&100_000_000u32.to_le_bytes()); // usize
    zip.extend_from_slice(&(b"skin.ini".len() as u16).to_le_bytes());
    zip.extend_from_slice(&0u16.to_le_bytes());
    zip.extend_from_slice(b"skin.ini");
    zip.extend_from_slice(b"data");
    let cd_off = zip.len() as u32;
    zip.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
    zip.extend_from_slice(&[0u8; 40]);
    zip.extend_from_slice(&(b"skin.ini".len() as u16).to_le_bytes());
    zip.extend_from_slice(&0u16.to_le_bytes());
    zip.extend_from_slice(&0u16.to_le_bytes());
    zip.extend_from_slice(&0u16.to_le_bytes());
    zip.extend_from_slice(&0u16.to_le_bytes());
    zip.extend_from_slice(&0u32.to_le_bytes());
    zip.extend_from_slice(&0u32.to_le_bytes()); // local offset 0
    zip.extend_from_slice(b"skin.ini");
    let cd_size = zip.len() as u32 - cd_off;
    zip.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    zip.extend_from_slice(&[0u8; 16]);
    zip.extend_from_slice(&1u16.to_le_bytes());
    zip.extend_from_slice(&1u16.to_le_bytes());
    zip.extend_from_slice(&cd_size.to_le_bytes());
    zip.extend_from_slice(&cd_off.to_le_bytes());
    zip.extend_from_slice(&0u16.to_le_bytes());
    // 声明 100MB 触发预算/尺寸拒绝
    let err = decode_archive(&zip).err().expect("炸弹应被拒").to_string();
    assert!(!err.is_empty());
}

#[test]
fn sanitize_path_rules() {
    assert_eq!(sanitize_path("Skin1.PNG").unwrap(), "skin1.png");
    assert_eq!(sanitize_path("a\\B\\c.png").unwrap(), "a/b/c.png");
    assert!(sanitize_path("../x").is_err());
    assert!(sanitize_path("a/../b").is_err());
    assert!(sanitize_path("/abs").is_err());
    assert!(sanitize_path("con.png").is_err());
    assert!(sanitize_path("COM1").is_err());
    assert!(sanitize_path("x|y").is_err());
    assert!(sanitize_path("").is_err());
}

// ---------- 真实语料（可选，env 门控） ----------

#[test]
fn real_corpus_when_configured() {
    let Ok(dir) = std::env::var("HUFU_SSF_CORPUS") else {
        return; // 未配置则跳过（CI 无语料）
    };
    let mut checked = 0usize;
    let mut failed = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap().flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("ssf") {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        match decode_archive(&bytes).and_then(|f| parse_definition(&f).map(|d| (f, d))) {
            Ok((files, definition)) => {
                assert!(!definition.schemes.used_images().is_empty());
                for image in definition.schemes.used_images() {
                    assert!(
                        files.get(&image).is_some(),
                        "{} 缺 {}",
                        path.display(),
                        image
                    );
                }
                let variants = [
                    definition.schemes.horizontal.is_some(),
                    definition.schemes.vertical.is_some(),
                    definition.schemes.horizontal_code.is_some(),
                    definition.schemes.horizontal_candidates.is_some(),
                    definition.schemes.vertical_code.is_some(),
                    definition.schemes.vertical_candidates.is_some(),
                ]
                .iter()
                .filter(|b| **b)
                .count();
                println!(
                    "[corpus] {:?} => 条目 {} | 变体 {}/6 | 预览 {} | 字号 {} | 告警 {:?}",
                    path.file_name().unwrap().to_string_lossy(),
                    files.len(),
                    variants,
                    definition.preview,
                    definition.display.font_size,
                    definition.warnings
                );
                checked += 1;
            }
            Err(e) => failed.push(format!("{} => {e}", path.display())),
        }
    }
    assert!(checked >= 1, "语料目录无可用皮肤：{dir}");
    assert!(failed.is_empty(), "真实皮肤解析失败：{failed:?}");
}
