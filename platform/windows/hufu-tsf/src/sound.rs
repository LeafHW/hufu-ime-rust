//! 按键音：waveOut 播放（数据目录 sounds/<tag>.wav，经管道取回并缓存）。
//!
//! 流程：引擎在 KeyOutcome.sound 填 tag（key/select/commit/page）→
//! 管道 op sound {tag} 返回 {data(base64), volume} → 解码缓存 →
//! waveOutOpen(WAVE_MAPPER) + SetVolume + Write（异步放完自动静默）。

use std::collections::HashMap;
use std::sync::Mutex;
use windows::Win32::Media::Audio::*;

/// wav PCM 片段 + 音量 0–100。
struct Clip {
    samples: Vec<u8>,
    samples_per_sec: u32,
    channels: u16,
    bits: u16,
    volume: u8,
}

static CACHE: Mutex<Option<HashMap<String, Clip>>> = Mutex::new(None);
/// 预开 waveOut 句柄池（按格式一组 4 个，轮转使用）：
/// 省掉每次播放的 waveOutOpen/Close（实测各 ~5-15ms，是音效迟滞主因），
/// 4 句柄天然支持 4 路交叠（系统混音），连打音效即刻出声。
struct Pool {
    key: (u32, u16, u16),
    handles: Vec<HWAVEOUT>,
    next: usize,
}
// HWAVEOUT 是裸句柄（内部 *mut c_void），跨线程移动安全（waveOut API 线程无关）
unsafe impl Send for Pool {}
static POOLS: Mutex<Vec<Pool>> = Mutex::new(Vec::new());

fn take_handle(rate: u32, channels: u16, bits: u16) -> Option<HWAVEOUT> {
    let mut pools = POOLS.lock().unwrap_or_else(|p| p.into_inner());
    let key = (rate, channels, bits);
    let pool = match pools.iter_mut().find(|p| p.key == key) {
        Some(p) => p,
        None => {
            let block_align = channels * bits / 8;
            let wfx = WAVEFORMATEX {
                wFormatTag: 1,
                nChannels: channels,
                nSamplesPerSec: rate,
                wBitsPerSample: bits,
                nBlockAlign: block_align,
                nAvgBytesPerSec: rate * block_align as u32,
                cbSize: 0,
            };
            let mut handles = Vec::new();
            for _ in 0..4 {
                let mut h = HWAVEOUT(std::ptr::null_mut());
                let ok = unsafe {
                    waveOutOpen(Some(&mut h), 0xFFFFFFFF, &wfx, 0, 0, CALLBACK_NULL)
                };
                if ok == 0 && !h.0.is_null() {
                    handles.push(h);
                }
            }
            if handles.is_empty() {
                return None;
            }
            pools.push(Pool {
                key,
                handles,
                next: 0,
            });
            pools.last_mut().unwrap()
        }
    };
    let h = pool.handles[pool.next % pool.handles.len()];
    pool.next = pool.next.wrapping_add(1);
    Some(h)
}

/// 播放 tag 音效（失败静默）。**单常驻线程 + 深度 10 合并队列**：
/// 每次播放 spawn 线程的老方案在按住键连发（~30 键/秒）时线程堆积，
/// 几秒后调度拖垮——正是「按住 D 三五秒后卡」的病根。
/// 现在整进程只有一条 hufu-snd 线程顺序播放（4 句柄池内交叠出声）；
/// 队列满（≥10 未播）直接丢弃本次——连击音效宁可丢新不积压。
pub fn play(tag: &str) {
    let clip = match with_clip(tag) {
        Some(c) => c,
        None => return,
    };
    static TX: Mutex<Option<std::sync::mpsc::SyncSender<Clip>>> = Mutex::new(None);
    let mut g = TX.lock().unwrap_or_else(|p| p.into_inner());
    if g.is_none() {
        // 深度 10：连击不丢音（4 句柄池交叠消化，长队列顺序播完）
        let (tx, rx) = std::sync::mpsc::sync_channel::<Clip>(10);
        let ok = std::thread::Builder::new()
            .name("hufu-snd".into())
            .spawn(move || while let Ok(c) = rx.recv() { play_sync(c) })
            .is_ok();
        if !ok {
            return;
        }
        *g = Some(tx);
    }
    let _ = g.as_ref().unwrap().try_send(clip);
}

/// 同步播放（独立线程内调用）：预开句柄 + 单缓冲写 + 忙等到 DONE 归还。
fn play_sync(c: Clip) {
    let block_align = c.channels * c.bits / 8;
    if block_align == 0 {
        return;
    }
    let Some(h) = take_handle(c.samples_per_sec, c.channels, c.bits) else {
        return;
    };
    unsafe {
        // 音量：0–100 → 0x0000–0xFFFF（左右声道同值；句柄复用需每次设置）
        let v = (c.volume as u32).min(100) * 0xFFFF / 100;
        let _ = waveOutSetVolume(h, v | (v << 16));
        let mut hdr = WAVEHDR {
            lpData: windows_core::PSTR(c.samples.as_ptr() as *mut u8),
            dwBufferLength: c.samples.len() as u32,
            dwBytesRecorded: 0,
            dwUser: 0,
            dwFlags: 0,
            dwLoops: 0,
            lpNext: std::ptr::null_mut(),
            reserved: 0,
        };
        if waveOutPrepareHeader(h, &mut hdr, std::mem::size_of::<WAVEHDR>() as u32) != 0 {
            return;
        }
        let _ = waveOutWrite(h, &mut hdr, std::mem::size_of::<WAVEHDR>() as u32);
        // 等 WHDR_DONE：音效 160-175ms，但同句柄并发播放会排队（4 句柄池），
        // 上限必须容纳排队（8 连击 → 至多 2 深队列 ≈ 360ms）。超时则 Reset
        // 清队列（头标 DONE）再 Unprepare——否则栈上 WAVEHDR 被驱动继续写 → UAF 崩溃。
        let mut spins = 0u32;
        while (hdr.dwFlags & 0x1) == 0 && spins < 1000 {
            std::thread::sleep(std::time::Duration::from_millis(2));
            spins += 1;
        }
        if (hdr.dwFlags & 0x1) == 0 {
            let _ = waveOutReset(h);
        }
        let _ = waveOutUnprepareHeader(h, &mut hdr, std::mem::size_of::<WAVEHDR>() as u32);
        // 句柄保留复用，不 Close
    }
}

