//! 按键音：waveOut 播放（数据目录 sounds/<tag>.wav，经管道取回并缓存）。
//!
//! 流程：引擎在 KeyOutcome.sound 填 tag（key/select/commit/page）→
//! 管道 op sound {tag} 返回 {data(base64), volume} → 解码缓存 →
//! waveOutOpen(WAVE_MAPPER) + SetVolume + Write（异步放完自动静默）。

use std::collections::HashMap;
use std::sync::Mutex;
use windows::Win32::Media::Audio::*;

/// wav PCM 片段 + 音量 0–100。
#[derive(Clone)]
struct Clip {
    samples: Vec<u8>,
    samples_per_sec: u32,
    channels: u16,
    bits: u16,
    volume: u8,
}

static CACHE: Mutex<Option<HashMap<String, Clip>>> = Mutex::new(None);
/// 预开 waveOut 句柄池（按格式一组 8 个，轮转使用）：
/// 省掉每次播放的 waveOutOpen/Close（实测各 ~5-15ms，是音效迟滞主因），
/// 8 句柄支持 8 路并行交叠（系统混音），连打音效即刻出声。
struct Pool {
    key: (u32, u16, u16),
    handles: Vec<HWAVEOUT>,
    next: usize,
}
// HWAVEOUT 是裸句柄（内部 *mut c_void），跨线程移动安全（waveOut API 线程无关）
unsafe impl Send for Pool {}
static POOLS: Mutex<Vec<Pool>> = Mutex::new(Vec::new());

/// 正在出声的（句柄, 是否键音）注册表：key_up() 只截断键音，
/// 选字/上屏等事件音不受松键影响（否则 space 一松 commit 音就被切没）。
struct ActiveHandle(HWAVEOUT, bool);
// HWAVEOUT 裸句柄跨线程使用安全（waveOut API 线程无关，同 Pool）
unsafe impl Send for ActiveHandle {}
static ACTIVE: Mutex<Vec<ActiveHandle>> = Mutex::new(Vec::new());

