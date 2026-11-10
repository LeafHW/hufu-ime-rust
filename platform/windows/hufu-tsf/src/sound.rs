//! 按键音：waveOut 播放（数据目录 sounds/<tag>.wav，经管道取回并缓存）。
//!
//! 流程：引擎在 KeyOutcome.sound 填 tag（key/select/commit/page）→
//! 管道 op sound {tag} 返回 {data(base64), volume} → 解码缓存 →
//! waveOutOpen(WAVE_MAPPER) + SetVolume + Write（异步放完自动静默）。

use std::collections::HashMap;
use std::sync::Mutex;
use windows::Win32::Media::Audio::*;

/// wav PCM 片段 + 音量 0–100。
/// 【音效减负 2026-09-08】samples 用 Arc 共享——旧版每次播放 clone
/// 整块 PCM（键音 ~30-60KB/键），高速跟打每秒数百 KB 无谓分配。
#[derive(Clone)]
struct Clip {
    samples: std::sync::Arc<Vec<u8>>,
    samples_per_sec: u32,
    channels: u16,
    bits: u16,
    volume: u8,
    generation: u64,
}

static CACHE: Mutex<Option<HashMap<String, Clip>>> = Mutex::new(None);
/// 清缓存与异步取料以代际隔离：旧包的迟到响应不能重新污染新缓存。
static CACHE_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// 键音池标签缓存（原为 key_pool_tags 函数内 static；提到模块级以便
/// invalidate() 在换音效包/音量变更时一并失效——池成员来自音效目录
/// 实测，旧池在换包后可能整包取不到 → 键音静默）。
static KEY_POOL_TAGS: Mutex<Option<Vec<String>>> = Mutex::new(None);
/// 预开 waveOut 句柄池（按格式一组 8 个，轮转使用）：
/// 省掉每次播放的 waveOutOpen/Close（实测各 ~5-15ms，是音效迟滞主因），
/// 8 句柄支持 8 路并行交叠（系统混音），连打音效即刻出声。
struct Pool {
    key: (u32, u16, u16),
    handles: Vec<HWAVEOUT>,
    busy: Vec<bool>,
}
impl Pool {
    fn take_free(&mut self) -> Option<HWAVEOUT> {
        let index = self.busy.iter().position(|busy| !busy)?;
        self.busy[index] = true;
        Some(self.handles[index])
    }

    fn release(&mut self, h: HWAVEOUT) -> bool {
        let Some(index) = self.handles.iter().position(|handle| *handle == h) else {
            return false;
        };
        self.busy[index] = false;
        true
    }
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
            let (block_align, byte_rate) = pcm_format(rate, channels, bits)?;
            let wfx = WAVEFORMATEX {
                wFormatTag: 1,
                nChannels: channels,
                nSamplesPerSec: rate,
                wBitsPerSample: bits,
                nBlockAlign: block_align,
                nAvgBytesPerSec: byte_rate,
                cbSize: 0,
            };
            let mut handles = Vec::new();
            // 【音效恢复 8 路 2026-09-08】减负期（3 路）曾为排查卡顿所
            // 设——现已实锤卡顿根因是 32 位老 DLL（与音效无关），按用户
            // 要求恢复 8 路交叠原版听感/行为。
            for _ in 0..8 {
                let mut h = HWAVEOUT(std::ptr::null_mut());
                let ok =
                    unsafe { waveOutOpen(Some(&mut h), 0xFFFFFFFF, &wfx, 0, 0, CALLBACK_NULL) };
                if ok == 0 && !h.0.is_null() {
                    handles.push(h);
                }
            }
            if handles.is_empty() {
                return None;
            }
            pools.push(Pool {
                key,
                busy: vec![false; handles.len()],
                handles,
            });
            pools.last_mut().unwrap()
        }
    };
    // 工人数量有限不代表轮转句柄已经空闲：一个长音未结束时，其他工人
    // 可连续播完多段短音绕回它。只租借空闲句柄，全忙则丢本次音效。
    pool.take_free()
}

