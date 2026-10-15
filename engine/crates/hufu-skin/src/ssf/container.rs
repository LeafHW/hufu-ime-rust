//! 【SSF M1】三代容器解码：明文 ZIP / "Skin" v1 明文 / "Skin" v3 加密。
//!
//! 结构参考虎娘 `native/SsfArchive.cpp`（语义照抄）与
//! 《.refs/ssf格式调研报告.md》§2（dsh-sogou-skin 实测口径）。
//! 全程内存解析、预算封顶、CRC 校验、路径消毒、重叠检测——
//! 本模块是解析不可信文件的边界，任何输入都不允许 panic。

use super::{
    SsfError, MAX_ASSET_BYTES, MAX_ENTRIES, MAX_EXPANDED_BYTES, MAX_INPUT_BYTES, MAX_NAME_BYTES,
};
use std::collections::BTreeMap;

/// 搜狗 Skin-v3 容器的 AES-256-CBC 固定密钥与 IV（公开格式常量；
/// KDE kimtoy / ssf2fcitx / ssfconv / unSSF 十余年共识，本机已对
/// 2020 年商店皮肤实测验证——见 .refs/虎娘ssf实现逆向报告.md §1.1）。
const V3_KEY: [u8; 32] = [
    0x52, 0x36, 0x46, 0x1a, 0xd3, 0x85, 0x03, 0x66, 0x90, 0x45, 0x16, 0x28, 0x79, 0x03, 0x36, 0x23,
    0xdd, 0xbe, 0x6f, 0x03, 0xff, 0x04, 0xe3, 0xca, 0xd5, 0x7f, 0xfc, 0xa3, 0x50, 0xe4, 0x9e, 0xd9,
];
const V3_IV: [u8; 16] = [
    0xe0, 0x7a, 0xad, 0x35, 0xe0, 0x90, 0xaa, 0x03, 0x8a, 0x51, 0xfd, 0x05, 0xdf, 0x8c, 0x5d, 0x0f,
];

/// 归一化后的条目表：小写、`/` 分隔的相对路径 → 原始字节。
#[derive(Debug, Default, Clone)]
pub struct Files(BTreeMap<String, Vec<u8>>);

impl Files {
    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.0.get(name).map(|v| v.as_slice())
    }
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Vec<u8>)> {
        self.0.iter()
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    fn insert(&mut self, name: String, bytes: Vec<u8>) -> Result<(), SsfError> {
        if self.0.insert(name.clone(), bytes).is_some() {
            return Err(SsfError::Message(format!("文件名重复：{name}")));
        }
        Ok(())
    }
    /// 【QPYS】由既有映射构造（QQ 皮肤 OLE 流表 → 条目表；名字已归一）。
    pub(crate) fn from_map(map: std::collections::BTreeMap<String, Vec<u8>>) -> Files {
        Files(map)
    }
}

fn err<T>(text: impl Into<String>) -> Result<T, SsfError> {
    Err(SsfError::Message(text.into()))
}

fn u16le(b: &[u8], p: usize) -> Result<u16, SsfError> {
    let p = p
        .checked_add(2)
        .filter(|end| *end <= b.len())
        .ok_or_else(|| SsfError::Message("整数读取越界（文件截断）".into()))?;
    Ok(u16::from_le_bytes([b[p - 2], b[p - 1]]))
}

fn u32le(b: &[u8], p: usize) -> Result<u32, SsfError> {
    let end = p
        .checked_add(4)
        .filter(|end| *end <= b.len())
        .ok_or_else(|| SsfError::Message("整数读取越界（文件截断）".into()))?;
    Ok(u32::from_le_bytes(b[p..end].try_into().unwrap()))
}

fn slice(b: &[u8], p: usize, n: usize) -> Result<&[u8], SsfError> {
    let end = p
        .checked_add(n)
        .filter(|end| *end <= b.len())
        .ok_or_else(|| SsfError::Message("区间读取越界（文件截断）".into()))?;
    Ok(&b[p..end])
}