fn with_clip(tag: &str) -> Option<Clip> {
    let mut guard = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    let map = guard.get_or_insert_with(HashMap::new);
    if let Some(c) = map.get(tag) {
        return Some(Clip {
            samples: c.samples.clone(),
            samples_per_sec: c.samples_per_sec,
            channels: c.channels,
            bits: c.bits,
            volume: c.volume,
        });
    }
    // 管道取
    let resp = crate::ipc::call(&serde_json::json!({"op": "sound", "tag": tag}))?;
    let data_b64 = resp.get("data").and_then(|v| v.as_str())?;
    let volume = resp.get("volume").and_then(|v| v.as_u64()).unwrap_or(50) as u8;
    let raw = base64_decode(data_b64)?;
    let (samples, rate, channels, bits) = parse_wav(&raw)?;
    let clip = Clip {
        samples,
        samples_per_sec: rate,
        channels,
        bits,
        volume,
    };
    map.insert(
        tag.to_string(),
        Clip {
            samples: clip.samples.clone(),
            samples_per_sec: clip.samples_per_sec,
            channels: clip.channels,
            bits: clip.bits,
            volume: clip.volume,
        },
    );
    Some(clip)
}

/// 清空缓存（音量/文件变更后重取）。
pub fn invalidate() {
    let mut guard = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    *guard = None;
}

/// 极简 wav 解析：RIFF→fmt→data（PCM，任意声道/位深，waveOut 按实际格式开）。
fn parse_wav(raw: &[u8]) -> Option<(Vec<u8>, u32, u16, u16)> {
    if raw.len() < 44 || &raw[0..4] != b"RIFF" || &raw[8..12] != b"WAVE" {
        return None;
    }
    let mut rate = 44_100u32;
    let mut channels = 1u16;
    let mut bits = 16u16;
    let mut data: Option<Vec<u8>> = None;
    let mut pos = 12usize;
    while pos + 8 <= raw.len() {
        let id = &raw[pos..pos + 4];
        let size = u32::from_le_bytes([
            raw[pos + 4],
            raw[pos + 5],
            raw[pos + 6],
            raw[pos + 7],
        ]) as usize;
        let body = pos + 8;
        if id == b"fmt " && size >= 16 {
            channels = u16::from_le_bytes([raw[body + 2], raw[body + 3]]);
            rate = u32::from_le_bytes([
                raw[body + 4],
                raw[body + 5],
                raw[body + 6],
                raw[body + 7],
            ]);
            bits = u16::from_le_bytes([raw[body + 14], raw[body + 15]]);
        } else if id == b"data" {
            let end = (body + size).min(raw.len());
            data = Some(raw[body..end].to_vec());
        }
        pos = body + size + (size & 1);
    }
    if bits != 8 && bits != 16 {
        return None; // 仅 PCM 8/16bit
    }
    Some((data?, rate, channels.max(1), bits))
}

/// 标准 base64 解码（无填充容错）。
fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let table = |c: u8| -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a' + 26) as u32),
            b'0'..=b'9' => Some((c - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    };
    let mut out = Vec::new();
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for &c in s.as_bytes() {
        if c == b'=' || c == b'\n' || c == b'\r' {
            continue;
        }
        let v = table(c)?;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xFF) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_stereo_wav() {
        // 最小 RIFF：fmt(16, stereo 44100 16bit) + data(4 字节)
        let mut w = Vec::new();
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&36u32.to_le_bytes());
        w.extend_from_slice(b"WAVE");
        w.extend_from_slice(b"fmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes()); // PCM
        w.extend_from_slice(&2u16.to_le_bytes()); // stereo
        w.extend_from_slice(&44100u32.to_le_bytes());
        w.extend_from_slice(&176400u32.to_le_bytes());
        w.extend_from_slice(&4u16.to_le_bytes()); // align
        w.extend_from_slice(&16u16.to_le_bytes()); // bits
        w.extend_from_slice(b"data");
        w.extend_from_slice(&4u32.to_le_bytes());
        w.extend_from_slice(&[1, 2, 3, 4]);
        let (d, rate, ch, bits) = parse_wav(&w).unwrap();
        assert_eq!(d, vec![1, 2, 3, 4]);
        assert_eq!((rate, ch, bits), (44100, 2, 16));
    }

    #[test]
    fn parse_rejects_non_pcm_bit_depth() {
        let mut w = Vec::new();
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&36u32.to_le_bytes());
        w.extend_from_slice(b"WAVE");
        w.extend_from_slice(b"fmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&44100u32.to_le_bytes());
        w.extend_from_slice(&88200u32.to_le_bytes());
        w.extend_from_slice(&2u16.to_le_bytes());
        w.extend_from_slice(&24u16.to_le_bytes()); // 24bit 不支持
        w.extend_from_slice(b"data");
        w.extend_from_slice(&4u32.to_le_bytes());
        w.extend_from_slice(&[1, 2, 3, 4]);
        assert!(parse_wav(&w).is_none());
    }
}