/// 播放 tag 音效（失败静默）。**调度线程 + 4 固定播放工人**：
/// 每键 spawn 的最初版听感最好（4 句柄交叠、每个键都出声）但连发
/// 线程堆积会卡；单线程顺序播放版不卡但 ~6 声/秒连打漏音。本版
/// 4 工人并发（与最初版交叠密度一致）+ 线程恒定（连发不卡），
/// 全忙且队列满才丢音。
/// 播放任务：clip + 是否键音（键音可被 key_up() 截断）。
/// 【T1-2a 2026-11-07】新增两种后台任务，跑在既有播放工人线程：
/// Fetch=按键线程 cache miss 时交接的 tag 取料（管道取+解码+入缓存，
/// 本键静音跳过、下一键命中即有声）；WarmPool=冷缓存/invalidate 后
/// 探明键音池成员——都不占 UI/按键线程。
enum Job {
    Play { clip: Clip, is_key: bool },
    Fetch(String),
    WarmPool,
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
    {
        let g = KEY_POOL_TAGS.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(v) = g.as_ref() {
            return v.clone();
        }
    }
    // 【T1-2a 2026-11-07】冷缓存不再在按键线程连查 4 个 tag（旧实现
    // 每查=BG 管道取料+解码，上限 150ms×4 卡按键线程）——池成员交
    // 工人线程后台探明（warm_pool），本键先回退 "key" 立即返回；
    // "key" 的 cache miss 同样只触发后台预取，本键静音不阻塞。
    warm_pool_async();
    vec!["key".to_string()]
}

/// 当前音效包名（server 键响应随行；变更即清缓存按新包重取）。
/// 【音效包 2026-10-30】空串 = 根目录平铺（旧模式）。langbar 5s 快照
/// 比对兜底（覆盖 config.json 手改/云同步拉取等不经键响应的变更）。
static CURRENT_PACK: Mutex<String> = Mutex::new(String::new());
static CONFIG_ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static CONFIG_VOLUME: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(70);

/// Pass-through keys may never reach OnKeyDown. Reuse the existing background
/// sound-state snapshot rather than adding IPC to OnTestKeyDown.
pub fn set_settings(enabled: bool, volume: u8, pack: &str) {
    set_enabled(enabled);
    CONFIG_VOLUME.store(volume.min(100), std::sync::atomic::Ordering::Relaxed);
    set_pack(pack);
}

pub fn set_enabled(enabled: bool) {
    if CONFIG_ENABLED.swap(enabled, std::sync::atomic::Ordering::Relaxed) != enabled {
        invalidate();
    }
}

pub fn configured_key_sound() -> Option<(u8, String)> {
    CONFIG_ENABLED
        .load(std::sync::atomic::Ordering::Relaxed)
        .then(|| {
            (
                CONFIG_VOLUME.load(std::sync::atomic::Ordering::Relaxed),
                CURRENT_PACK
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone(),
            )
        })
}

