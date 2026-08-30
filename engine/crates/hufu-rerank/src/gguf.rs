//! GGUF 容器解析（只读所需：元数据 + 张量目录 + 按需读 q8 数据块）。

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};

#[derive(Debug, Clone)]
pub enum Value {
    U8(u8),
    I8(i8),
    U16(u16),
    I16(i16),
    U32(u32),
    I32(i32),
    U64(u64),
    I64(i64),
    F32(f32),
    F64(f64),
    Bool(bool),
    Str(String),
    Arr(Vec<Value>),
}

impl Value {
    pub fn as_u32(&self) -> Option<u32> {
        match self {
            Value::U8(v) => Some(*v as u32),
            Value::I8(v) => Some(*v as i64 as u32),
            Value::U16(v) => Some(*v as u32),
            Value::I16(v) => Some(*v as i64 as u32),
            Value::U32(v) => Some(*v),
            Value::I32(v) => Some(*v as i64 as u32),
            Value::U64(v) => Some(*v as u32),
            Value::I64(v) => Some(*v as u64 as u32),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::F32(v) => Some(*v as f64),
            Value::F64(v) => Some(*v),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_arr(&self) -> Option<&[Value]> {
        match self {
            Value::Arr(a) => Some(a),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GgmlDType {
    Q8_0,
    F32,
    F16,
    Other(u32),
}

impl GgmlDType {
    pub fn is_q8_0(&self) -> bool {
        matches!(self, GgmlDType::Q8_0)
    }
}

/// f16（半精度）→ f32，q8_0 块缩放用
pub fn f16_to_f32(b0: u8, b1: u8) -> f32 {
    let h = u16::from_le_bytes([b0, b1]);
    let sign = ((h >> 15) & 1) as u32;
    let exp = ((h >> 10) & 0x1f) as i32;
    let mant = (h & 0x3ff) as u32;
    let bits: u32 = if exp == 0 {
        if mant == 0 {
            sign << 31
        } else {
            // 次正规数：值 = mant × 2^-24（f64 中转，精确且无位技巧坑）
            let v = (mant as f64) * (-24f64).exp2();
            let v = if sign == 1 { -v } else { v };
            return v as f32;
        }
    } else if exp == 0x1f {
        (sign << 31) | (0xff << 23) | (mant << 13)
    } else {
        (sign << 31) | (((exp - 15 + 127) as u32) << 23) | (mant << 13)
    };
    f32::from_bits(bits)
}

#[derive(Debug, Clone)]
pub struct TensorInfo {
    pub name: String,
    pub shape: Vec<usize>, // gguf 维度序（首维=行/输出维）
    pub dtype: GgmlDType,
    pub offset: u64,
}

pub struct GgufFile {
    pub meta: HashMap<String, Value>,
    pub tensors: HashMap<String, TensorInfo>,
    pub data_start: u64,
    pub data: Vec<u8>,
    path: Option<String>,
    /// 懒模式 mmap：整个数据段只读映射一次（页缓存按需填充）。
    /// 此前每 tile 一次 File::open+seek_read（~1064 次/前向，~140ms
    /// 纯句柄开销）；mmap 后读权重=零系统调用。
    map: Option<memmap2::Mmap>,
}

struct Rd<R: Read> {
    f: Counting<R>,
}

struct Counting<R: Read> {
    inner: R,
    n: u64,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let k = self.inner.read(buf)?;
        self.n += k as u64;
        Ok(k)
    }
}

impl<R: Read> Rd<R> {
    fn new(f: R) -> Self {
        Rd { f: Counting { inner: f, n: 0 } }
    }
    fn read_u8(&mut self) -> std::io::Result<u8> {
        let mut b = [0u8; 1];
        self.f.read_exact(&mut b)?;
        Ok(b[0])
    }
    fn u16(&mut self) -> std::io::Result<u16> {
        let mut b = [0u8; 2];
        self.f.read_exact(&mut b)?;
        Ok(u16::from_le_bytes(b))
    }
    fn u32(&mut self) -> std::io::Result<u32> {
        let mut b = [0u8; 4];
        self.f.read_exact(&mut b)?;
        Ok(u32::from_le_bytes(b))
    }
    fn u64(&mut self) -> std::io::Result<u64> {
        let mut b = [0u8; 8];
        self.f.read_exact(&mut b)?;
        Ok(u64::from_le_bytes(b))
    }
    fn f32(&mut self) -> std::io::Result<f32> {
        let mut b = [0u8; 4];
        self.f.read_exact(&mut b)?;
        Ok(f32::from_le_bytes(b))
    }
    fn f64(&mut self) -> std::io::Result<f64> {
        let mut b = [0u8; 8];
        self.f.read_exact(&mut b)?;
        Ok(f64::from_le_bytes(b))
    }
    fn string(&mut self) -> std::io::Result<String> {
        let at = self.f.n;
        let len = self.u64()? as usize;
        if len > 64 * 1024 * 1024 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("字符串长度异常 {len} @ {at}"),
            ));
        }
        let mut b = vec![0u8; len];
        self.f.read_exact(&mut b)?;
        Ok(String::from_utf8_lossy(&b).into_owned())
    }
}

fn read_value<R: Read>(rd: &mut Rd<R>, vtype: u32) -> std::io::Result<Value> {
    Ok(match vtype {
        0 => Value::U8(rd.read_u8()?),
        1 => Value::I8(rd.read_u8()? as i8),
        2 => Value::U16(rd.u16()?),
        3 => Value::I16(rd.u16()? as i16),
        4 => Value::U32(rd.u32()?),
        5 => Value::I32(rd.u32()? as i32),
        6 => Value::F32(rd.f32()?),
        7 => Value::Bool(rd.read_u8()? != 0),
        8 => Value::Str(rd.string()?),
        9 => {
            let et = rd.u32()?;
            let n = rd.u64()? as usize;
            let mut v = Vec::with_capacity(n.min(1 << 22));
            for _ in 0..n {
                v.push(read_value(rd, et)?);
            }
            Value::Arr(v)
        }
        10 => Value::U64(rd.u64()?),
        11 => Value::I64(rd.u64()? as i64),
        12 => Value::F64(rd.f64()?),
        _ => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("未知 GGUF 值类型 {vtype}"),
            ))
        }
    })
}