/// 键松开：截断所有正在响的键音（按下出声、松开即停的打字机手感）。
/// waveOutReset 使头标立即 DONE → 播放线程忙等退出、归还句柄。
pub fn key_up() {
    let act = ACTIVE.lock().unwrap_or_else(|p| p.into_inner());
    for ah in act.iter() {
        if ah.1 {
            unsafe {
                let _ = waveOutReset(ah.0);
            }
        }
    }
}

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
            for _ in 0..8 {
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

/// 播放 tag 音效（失败静默）。**调度线程 + 4 固定播放工人**：
/// 每键 spawn 的最初版听感最好（4 句柄交叠、每个键都出声）但连发
/// 线程堆积会卡；单线程顺序播放版不卡但 ~6 声/秒连打漏音。本版
/// 4 工人并发（与最初版交叠密度一致）+ 线程恒定（连发不卡），
/// 全忙且队列满才丢音。
/// 播放任务：clip + 是否键音（键音可被 key_up() 截断）。
struct Job {
    clip: Clip,
    is_key: bool,
}

/// 伪随机（splitmix64 步进）：键音随机挑 clip、随机音量抖动用。
fn rnd() -> u64 {
    static S: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E3779B9)
        | 1;
    let _ = S.compare_exchange(
        0,
        t,
        std::sync::atomic::Ordering::Relaxed,
        std::sync::atomic::Ordering::Relaxed,
    );
    let mut z = S.fetch_add(0x9E3779B97F4A7C15, std::sync::atomic::Ordering::Relaxed);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

/// 键音候选池：音效目录全部事件音（key/select/commit/page 皆为短促
/// 击键声）——每次按键随机挑一个 + 音量 ±15% 抖动，杜绝「固定音」。
fn key_pool_tags() -> Vec<String> {
    static TAGS: Mutex<Option<Vec<String>>> = Mutex::new(None);
    let mut g = TAGS.lock().unwrap_or_else(|p| p.into_inner());
    if g.is_none() {
        let mut v: Vec<String> = ["key", "select", "commit", "page"]
            .iter()
            .filter(|t| with_clip(t).is_some())
            .map(|t| t.to_string())
            .collect();
        if v.is_empty() {
            v.push("key".to_string());
        }
        *g = Some(v);
    }
    g.clone().unwrap()
}

/// 播放 tag 音效（失败静默）。**调度线程 + 8 固定播放工人**：
/// tag=="key" 走键音新模型：随机 clip + 音量抖动、可被 key_up() 截断
/// （按下出声松开即停）；其余（select/commit/page）为事件音不受松键影响。
/// vol = server 每键随行的当前音量（0-100，热生效）——wav 数据可缓存，
/// 音量永远用最新值，设置页改音量下一键即生效。
pub fn play(tag: &str, vol: u8) {
    let is_key = tag == "key";
    let clip = if is_key {
        let tags = key_pool_tags();
        let pick = &tags[(rnd() % tags.len() as u64) as usize];
        let mut c = match with_clip(pick) {
            Some(c) => c,
            None => return,
        };
        c.volume = vol;
        // 音量抖动 ±15%：同一 wav 也不重样
        let jitter = 85 + (rnd() % 31) as u32; // 85–115
        c.volume = ((c.volume as u32 * jitter) / 100).min(100) as u8;
        c
    } else {
        match with_clip(tag) {
            Some(mut c) => {
                c.volume = vol;
                c
            }
            None => return,
        }
    };
    static TX: Mutex<Option<std::sync::mpsc::SyncSender<Job>>> = Mutex::new(None);
    let mut g = TX.lock().unwrap_or_else(|p| p.into_inner());
    if g.is_none() {
        // 缓冲 10 只为吸收突发；播放侧「排空取最新」：起播前把队列里
        // 攒的全部倒掉只播最新一条——连打期间声音连续不中断（队列非空），
        // 停键后至多再播一条（尾巴 ≤1 个音，不再拖一串余音）。
        let (tx, rx) = std::sync::mpsc::sync_channel::<Job>(32);
        let ok = std::thread::Builder::new()
            .name("hufu-snd-disp".into())
            .spawn(move || {
                // 8 条固定播放工人，轮转分发；单工人队列满(4)跳下一条，
                // 全满才丢——8 路并行交叠、线程恒定不堆积。
                let mut wtx = Vec::with_capacity(8);
                for i in 0..8 {
                    let (wtx_i, wrx) = std::sync::mpsc::sync_channel::<Job>(4);
                    if std::thread::Builder::new()
                        .name(format!("hufu-snd-{i}"))
                        .spawn(move || {
                            while let Ok(j) = wrx.recv() {
                                play_sync(j.clip, j.is_key);
                            }
                        })
                        .is_ok()
                    {
                        wtx.push(wtx_i);
                    }
                }
                if wtx.is_empty() {
                    return;
                }
                let n = wtx.len();
                let mut next = 0usize;
                while let Ok(j) = rx.recv() {
                    for k in 0..n {
                        let idx = (next + k) % n;
                        if wtx[idx]
                            .try_send(Job {
                                clip: j.clip.clone(),
                                is_key: j.is_key,
                            })
                            .is_ok()
                        {
                            next = (idx + 1) % n;
                            break;
                        }
                    }
                }
            })
            .is_ok();
        if !ok {
            return;
        }
        *g = Some(tx);
    }
    let _ = g.as_ref().unwrap().try_send(Job { clip, is_key });
}

/// 同步播放（工人线程内调用）：预开句柄 + 单缓冲写 + 忙等到 DONE 归还。
/// 句柄在写入前登记 ACTIVE（key_up 据此截断键音），归还前注销。
fn play_sync(c: Clip, is_key: bool) {
    let block_align = c.channels * c.bits / 8;
    if block_align == 0 {
        return;
    }
    let Some(h) = take_handle(c.samples_per_sec, c.channels, c.bits) else {
        return;
    };
    {
        let mut act = ACTIVE.lock().unwrap_or_else(|p| p.into_inner());
        act.push(ActiveHandle(h, is_key));
    }
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
            unregister_active(h);
            return;
        }
        let _ = waveOutWrite(h, &mut hdr, std::mem::size_of::<WAVEHDR>() as u32);
        // 等 WHDR_DONE：键音 160-175ms；key_up() 的 waveOutReset 会把头标
        // 置 DONE 提前出循环（松开即停）。超时兜底 Reset 防 UAF：栈上
        // WAVEHDR 若仍被驱动写，函数返回后就是悬垂指针。
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
    unregister_active(h);
}

/// 从 ACTIVE 注销句柄（播放完成/Prepare 失败时）。
fn unregister_active(h: HWAVEOUT) {
    let mut act = ACTIVE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(pos) = act.iter().position(|ah| ah.0 == h) {
        act.swap_remove(pos);
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