/// Only the requested typing keys. Navigation and shortcut chords stay silent.
pub fn additional_typing_key(vk: usize, ctrl: bool, alt: bool, meta: bool) -> bool {
    !ctrl && !alt && !meta && matches!(vk, 0x08 | 0x20 | 0x30..=0x39 | 0x60..=0x69)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct KeySoundStamp(usize, usize, i32);

/// A TSF test callback is a query and can be repeated for one queued key message.
/// Its real callback uses the same message timestamp/scan data. This gate only
/// deduplicates sound; it never changes whether the key is eaten or forwarded.
#[derive(Default)]
pub struct InputSoundGate {
    test: Option<KeySoundStamp>,
    real: Option<KeySoundStamp>,
}

impl InputSoundGate {
    pub fn test(&mut self, vk: usize, lparam: usize, message_time: i32) -> bool {
        let stamp = KeySoundStamp(vk, lparam, message_time);
        let fresh = self.test != Some(stamp) && self.real != Some(stamp);
        self.test = Some(stamp);
        fresh
    }

    pub fn real(&mut self, vk: usize, lparam: usize, message_time: i32) -> bool {
        let stamp = KeySoundStamp(vk, lparam, message_time);
        let fresh = self.test != Some(stamp) && self.real != Some(stamp);
        self.real = Some(stamp);
        fresh
    }

    pub fn release(&mut self, vk: usize) {
        if self.test.is_some_and(|stamp| stamp.0 == vk) {
            self.test = None;
        }
        if self.real.is_some_and(|stamp| stamp.0 == vk) {
            self.real = None;
        }
    }
}

/// 包名变更检测：不同→invalidate()+记新值；相同→no-op（零成本）。
pub fn set_pack(pack: &str) {
    let mut g = CURRENT_PACK.lock().unwrap_or_else(|p| p.into_inner());
    if g.as_str() != pack {
        invalidate();
        *g = pack.to_string();
    }
}

/// 播放 tag 音效（失败静默）。**调度线程 + 8 固定播放工人**：
/// tag=="key" 走键音新模型：随机 clip + 音量抖动、可被 key_up() 截断
/// （按下出声松开即停）；其余（select/commit/page）为事件音不受松键影响。
/// vol = server 每键随行的当前音量（0-100，热生效）——wav 数据可缓存，
/// 音量永远用最新值，设置页改音量下一键即生效。
/// pack = server 随行音效包名：变更即失效缓存（设置页换包下一键即新音）。
pub fn play(tag: &str, vol: u8, pack: &str) {
    set_pack(pack);
    let is_key = tag == "key";
    let pick: String = if is_key {
        let tags = key_pool_tags();
        tags[(rnd() % tags.len() as u64) as usize].clone()
    } else {
        tag.to_string()
    };
    // 【T1-2a 2026-11-07】cache miss 不在按键线程同步取料（旧 with_clip
    // 持 CACHE 锁同步走 BG 管道，冷缓存最长 150ms/Tag 卡按键线程）：
    // tag 交播放工人线程后台取（下一键命中即有声），本键静音跳过。
    let Some(mut c) = cached_clip(&pick) else {
        prefetch_async(&pick);
        return;
    };
    if is_key {
        c.volume = vol;
        // 音量抖动 ±15%：同一 wav 也不重样
        let jitter = 85 + (rnd() % 31) as u32; // 85–115
        c.volume = ((c.volume as u32 * jitter) / 100).min(100) as u8;
    } else {
        c.volume = vol;
    }
    let Some(tx) = dispatch_tx() else {
        return;
    };
    let _ = tx.try_send(Job::Play { clip: c, is_key });
}

/// 播放调度器（懒启动）+ 8 固定播放工人组的发送端。
/// 【T1-2a 2026-11-07】从 play() 抽出：取料（Fetch）/池预热（WarmPool）
/// 任务投同一组工人线程，不再另起线程。
fn dispatch_tx() -> Option<std::sync::mpsc::SyncSender<Job>> {
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
                // 8 条固定播放工人（与句柄池 8 对齐），轮转分发；单工人
                // 队列满(4)跳下一条，全满才丢——8 路并行交叠、线程恒定
                // 不堆积。（3 路减负期已过：卡顿根因实锤为 32 位老
                // DLL，与音效无关，恢复原版。）
                let mut wtx = Vec::with_capacity(8);
                for i in 0..8 {
                    let (wtx_i, wrx) = std::sync::mpsc::sync_channel::<Job>(4);
                    if std::thread::Builder::new()
                        .name(format!("hufu-snd-{i}"))
                        .spawn(move || {
                            while let Ok(j) = wrx.recv() {
                                match j {
                                    Job::Play { clip, is_key } => play_sync(clip, is_key),
                                    // 【T1-2a 2026-11-07】取料/池预热与播放
                                    // 同池：占一路工人 ≤150ms/Tag（BG 管道
                                    // 超时上限），其余工人照常出声。
                                    Job::Fetch(tag) => {
                                        let _ = fetch_clip(&tag);
                                    }
                                    Job::WarmPool => warm_pool(),
                                }
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
                        // 【T1-2a 2026-11-07】任务 enum 化后逐次构造转发
                        //（Play 克隆重试语义与旧 struct 版一致）。
                        let fwd = match &j {
                            Job::Play { clip, is_key } => Job::Play {
                                clip: clip.clone(),
                                is_key: *is_key,
                            },
                            Job::Fetch(t) => Job::Fetch(t.clone()),
                            Job::WarmPool => Job::WarmPool,
                        };
                        if wtx[idx].try_send(fwd).is_ok() {
                            next = (idx + 1) % n;
                            break;
                        }
                    }
                }
            })
            .is_ok();
        if !ok {
            return None;
        }
        *g = Some(tx);
    }
    g.as_ref().cloned()
}