impl GgufFile {
    pub fn open(path: &str) -> std::io::Result<Self> {
        let mut f = std::fs::File::open(path)?;
        let mut rd = Rd::new(&mut f);
        let magic = rd.u32()?;
        if &magic.to_le_bytes() != b"GGUF" {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "不是 GGUF 文件",
            ));
        }
        let _version = rd.u32()?;
        let tensor_count = rd.u64()?;
        let kv_count = rd.u64()?;
        let mut meta = HashMap::new();
        let dbg = std::env::var("GGUF_DEBUG").is_ok();
        for kvi in 0..kv_count {
            let key = rd.string()?;
            let vtype = rd.u32()?;
            let v = read_value(&mut rd, vtype).map_err(|e| {
                std::io::Error::new(
                    e.kind(),
                    format!("kv[{kvi}] {key} (类型{vtype}) 解析失败: {e}"),
                )
            })?;
            if dbg {
                eprintln!("kv[{kvi}] {key} 类型{vtype} ok");
            }
            meta.insert(key, v);
        }
        let mut tensors = HashMap::new();
        for ti in 0..tensor_count {
            let name = rd.string()?;
            let n_dims = rd.u32()? as usize;
            if n_dims > 8 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("tensor[{ti}] {name} 维度数异常: {n_dims}"),
                ));
            }
            let mut dims = Vec::with_capacity(n_dims);
            for d in 0..n_dims {
                let dv = rd.u64()?;
                if dv > 1_000_000_000 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("tensor[{ti}] {name} 第{d}维异常: {dv}"),
                    ));
                }
                dims.push(dv as usize);
            }
            let dt = rd.u32()?;
            let dtype = match dt {
                8 => GgmlDType::Q8_0,
                0 => GgmlDType::F32,
                1 => GgmlDType::F16,
                other => GgmlDType::Other(other),
            };
            let offset = rd.u64()?;
            tensors.insert(
                name.clone(),
                TensorInfo { name, shape: dims, dtype, offset },
            );
        }
        let pos = f.stream_position()?;
        let data_start = pos.div_ceil(32) * 32; // GGUF 数据段 32 对齐
        let total = f.metadata()?.len();
        let data_len = (total - data_start) as usize;
        let mut data = Vec::with_capacity(data_len);
        f.seek(SeekFrom::Start(data_start))?;
        f.take(data_len as u64).read_to_end(&mut data)?;
        Ok(Self { meta, tensors, data_start, data, path: None, map: None })
    }

    /// 只解析头部，数据按需从磁盘流式读（f32 大文件用，低内存）
    pub fn open_lazy(path: &str) -> std::io::Result<Self> {
        let mut probe = Self::open_header(path)?;
        probe.path = Some(path.to_string());
        // 数据段整体 mmap（只读共享映射，页缓存按需）——raw_rows 零系统调用
        let flen = std::fs::metadata(path)?.len();
        if flen > probe.data_start {
            if let Ok(f) = std::fs::File::open(path) {
                if let Ok(m) = unsafe { memmap2::MmapOptions::new().len(flen as usize).map(&f) } {
                    probe.map = Some(m);
                }
            }
        }
        Ok(probe)
    }

    fn open_header(path: &str) -> std::io::Result<Self> {
        // 复用 open 的解析逻辑但不读数据段
        let mut f = std::fs::File::open(path)?;
        let mut rd = Rd::new(&mut f);
        let magic = rd.u32()?;
        if &magic.to_le_bytes() != b"GGUF" {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "不是 GGUF 文件"));
        }
        let _version = rd.u32()?;
        let tensor_count = rd.u64()?;
        let kv_count = rd.u64()?;
        let mut meta = HashMap::new();
        for _ in 0..kv_count {
            let key = rd.string()?;
            let vtype = rd.u32()?;
            let v = read_value(&mut rd, vtype)?;
            meta.insert(key, v);
        }
        let mut tensors = HashMap::new();
        for _ in 0..tensor_count {
            let name = rd.string()?;
            let n_dims = rd.u32()? as usize;
            let mut dims = Vec::with_capacity(n_dims.min(8));
            for _ in 0..n_dims {
                dims.push(rd.u64()? as usize);
            }
            let dt = rd.u32()?;
            let dtype = match dt {
                8 => GgmlDType::Q8_0,
                0 => GgmlDType::F32,
                1 => GgmlDType::F16,
                other => GgmlDType::Other(other),
            };
            let offset = rd.u64()?;
            tensors.insert(name.clone(), TensorInfo { name, shape: dims, dtype, offset });
        }
        let pos = f.stream_position()?;
        let data_start = pos.div_ceil(32) * 32;
        Ok(Self { meta, tensors, data_start, data: Vec::new(), path: None, map: None })
    }

    pub fn is_lazy(&self) -> bool {
        self.path.is_some()
    }

    /// 读张量行 [p0, p0+pn) 的原始字节（懒模式从磁盘 seek_read）
    pub fn raw_rows(&self, info: &TensorInfo, p0: usize, pn: usize) -> std::io::Result<Vec<u8>> {
        let row_bytes = Self::row_bytes(info);
        let len = pn * row_bytes;
        let mut buf = vec![0u8; len];
        if !self.data.is_empty() {
            // 常驻：data 从 data_start 起存，张量偏移已是段内相对
            let start = info.offset as usize + p0 * row_bytes;
            buf.copy_from_slice(&self.data[start..start + len]);
            Ok(buf)
        } else if let Some(m) = &self.map {
            // mmap 热路径：零 open/零 read 系统调用（map 覆盖整个文件，
            // 张量偏移需加 data_start）
            let off = self.data_start as usize + info.offset as usize + p0 * row_bytes;
            if off + len <= m.len() {
                buf.copy_from_slice(&m[off..off + len]);
                Ok(buf)
            } else {
                Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "mmap 越界"))
            }
        } else {
            use std::os::windows::fs::FileExt;
            let start = self.data_start + info.offset + (p0 * row_bytes) as u64;
            let f = std::fs::File::open(self.path.as_deref().unwrap_or(""))?;
            let mut got = 0usize;
            while got < len {
                let n = f.seek_read(&mut buf[got..], start + got as u64)?;
                if n == 0 {
                    return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "读张量越界"));
                }
                got += n;
            }
            Ok(buf)
        }
    }

    fn row_bytes(info: &TensorInfo) -> usize {
        let k = info.shape[0];
        match info.dtype {
            GgmlDType::Q8_0 => k.div_ceil(32) * 34,
            GgmlDType::F32 => k * 4,
            GgmlDType::F16 => k * 2,
            GgmlDType::Other(_) => k * 4,
        }
    }

    /// Q8_0 单行 AVX2 反量化（k 为 32 的倍数；32×i8 = 2×128bit = 4 组 8 lane）。
    #[cfg(target_arch = "x86_64")]
    unsafe fn dequant_q8_0_row_avx2(raw: &[u8], blocks: usize, out: &mut Vec<f32>) {
        use std::arch::x86_64::*;
        let pre = out.len();
        out.resize(pre + blocks * 32, 0.0);
        let dst = &mut out[pre..];
        for b in 0..blocks {
            let off = b * 34;
            let scale = f16_to_f32(raw[off], raw[off + 1]);
            let s = _mm256_set1_ps(scale);
            // 32 个 int8：两条 128bit 载入，各拆 low/high 64bit → 4 组 8×i32→f32
            let a0 = _mm_loadu_si128(raw.as_ptr().add(off + 2) as *const __m128i);
            let a1 = _mm_loadu_si128(raw.as_ptr().add(off + 2 + 16) as *const __m128i);
            let base = dst.as_mut_ptr().add(b * 32);
            let w0 = _mm256_cvtepi8_epi32(a0);
            let w1 = _mm256_cvtepi8_epi32(_mm_unpackhi_epi64(a0, a0));
            let w2 = _mm256_cvtepi8_epi32(a1);
            let w3 = _mm256_cvtepi8_epi32(_mm_unpackhi_epi64(a1, a1));
            _mm256_storeu_ps(base, _mm256_mul_ps(_mm256_cvtepi32_ps(w0), s));
            _mm256_storeu_ps(base.add(8), _mm256_mul_ps(_mm256_cvtepi32_ps(w1), s));
            _mm256_storeu_ps(base.add(16), _mm256_mul_ps(_mm256_cvtepi32_ps(w2), s));
            _mm256_storeu_ps(base.add(24), _mm256_mul_ps(_mm256_cvtepi32_ps(w3), s));
        }
    }

    /// 原始字节 → f32 行主 [pn, k]
    pub fn dequant_rows_bytes(info: &TensorInfo, raw: &[u8], pn: usize) -> Vec<f32> {
        let k = info.shape[0];
        let rb = Self::row_bytes(info);
        let mut out = Vec::with_capacity(pn * k);
        match info.dtype {
            GgmlDType::Q8_0 => {
                // AVX2 向量反量化：一块 32×i8 恰为 4 组 8 lane，标量循环的
                // ~6-8 倍（实测单候选前向 790ms 中 dequant 是大头）。
                static AVX2: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
                let avx2 = *AVX2
                    .get_or_init(|| std::arch::is_x86_feature_detected!("avx2"));
                let blocks = k.div_ceil(32);
                for r in 0..pn {
                    let ro = r * rb;
                    if avx2 && k % 32 == 0 {
                        unsafe { Self::dequant_q8_0_row_avx2(&raw[ro..ro + blocks * 34], blocks, &mut out) };
                        continue;
                    }
                    for b in 0..blocks {
                        let off = ro + b * 34;
                        let scale = f16_to_f32(raw[off], raw[off + 1]);
                        let n = 32.min(k - b * 32);
                        for i in 0..n {
                            out.push(raw[off + 2 + i] as i8 as f32 * scale);
                        }
                    }
                }
            }
            GgmlDType::F32 => {
                for r in 0..pn {
                    let ro = r * rb;
                    for i in 0..k {
                        let off = ro + i * 4;
                        out.push(f32::from_le_bytes([raw[off], raw[off + 1], raw[off + 2], raw[off + 3]]));
                    }
                }
            }
            GgmlDType::F16 => {
                for r in 0..pn {
                    let ro = r * rb;
                    for i in 0..k {
                        let off = ro + i * 2;
                        out.push(f16_to_f32(raw[off], raw[off + 1]));
                    }
                }
            }
            GgmlDType::Other(_) => {}
        }
        out
    }

    /// 读张量行 [p0, p0+pn) 并反量化为 f32 行主 [pn, k]
    pub fn read_rows(&self, info: &TensorInfo, p0: usize, pn: usize) -> std::io::Result<Vec<f32>> {
        let raw = self.raw_rows(info, p0, pn)?;
        Ok(Self::dequant_rows_bytes(info, &raw, pn))
    }

    pub fn md_u32(&self, k: &str, d: u32) -> u32 {
        self.meta.get(k).and_then(|v| v.as_u32()).unwrap_or(d)
    }
    pub fn md_f64(&self, k: &str, d: f64) -> f64 {
        self.meta.get(k).and_then(|v| v.as_f64()).unwrap_or(d)
    }

    /// 读张量并反量化为 f32（行主 [rows, cols]）。
    /// GGUF 维度序：ne[0]=内维（列），ne[1]=行 → 2D shape=[cols, rows]
    pub fn tensor_f32(&self, name: &str) -> Option<Vec<f32>> {
        let info = self.tensors.get(name)?;
        let (rows, cols) = if info.shape.len() == 1 {
            (info.shape[0], 1)
        } else {
            (info.shape[1], info.shape[0])
        };
        let start = info.offset as usize;
        let mut out = Vec::with_capacity(rows * cols);
        let d = &self.data;
        if info.dtype.is_q8_0() {
            let blocks = cols.div_ceil(32);
            for r in 0..rows {
                let ro = start + r * blocks * 34;
                for b in 0..blocks {
                    let off = ro + b * 34;
                    let scale = f16_to_f32(d[off], d[off + 1]);
                    let n = 32.min(cols - b * 32);
                    for i in 0..n {
                        out.push(d[off + 2 + i] as i8 as f32 * scale);
                    }
                }
            }
        } else if info.dtype == GgmlDType::F32 {
            for i in 0..rows * cols {
                let off = start + i * 4;
                out.push(f32::from_le_bytes([
                    d[off],
                    d[off + 1],
                    d[off + 2],
                    d[off + 3],
                ]));
            }
        } else {
            return None;
        }
        Some(out)
    }
}
