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

/// 播放 tag 音效（失败静默）。
pub fn play(tag: &str) {
    let clip = with_clip(tag);
    let Some(c) = clip else { return };
    let block_align = c.channels * c.bits / 8;
    if block_align == 0 {
        return;
    }
    unsafe {
        let wfx = WAVEFORMATEX {
            wFormatTag: 1, // PCM
            nChannels: c.channels,
            nSamplesPerSec: c.samples_per_sec,
            wBitsPerSample: c.bits,
            nBlockAlign: block_align,
            nAvgBytesPerSec: c.samples_per_sec * block_align as u32,
            cbSize: 0,
        };
        let mut h: HWAVEOUT = HWAVEOUT(std::ptr::null_mut());
        if waveOutOpen(
            Some(&mut h),
            0xFFFFFFFF, // WAVE_MAPPER
            &wfx,
            0,
            0,
            CALLBACK_NULL,
        ) != 0 {
            return;
        }
        // 音量：0–100 → 0x0000–0xFFFF（左右声道同值）
        let v = (c.volume as u32).min(100) * 0xFFFF / 100;
        let _ = waveOutSetVolume(h, v | (v << 16));
        // 缓冲区需在播放期间存活：泄漏到 Box 并用 waveOutProc 回收过于复杂，
        // 这里用「分配不释放 + 短生命周期覆盖」不可取 —— 采用静态池。
        // 简化安全做法：播放前把数据拷入泄漏 Box（每个音 <10KB，且实际
        // 只在按键时触发；waveOutWrite 返回后 waveOutReset+Close 立即等待）。
        // 为避免泄漏：同步等待播放完成（音效 ≤80ms，可接受）。
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
            let _ = waveOutClose(h);
            return;
        }
        let _ = waveOutWrite(h, &mut hdr, std::mem::size_of::<WAVEHDR>() as u32);
        // 忙等 WHDR_DONE（音效极短）
        let mut spins = 0u32;
        while (hdr.dwFlags & 0x1) == 0 && spins < 200 {
            std::thread::sleep(std::time::Duration::from_millis(2));
            spins += 1;
        }
        let _ = waveOutReset(h);
        let _ = waveOutUnprepareHeader(h, &mut hdr, std::mem::size_of::<WAVEHDR>() as u32);
        let _ = waveOutClose(h);
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