/// 同步播放（工人线程内调用）：预开句柄 + 单缓冲写 + 忙等到 DONE 归还。
/// 句柄在写入前登记 ACTIVE（key_up 据此截断键音），归还前注销。
fn play_sync(c: Clip, is_key: bool) {
    if c.generation != CACHE_GENERATION.load(std::sync::atomic::Ordering::Relaxed) {
        return; // 换包前排队的播放任务也随缓存一起失效
    }
    if pcm_format(c.samples_per_sec, c.channels, c.bits).is_none() {
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
            retire_handle(h, c.samples_per_sec, c.channels, c.bits);
            return;
        }
        if waveOutWrite(h, &mut hdr, std::mem::size_of::<WAVEHDR>() as u32) != 0 {
            // 【设备热插拔自愈 2026-09-11】写失败≈输出设备移除/默认设备
            // 切换——旧实现静默失败到进程结束（音效从此无声）。退役死
            // 句柄：Close + 从池剔除；池空删条目，下次播放 take_handle
            // 全新重开（拿到新默认设备）。
            let _ = waveOutReset(h);
            let _ = waveOutUnprepareHeader(h, &mut hdr, std::mem::size_of::<WAVEHDR>() as u32);
            unregister_active(h);
            retire_handle(h, c.samples_per_sec, c.channels, c.bits);
            return;
        }
        // 等 WHDR_DONE：键音 160-175ms；key_up() 的 waveOutReset 会把头标
        // 置 DONE 提前出循环（松开即停）。超时兜底 Reset 防 UAF：栈上
        // WAVEHDR 若仍被驱动写，函数返回后就是悬垂指针。
        // 【音效恢复 2ms 粒度 2026-09-08】10ms 粒度曾为排查卡顿所设；
        // 根因实锤为 32 位老 DLL（与音效无关），恢复原版 2ms 响应度
        //（key_up 截断零迟滞）。
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
    release_handle(h);
}

/// 成功/超时播放结束后归还租借；失败路径直接 retire，不再进入池。
fn release_handle(h: HWAVEOUT) {
    let mut pools = POOLS.lock().unwrap_or_else(|p| p.into_inner());
    for pool in pools.iter_mut() {
        if pool.release(h) {
            return;
        }
    }
}

/// 从 ACTIVE 注销句柄（播放完成/Prepare 失败时）。
fn unregister_active(h: HWAVEOUT) {
    let mut act = ACTIVE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(pos) = act.iter().position(|ah| ah.0 == h) {
        act.swap_remove(pos);
    }
}

/// 退役失效句柄（设备热插拔）：Close 死句柄并从对应格式池剔除；
/// 池空则删整条目（下次 take_handle 重建拿新设备）。
fn retire_handle(h: HWAVEOUT, rate: u32, channels: u16, bits: u16) {
    unsafe {
        let _ = waveOutClose(h);
    }
    let mut pools = POOLS.lock().unwrap_or_else(|p| p.into_inner());
    let key = (rate, channels, bits);
    if let Some(pool) = pools.iter_mut().find(|p| p.key == key) {
        if let Some(index) = pool.handles.iter().position(|handle| *handle == h) {
            pool.handles.swap_remove(index);
            pool.busy.swap_remove(index);
        }
        if pool.handles.is_empty() {
            pools.retain(|p| p.key != key);
        }
    }
}