/// CRC32（IEEE 802.3，多项式 0xEDB88320）——ZIP 与 v3 均用它校验。
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in bytes {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

// --------------------------------------------------------------------------------
// 路径消毒
// --------------------------------------------------------------------------------

/// 条目名/图片引用 → 归一安全相对路径：`\`→`/`、ASCII 小写、拒绝空段/`.`/`..`/
/// 非法字符(`< > : " | ? *` 与控制字符)/Windows 保留设备名，限长。
/// ZIP 名若非 UTF-8 再按 GBK 兜底解码（历史皮肤多为 ASCII，正常路径不受影响）。
pub fn sanitize_path(name: &str) -> Result<String, SsfError> {
    let name = name.trim_start_matches('\u{feff}');
    if name.is_empty() || name.len() > MAX_NAME_BYTES {
        return err("路径为空或超长");
    }
    const RESERVED: [&str; 22] = [
        "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
        "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ];
    let mut parts: Vec<String> = Vec::new();
    for raw in name.split(['/', '\\']) {
        if raw.is_empty() || raw == "." || raw == ".." {
            return err(format!("路径含非法段：{name}"));
        }
        if raw.len() > 255 {
            return err(format!("路径段超长：{name}"));
        }
        let lowered: String = raw
            .chars()
            .map(|c| {
                if c.is_ascii_uppercase() {
                    c.to_ascii_lowercase()
                } else {
                    c
                }
            })
            .collect();
        if lowered
            .chars()
            .any(|c| (c as u32) < 0x20 || "<>:\"|?*".contains(c))
        {
            return err(format!("路径含非法字符：{name}"));
        }
        // 保留设备名判定取主名（去扩展名），如 con.txt 仍拒绝。
        let stem = lowered.split('.').next().unwrap_or("");
        if RESERVED.contains(&stem) {
            return err(format!("路径段为保留设备名：{name}"));
        }
        parts.push(lowered);
    }
    let joined = parts.join("/");
    if joined.len() > 1024 {
        return err("路径总长超限");
    }
    Ok(joined)
}

/// ZIP 条目名字节 → 文本：先 UTF-8，失败按 GBK（历史编辑器产物的 OEM 名）。
fn zip_name_text(bytes: &[u8]) -> Result<String, SsfError> {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return Ok(text.to_string());
    }
    let (text, _, had_errors) = encoding_rs::GBK.decode(bytes);
    if had_errors {
        return err("条目名既非 UTF-8 也非 GBK");
    }
    Ok(text.into_owned())
}

// --------------------------------------------------------------------------------
// 解压
// --------------------------------------------------------------------------------

/// 以预期解压尺寸解 zlib 流（v3 内层）。要求恰好产出 expanded 字节；
/// 尾部允许多余全零字节（对零填充变体的兼容——本机实测商店皮肤均为
/// 合法 PKCS7，但社区实现按零填充处理，两者都容）。
fn inflate_zlib_expect(input: &[u8], expanded: usize) -> Result<Vec<u8>, SsfError> {
    if expanded > MAX_EXPANDED_BYTES {
        return err("解压预算超限（>128MiB）");
    }
    let mut out = vec![0u8; expanded.max(1)];
    let mut d = flate2::Decompress::new(true);
    let status = d
        .decompress(input, &mut out, flate2::FlushDecompress::None)
        .map_err(|_| SsfError::Message("zlib 数据损坏".into()))?;
    if status != flate2::Status::StreamEnd
        || d.total_out() != expanded as u64
        || (d.total_in() as usize) > input.len()
    {
        return err("zlib 流长度与声明不符");
    }
    // 严格 PKCS7 剥离后通常恰好耗尽；零填充变体会留下全零尾巴。
    let consumed = d.total_in() as usize;
    if consumed < input.len() && input[consumed..].iter().any(|&b| b != 0) {
        return err("zlib 流后存在非零冗余数据");
    }
    out.truncate(expanded);
    Ok(out)
}

/// 原始 deflate（ZIP method 8），要求恰好产出 expanded 字节、恰好消费 packed 字节。
fn inflate_raw_expect(input: &[u8], expanded: usize) -> Result<Vec<u8>, SsfError> {
    if expanded > MAX_EXPANDED_BYTES {
        return err("解压预算超限（>128MiB）");
    }
    let mut out = vec![0u8; expanded.max(1)];
    let mut d = flate2::Decompress::new(false);
    let status = d
        .decompress(input, &mut out, flate2::FlushDecompress::None)
        .map_err(|_| SsfError::Message("deflate 数据损坏".into()))?;
    if status != flate2::Status::StreamEnd
        || d.total_out() != expanded as u64
        || d.total_in() as usize != input.len()
    {
        return err("deflate 流与 ZIP 声明尺寸不符");
    }
    Ok(out)
}

// --------------------------------------------------------------------------------
// Skin 容器（v1 明文 / v3 加密）共用的内层文件表
// --------------------------------------------------------------------------------

/// 内层文件表：`u32 自尺寸, u32 目录表字节, u32 偏移[N]`，
/// 每条目 `u32 名长 + UTF-16LE 名 + u32 内容长 + 内容`。
fn inner_table(data: &[u8]) -> Result<Files, SsfError> {
    if data.len() < 8 {
        return err("内层文件表过短");
    }
    let self_size = u32le(data, 0)? as usize;
    if self_size != data.len() {
        return err("自尺寸校验不符");
    }
    let table = u32le(data, 4)? as usize;
    if table == 0 || !table.is_multiple_of(4) || table > data.len() - 8 || table / 4 > MAX_ENTRIES {
        return err("目录表损坏");
    }
    let entries = table / 4;
    let table_end = 8 + table;
    let mut files = Files::default();
    let mut previous_end = table_end; // 条目偏移必须升序、不得重叠
    for i in 0..entries {
        let offset = u32le(data, 8 + i * 4)? as usize;
        if offset < previous_end {
            return err("条目偏移重叠");
        }
        let name_len = u32le(data, offset)? as usize;
        if name_len == 0 || name_len > MAX_NAME_BYTES || !name_len.is_multiple_of(2) {
            return err("条目名长度非法");
        }
        let name_bytes = slice(data, offset + 4, name_len)?;
        let name = utf16le_name(name_bytes)?;
        let content_len = u32le(data, offset + 4 + name_len)? as usize;
        if content_len > MAX_ASSET_BYTES {
            return err("条目内容超限（>64MiB）");
        }
        let content = slice(data, offset + 8 + name_len, content_len)?.to_vec();
        previous_end = offset + 8 + name_len + content_len;
        files.insert(name, content)?;
    }
    if previous_end != data.len() {
        return err("文件表尾部有多余数据");
    }
    Ok(files)
}

/// UTF-16LE 名 → 消毒路径（含代理对校验）。
fn utf16le_name(bytes: &[u8]) -> Result<String, SsfError> {
    let mut units = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.as_chunks::<2>().0 {
        units.push(u16::from_le_bytes([pair[0], pair[1]]));
    }
    let text =
        String::from_utf16(&units).map_err(|_| SsfError::Message("文件名 UTF-16 非法".into()))?;
    sanitize_path(&text)
}

/// Skin v3：`"Skin" + u32(3) + AES-256-CBC 密文`；
/// 明文 = `u32 解压后长度 + zlib(内层文件表)`。
fn skin_v3(raw: &[u8]) -> Result<Files, SsfError> {
    let body = &raw[8..];
    if body.is_empty() || !body.len().is_multiple_of(16) {
        return err("密文长度非法（非 16 字节对齐）");
    }
    use aes::cipher::{BlockDecryptMut, KeyIvInit};
    type Dec = cbc::Decryptor<aes::Aes256>;
    let mut buf = body.to_vec();
    // 先按严格 PKCS7；失败退回"剥尾部零"（社区零填充变体，见函数头注释）。
    let plain: Vec<u8> = match Dec::new(&V3_KEY.into(), &V3_IV.into())
        .decrypt_padded_mut::<aes::cipher::block_padding::Pkcs7>(&mut buf)
    {
        Ok(stripped) => stripped.to_vec(),
        Err(_) => {
            // 注意：上面失败的调用已把 buf 原地解密——必须用原始密文重来。
            let mut retry = body.to_vec();
            let mut unpadded = Dec::new(&V3_KEY.into(), &V3_IV.into())
                .decrypt_padded_mut::<aes::cipher::block_padding::NoPadding>(&mut retry)
                .map_err(|_| SsfError::Message("AES 解密失败".into()))?
                .to_vec();
            // 零填充变体：剥掉尾部全零（剥多剥少都无妨——zlib 解压
            // 按声明长度自校验，错一刀立刻被拒）。
            if *unpadded.last().unwrap_or(&1) == 0 {
                while matches!(unpadded.last(), Some(0)) {
                    unpadded.pop();
                }
                unpadded
            } else {
                return err("密文填充非法（既非 PKCS7 也非零填充）");
            }
        }
    };
    if plain.len() < 4 {
        return err("解密后数据过短");
    }
    let expanded = u32le(&plain, 0)? as usize;
    let inflated = inflate_zlib_expect(&plain[4..], expanded)?;
    inner_table(&inflated)
}

/// Skin v1：`"Skin" + u32(1) + 内层文件表`（无加密无压缩）。
/// 防御性双路径：若偏移 8 处不是合法自尺寸而像 zlib 流，也试解压路线。
fn skin_v1(raw: &[u8]) -> Result<Files, SsfError> {
    let rest = &raw[8..];
    if rest.len() >= 8 && u32le(rest, 0)? as usize == rest.len() {
        return inner_table(rest);
    }
    if rest.len() >= 8 {
        let expanded = u32le(rest, 0)? as usize;
        if expanded <= MAX_EXPANDED_BYTES {
            if let Ok(inflated) = inflate_zlib_expect(&rest[4..], expanded) {
                return inner_table(&inflated);
            }
        }
    }
    err("Skin v1 内层结构非法")
}

// --------------------------------------------------------------------------------
// 明文 ZIP（皮肤编辑器产物 / 虎娘皮肤）
// --------------------------------------------------------------------------------

fn zip(raw: &[u8]) -> Result<Files, SsfError> {
    // 1) 从尾部找 EOCD（PK\x05\x06），注释长度必须与剩余字节吻合。
    let min_eocd = 22;
    if raw.len() < min_eocd {
        return err("ZIP 过短");
    }
    let scan_start = raw.len().saturating_sub(min_eocd + 65_535);
    let mut eocd = None;
    let mut i = raw.len() - min_eocd;
    loop {
        if &raw[i..i + 4] == b"PK\x05\x06" {
            let comment_len = u16le(raw, i + 20)? as usize;
            if i + min_eocd + comment_len == raw.len() {
                eocd = Some(i);
                break;
            }
        }
        if i == scan_start {
            break;
        }
        i -= 1;
    }
    let eocd = eocd.ok_or_else(|| SsfError::Message("ZIP 找不到目录结尾记录".into()))?;
    if u16le(raw, eocd + 4)? != 0 || u16le(raw, eocd + 6)? != 0 {
        return err("ZIP64/多盘 SSF 不受支持");
    }
    let entries_disk = u16le(raw, eocd + 8)? as usize;
    let entries_total = u16le(raw, eocd + 10)? as usize;
    let cd_size = u32le(raw, eocd + 12)? as usize;
    let cd_offset = u32le(raw, eocd + 16)? as usize;
    if entries_disk != entries_total || entries_total == 0 || entries_total > MAX_ENTRIES {
        return err("ZIP 条目数非法");
    }
    if cd_offset
        .checked_add(cd_size)
        .map(|end| end > eocd)
        .unwrap_or(true)
    {
        return err("ZIP 目录区越界");
    }

    // 2) 中央目录条目 → (消毒名, span, method, crc, csize, usize, local_off)
    struct Entry {
        key: String,
        /// 中央目录里名字的原始字节区间（与本地头比对用）。
        name_range: std::ops::Range<usize>,
        csize: usize,
        usize_: usize,
        method: u16,
        crc: u32,
        local_off: usize,
    }
    let mut entries = Vec::with_capacity(entries_total);
    let mut p = cd_offset;
    for _ in 0..entries_total {
        if u32le(raw, p)? != 0x0201_4b50 {
            return err("ZIP 中央目录签名损坏");
        }
        if u16le(raw, p + 6)? >= 45 {
            return err("ZIP64/多盘 SSF 不受支持");
        }
        let flags = u16le(raw, p + 8)?;
        if flags & 1 != 0 {
            return err("加密 ZIP 不受支持");
        }
        let method = u16le(raw, p + 10)?;
        if method != 0 && method != 8 {
            return err("ZIP 压缩方法不受支持");
        }
        let crc = u32le(raw, p + 16)?;
        let csize = u32le(raw, p + 20)? as usize;
        let usize_ = u32le(raw, p + 24)? as usize;
        let name_len = u16le(raw, p + 28)? as usize;
        let extra_len = u16le(raw, p + 30)? as usize;
        let comment_len = u16le(raw, p + 32)? as usize;
        if u16le(raw, p + 34)? != 0 {
            return err("ZIP64/多盘 SSF 不受支持");
        }
        let local_off = u32le(raw, p + 42)? as usize;
        if name_len == 0
            || name_len > MAX_NAME_BYTES
            || csize > MAX_ASSET_BYTES
            || usize_ > MAX_ASSET_BYTES
        {
            return err("ZIP 条目尺寸/名长非法");
        }
        let name_raw = slice(raw, p + 46, name_len)?;
        let key = sanitize_path(&zip_name_text(name_raw)?)?;
        entries.push(Entry {
            key,
            name_range: p + 46..p + 46 + name_len,
            csize,
            usize_,
            method,
            crc,
            local_off,
        });
        p += 46 + name_len + extra_len + comment_len;
    }

    // 3) 本地头校验 + 解压 + CRC。数据区 span 不得重叠。
    let mut files = Files::default();
    let mut spans: Vec<(usize, usize)> = Vec::with_capacity(entries.len());
    let mut expanded_total: usize = 0;
    for e in &entries {
        if u32le(raw, e.local_off)? != 0x0403_4b50 {
            return err("ZIP 本地头签名损坏");
        }
        let lname = u16le(raw, e.local_off + 26)? as usize;
        let lextra = u16le(raw, e.local_off + 28)? as usize;
        let data = e.local_off + 30 + lname + lextra;
        let local_name = slice(raw, e.local_off + 30, lname)?;
        // 名字与中央目录一致（按消毒前的原始字节比对）。
        if local_name != &raw[e.name_range.clone()] {
            return err("ZIP 本地头与目录名不一致");
        }
        if e.usize_ == 0 && e.csize == 0 {
            files.insert(e.key.clone(), Vec::new())?;
            spans.push((data, data));
            continue;
        }
        let packed = slice(raw, data, e.csize)?;
        expanded_total = expanded_total
            .checked_add(e.usize_)
            .filter(|t| *t <= MAX_EXPANDED_BYTES)
            .ok_or_else(|| SsfError::Message("解压预算超限（>128MiB）".into()))?;
        let content = match e.method {
            0 => {
                if e.csize != e.usize_ {
                    return err("ZIP stored 条目尺寸不符");
                }
                packed.to_vec()
            }
            _ => inflate_raw_expect(packed, e.usize_)?,
        };
        if crc32(&content) != e.crc {
            return err(format!("CRC 校验失败：{}", e.key));
        }
        spans.push((data, data + e.csize));
        files.insert(e.key.clone(), content)?;
    }
    spans.sort_unstable();
    for w in spans.windows(2) {
        if w[0].1 > w[1].0 {
            return err("ZIP 条目数据区重叠");
        }
    }
    Ok(files)
}

// --------------------------------------------------------------------------------
// 入口
// --------------------------------------------------------------------------------

/// 识别三代容器并解码为条目表。要求根 `skin.ini` 存在。
pub fn decode_archive(raw: &[u8]) -> Result<Files, SsfError> {
    if raw.len() < 8 {
        return err("SSF 输入过短");
    }
    if raw.len() > MAX_INPUT_BYTES {
        return err("SSF 输入超限（>64MiB）");
    }
    let files = if &raw[0..4] == b"Skin" {
        match u32le(raw, 4)? {
            1 => skin_v1(raw)?,
            3 => skin_v3(raw)?,
            v => return err(format!("不支持的 Skin 容器版本：{v}")),
        }
    } else if u32le(raw, 0)? == 0x0403_4b50 {
        zip(raw)?
    } else {
        return err("不支持的 SSF 签名（既非 ZIP 也非 Skin 容器）");
    };
    if files.get("skin.ini").is_none() {
        return err("SSF 缺少根 skin.ini");
    }
    Ok(files)
}