/// 命中查询：CACHE 锁内纯 map 查（短持）。【T1-2b 2026-11-07】旧
/// with_clip 持 CACHE 锁同步走 BG 管道（取料+解码+入缓存全在锁内）
/// ——冷缓存时按键线程最长 150ms/Tag 卡键且其他线程查缓存全体排队；
/// 拆成「锁内查 / 锁外取」两半，锁永不跨管道调用。
fn cached_clip(tag: &str) -> Option<Clip> {
    let guard = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    guard.as_ref()?.get(tag).cloned()
}

/// 取料（工人线程内调用）：管道取+base64 解码+wav 解析在锁外，插入
/// 缓存短持锁。【T1-2b 2026-11-07】锁不跨管道调用——工人取料期间
/// 按键线程查缓存/派播放照常，互不阻塞。
fn fetch_clip(tag: &str) -> Option<Clip> {
    if let Some(c) = cached_clip(tag) {
        return Some(c); // 排队期间他人已取好：直接复用
    }
    let generation = CACHE_GENERATION.load(std::sync::atomic::Ordering::Relaxed);
    let resp = crate::ipc::call(&serde_json::json!({"op": "sound", "tag": tag}))?;
    let data_b64 = resp.get("data").and_then(|v| v.as_str())?;
    let volume = resp.get("volume").and_then(|v| v.as_u64()).unwrap_or(50) as u8;
    let raw = base64_decode(data_b64)?;
    let (samples, rate, channels, bits) = parse_wav(&raw)?;
    let clip = Clip {
        samples: samples.into(),
        samples_per_sec: rate,
        channels,
        bits,
        volume,
        generation,
    };
    {
        let mut guard = CACHE.lock().unwrap_or_else(|p| p.into_inner());
        if generation != CACHE_GENERATION.load(std::sync::atomic::Ordering::Relaxed) {
            return None; // 取料期间换包，丢弃旧响应
        }
        let map = guard.get_or_insert_with(HashMap::new);
        map.insert(tag.to_string(), clip.clone());
    }
    Some(clip)
}

/// 【T1-2a 2026-11-07】把取料任务丢给播放工人线程（队列满/线程组起不
/// 来则丢弃——下一键 miss 会再触发，最终一致）。
fn prefetch_async(tag: &str) {
    if let Some(tx) = dispatch_tx() {
        let _ = tx.try_send(Job::Fetch(tag.to_string()));
    }
}

/// 【T1-2c 2026-11-07】后台预热键音池（开关翻转/换包 invalidate 后由
/// 工人线程执行，不占 UI/按键线程）。
fn warm_pool_async() {
    if let Some(tx) = dispatch_tx() {
        let _ = tx.try_send(Job::WarmPool);
    }
}

/// 池成员探明（工人线程内调用）：与旧 key_pool_tags 同口径——4 个
/// 事件 tag 逐个取料入缓存，取得到的进池；全空回退 "key"。
/// 【T1-2c 2026-11-07】原在按键线程同步做，现整体搬到后台。
fn warm_pool() {
    let generation = CACHE_GENERATION.load(std::sync::atomic::Ordering::Relaxed);
    let mut v: Vec<String> = ["key", "select", "commit", "page"]
        .iter()
        .filter(|t| fetch_clip(t).is_some())
        .map(|t| t.to_string())
        .collect();
    if v.is_empty() {
        v.push("key".to_string());
    }
    // 与 invalidate 相同锁序，检查到发布之间不允许跨代际。
    let _cache = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    if generation == CACHE_GENERATION.load(std::sync::atomic::Ordering::Relaxed) {
        let mut g = KEY_POOL_TAGS.lock().unwrap_or_else(|p| p.into_inner());
        *g = Some(v);
    }
}

/// 清空缓存（音量/文件变更后重取）。
pub fn invalidate() {
    {
        let mut guard = CACHE.lock().unwrap_or_else(|p| p.into_inner());
        CACHE_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        *guard = None;
        // 【P2-8 2026-11-07】键音池标签一并失效：换音效包后旧池标签可能
        // 在新包里全部取不到 → 键音整包静默。
        let mut tags = KEY_POOL_TAGS.lock().unwrap_or_else(|p| p.into_inner());
        *tags = None;
    }
    // 【T1-2c 2026-11-07】失效后下一键不再重付冷缓存（旧实现首键在
    // 按键线程连查 4 tag 各 ≤150ms）：常用 tag+池成员交工人线程后台
    // 预取，UI/按键线程零阻塞（langbar 开关翻转/换包同样受益）。
    // 【防回归修复(09-reg P3-3) 2026-11-07】预热挪出两锁外——首启工
    // 人线程毫秒级，不该占 CACHE/KEY_POOL_TAGS 锁（锁序单向不变）。
    warm_pool_async();
}

/// 极简 wav 解析：RIFF→fmt→data（PCM，任意声道/位深，waveOut 按实际格式开）。
fn pcm_format(rate: u32, channels: u16, bits: u16) -> Option<(u16, u32)> {
    if rate == 0 || channels == 0 || (bits != 8 && bits != 16) {
        return None;
    }
    let block_align = channels.checked_mul(bits / 8)?;
    Some((block_align, rate.checked_mul(block_align as u32)?))
}

fn parse_wav(raw: &[u8]) -> Option<(Vec<u8>, u32, u16, u16)> {
    if raw.len() < 44 || &raw[0..4] != b"RIFF" || &raw[8..12] != b"WAVE" {
        return None;
    }
    let mut format = None;
    let mut data: Option<Vec<u8>> = None;
    let mut pos = 12usize;
    while pos + 8 <= raw.len() {
        let id = &raw[pos..pos + 4];
        let size =
            u32::from_le_bytes([raw[pos + 4], raw[pos + 5], raw[pos + 6], raw[pos + 7]]) as usize;
        let body = pos + 8;
        // 先校验整个 chunk；尤其 32 位上 body + u32::MAX 不能先做裸加。
        let end = body.checked_add(size)?;
        if end > raw.len() {
            return None;
        }
        if id == b"fmt " {
            // 【越界修复 2026-09-09】截断/损坏 wav（fmt 头声称 16 字节但
            // 文件不够）原样读 raw[body+14] 越界 panic——本函数在宿主
            // UI 线程同步调用，panic=宿主进程崩溃。读前验界。
            if size < 16 || u16::from_le_bytes([raw[body], raw[body + 1]]) != 1 {
                return None;
            }
            let channels = u16::from_le_bytes([raw[body + 2], raw[body + 3]]);
            let rate = u32::from_le_bytes([raw[body + 4], raw[body + 5], raw[body + 6], raw[body + 7]]);
            let bits = u16::from_le_bytes([raw[body + 14], raw[body + 15]]);
            pcm_format(rate, channels, bits)?;
            format = Some((rate, channels, bits));
        } else if id == b"data" {
            data = Some(raw[body..end].to_vec());
        }
        // pos 递增防溢出回绕（size 伪造巨大值→usize 回绕→死循环/越界）
        match body.checked_add(size).and_then(|v| v.checked_add(size & 1)) {
            Some(next) if next > pos => pos = next,
            _ => break,
        }
    }
    let (rate, channels, bits) = format?;
    Some((data?, rate, channels, bits))
}

/// 标准 base64 解码（无填充容错）。overlaywin 贴图挂件同款复用。
pub(crate) fn base64_decode(s: &str) -> Option<Vec<u8>> {
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
    fn typing_sound_coverage_excludes_navigation_and_shortcuts() {
        for vk in [0x08, 0x20, 0x30, 0x39, 0x60, 0x69] {
            assert!(additional_typing_key(vk, false, false, false));
            assert!(!additional_typing_key(vk, true, false, false));
            assert!(!additional_typing_key(vk, false, true, false));
            assert!(!additional_typing_key(vk, false, false, true));
        }
        for vk in [0x09, 0x0d, 0x10, 0x11, 0x12, 0x25, 0x2e, 0x41, 0x6a, 0x70] {
            assert!(!additional_typing_key(vk, false, false, false));
        }
    }

    #[test]
    fn repeated_test_queries_and_real_callback_sound_only_once() {
        let mut gate = InputSoundGate::default();
        let lp = 0x0002_0001;
        assert!(gate.test(0x31, lp, 100));
        assert!(!gate.test(0x31, lp, 100));
        assert!(!gate.real(0x31, lp, 100));
        assert!(!gate.real(0x31, lp, 100));
        gate.release(0x31);
        assert!(gate.test(0x31, lp, 200));
        assert!(!gate.real(0x31, lp, 200));
    }

    #[test]
    fn test_only_and_real_only_hosts_keep_every_new_press_and_repeat() {
        let mut gate = InputSoundGate::default();
        let lp = 0x000e_0001;
        assert!(gate.test(0x08, lp, 100));
        assert!(gate.test(0x08, lp | 0x4000_0000, 133));
        assert!(gate.test(0x08, lp | 0x4000_0000, 166));
        // A lost up or a host without real callbacks cannot suppress the next key.
        assert!(gate.test(0x08, lp, 300));
        assert!(gate.real(0x20, 0x0039_0001, 400));
        assert!(!gate.real(0x20, 0x0039_0001, 400));
        assert!(gate.real(0x20, 0x4039_0001, 433));
        gate.release(0x20);
        assert!(gate.real(0x20, 0x0039_0001, 433));
    }

    #[test]
    fn long_playback_keeps_its_handle_until_released() {
        // 只测句柄分配状态，不打开音频设备。一个长音占住 h1 时，其余
        // 工人重复播短音，不论轮转多少次都只能复用已归还的 h2。
        let h1 = HWAVEOUT(1usize as *mut core::ffi::c_void);
        let h2 = HWAVEOUT(2usize as *mut core::ffi::c_void);
        let mut pool = Pool {
            key: (44100, 1, 16),
            handles: vec![h1, h2],
            busy: vec![false, false],
        };
        assert_eq!(pool.take_free(), Some(h1));
        for _ in 0..16 {
            assert_eq!(pool.take_free(), Some(h2));
            assert!(pool.take_free().is_none(), "忙句柄不能重复借出");
            assert!(pool.release(h2));
        }
        assert!(pool.release(h1));
        assert_eq!(pool.take_free(), Some(h1));
    }

    fn stereo_wav() -> Vec<u8> {
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
        w
    }

    #[test]
    fn parse_stereo_wav() {
        let w = stereo_wav();
        let (d, rate, ch, bits) = parse_wav(&w).unwrap();
        assert_eq!(d, vec![1, 2, 3, 4]);
        assert_eq!((rate, ch, bits), (44100, 2, 16));
    }

    #[test]
    fn parse_rejects_corrupt_pcm_headers_and_chunk_bounds() {
        let mut non_pcm = stereo_wav();
        non_pcm[20..22].copy_from_slice(&2u16.to_le_bytes());
        assert!(parse_wav(&non_pcm).is_none(), "非 PCM 数据不能当 PCM 播放");

        let mut huge_chunk = stereo_wav();
        huge_chunk[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_wav(&huge_chunk).is_none(), "32 位也不能溢出或接受截断 data");

        let mut no_fmt = stereo_wav();
        no_fmt[12..16].copy_from_slice(b"JUNK");
        assert!(parse_wav(&no_fmt).is_none(), "缺少 fmt 不能猜测格式");

        let mut zero_channels = stereo_wav();
        zero_channels[22..24].copy_from_slice(&0u16.to_le_bytes());
        assert!(parse_wav(&zero_channels).is_none());

        let mut huge_rate = stereo_wav();
        huge_rate[24..28].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_wav(&huge_rate).is_none(), "byte rate 溢出应拒绝");

        let mut huge_channels = stereo_wav();
        huge_channels[22..24].copy_from_slice(&u16::MAX.to_le_bytes());
        assert!(parse_wav(&huge_channels).is_none(), "block align 溢出应拒绝");
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
