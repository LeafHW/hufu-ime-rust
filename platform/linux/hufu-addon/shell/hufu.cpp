// SPDX-FileCopyrightText: 2026 明雅流风 <crrvx@outlook.com>
// SPDX-License-Identifier: GPL-3.0-or-later

// 虎符（hufu-ime）fcitx5 addon 的 C++ 薄壳：只做 fcitx5 接口适配，
// 按键经 C ABI（libhufu_fcitx5_client，Rust）走 Unix socket 到 hufu-server。
#include <fcitx/action.h>
#include <fcitx/addonfactory.h>
#include <fcitx/addoninstance.h>
#include <fcitx/addonmanager.h>
#include <fcitx/candidatelist.h>
#include <fcitx/event.h>
#include <fcitx/inputcontext.h>
#include <fcitx/inputmethodengine.h>
#include <fcitx/inputmethodentry.h>
#include <fcitx/inputpanel.h>
#include <fcitx/inputcontextproperty.h>
#include <fcitx/instance.h>
#include <fcitx/menu.h>
#include <fcitx/statusarea.h>
#include <fcitx/surroundingtext.h>
#include <fcitx/text.h>
#include <fcitx/userinterface.h>
#include <fcitx/userinterfacemanager.h>
#include <fcitx-config/configuration.h>
#include <fcitx-config/enum.h>
#include <fcitx-config/iniparser.h>
#include <fcitx-config/option.h>
#include <fcitx-utils/capabilityflags.h>
#include <fcitx-utils/event.h>
#include <fcitx-utils/i18n.h>
#include <fcitx-utils/key.h>
#include <fcitx-utils/keysym.h>
#include <fcitx-utils/log.h>
#include <fcitx-utils/trackableobject.h>
#include <fcitx-utils/utf8.h>

#include <algorithm>
#include <cerrno>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <ctime>
#include <functional>
#include <memory>
#include <string>
#include <vector>

#include <fcntl.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#include "hufu_abi.h"

// 日志类别：有了自己的类别，`--verbose='hufu=5'` 就能只把本插件的日志调到 Debug，
// 不必连累其它 addon；类别默认级别为 Info，故现有 Info/Warning 的可见性不变。
FCITX_DEFINE_LOG_CATEGORY(hufuLog, "hufu");
#define HUFU_DEBUG() FCITX_LOGC(hufuLog, Debug)

namespace {

/// Shift 形态 → 基准字符（引擎按「基准字符 + shift 修饰」处理，
/// 如 Shift+, → 《、Shift+1 → ！由引擎内部完成）。
char unshiftAscii(char c) {
    if (c >= 'A' && c <= 'Z') {
        return static_cast<char>(c - 'A' + 'a');
    }
    switch (c) {
    case '!': return '1';
    case '@': return '2';
    case '#': return '3';
    case '$': return '4';
    case '%': return '5';
    case '^': return '6';
    case '&': return '7';
    case '*': return '8';
    case '(': return '9';
    case ')': return '0';
    case '_': return '-';
    case '+': return '=';
    case '{': return '[';
    case '}': return ']';
    case '|': return '\\';
    case ':': return ';';
    case '"': return '\'';
    case '<': return ',';
    case '>': return '.';
    case '?': return '/';
    case '~': return '`';
    default: return c;
    }
}

/// fcitx5 Key → 引擎键名（与 macOS IMK / Windows TSF 前端同一张表）。
/// 返回空串 = 不归本引擎处理（透传）。
std::string keyNameOf(const fcitx::Key &key) {
    switch (key.sym()) {
    case FcitxKey_space: return "space";
    case FcitxKey_Return:
    case FcitxKey_KP_Enter: return "enter";
    case FcitxKey_BackSpace: return "backspace";
    case FcitxKey_Tab:
    case FcitxKey_ISO_Left_Tab: return "tab";
    case FcitxKey_Escape: return "escape";
    case FcitxKey_Delete: return "delete";
    case FcitxKey_Up: return "up";
    case FcitxKey_Down: return "down";
    case FcitxKey_Left: return "left";
    case FcitxKey_Right: return "right";
    case FcitxKey_Home: return "home";
    case FcitxKey_End: return "end";
    case FcitxKey_Prior: return "pageup";
    case FcitxKey_Next: return "pagedown";
    // 【Linux 策略 2026-09-19】不向引擎转发独立的 Shift/CapsLock：
    // Linux 的英文输入由 fcitx5 键盘布局（keyboard-us 等）提供，
    // 引擎不应自带中英切换（Shift 单击切中英 / Caps 切英文）。
    // 说明：Shift+标点（Shift+, → 《）等带修饰的可打印键不受影响，
    // 仍按「基准字符 + shift」转发给引擎处理。
    case FcitxKey_Control_L: return "ctrl";
    case FcitxKey_Control_R: return "ctrlright";
    case FcitxKey_Alt_L: return "alt";
    case FcitxKey_Alt_R: return "altright";
    default: break;
    }
    // 可打印 ASCII：X11 keysym → Unicode → 去 Shift 形态
    uint32_t u = fcitx::Key::keySymToUnicode(key.sym());
    if (u == 0 || u > 0x7f) {
        return {};
    }
    return std::string(1, unshiftAscii(static_cast<char>(u)));
}

/// JSON 字符串转义（配置补丁值都是短字符/短串，只处理 `"`、`\` 与控制符）。
std::string jesc(const std::string &s) {
    std::string o;
    o.reserve(s.size() + 2);
    for (unsigned char c : s) {
        if (c == '"' || c == '\\') {
            o.push_back('\\');
            o.push_back(static_cast<char>(c));
        } else if (c >= 0x20) {
            o.push_back(static_cast<char>(c));
        }
    }
    return o;
}

inline const char *jbool(bool v) { return v ? "true" : "false"; }

/// 字反查数据根目录：`${XDG_DATA_HOME:-$HOME/.local/share}/hufu`（与 install.sh 同一口径）。
/// `XDG_DATA_HOME` 与 `HOME` 都拿不到时返回空串——调用方按「数据不可用」处理，
/// 不臆造一个相对路径去碰运气。
std::string hufuRootDir() {
    std::string base;
    if (const char *xdg = std::getenv("XDG_DATA_HOME");
        xdg != nullptr && *xdg != '\0') {
        base = xdg;
    } else if (const char *home = std::getenv("HOME");
               home != nullptr && *home != '\0') {
        base = std::string(home) + "/.local/share";
    }
    if (base.empty()) {
        return {};
    }
    return base + "/hufu";
}

/// 是否汉字（字反查只认汉字：光标左侧是空白/标点/拉丁字母时不出提示）。
/// 覆盖基本区、扩展 A、兼容表意文字与扩展 B 及以上（增补平面）。
bool isHanCodePoint(uint32_t c) {
    return (c >= 0x3400 && c <= 0x4dbf) || // 扩展 A
           (c >= 0x4e00 && c <= 0x9fff) || // 基本区
           (c >= 0xf900 && c <= 0xfaff) || // 兼容表意文字
           (c >= 0x20000 && c <= 0x3ffff); // 扩展 B–G（增补平面）
}

/// 按键「实际产生的字符」（触发键归一用；0 = 该键不产字符，如功能键/独立修饰键）。
///
/// `~` 在物理键盘上是 Shift+`` ` ``：前端可能上报该 level 的 keysym（`asciitilde`），
/// 也可能原样上报 `grave`+Shift——两者都归一为 `~`，与配置里「无修饰的 `~`」同形。
/// 因此触发键比对只看字符与 Ctrl/Alt/Super，Shift 交给这里的字符归一。
char triggerCharOf(const fcitx::Key &key) {
    const uint32_t u = fcitx::Key::keySymToUnicode(key.sym());
    if (u == 0 || u > 0x7f) {
        return 0;
    }
    if (u == '`' && key.states().test(fcitx::KeyState::Shift)) {
        return '~';
    }
    return static_cast<char>(u);
}

/// 字反查武装期间「不消费」的光标移动键（Left / Right / Home / End）。
///
/// 有意选择：这些键**不消费**，交回 fcitx5 转发给应用（应用光标照常移动），
/// 查找结果由 30ms 量级的延迟重查异步跟随，用户感知为实时。
/// 若日后要改成「消费方向键、移动一个虚拟查找光标」，改这里（不再让 `keyEvent` 早退）
/// 与 `scheduleCharLookupRefresh` 的调用点：把移动量记在该输入上下文的
/// `HufuUiState` 上，重查时按「光标位置 + 移动量」取字即可。
bool isCharLookupMoveKey(const fcitx::Key &key) {
    switch (key.sym()) {
    case FcitxKey_Left:
    case FcitxKey_Right:
    case FcitxKey_Home:
    case FcitxKey_End:
        return true;
    default:
        return false;
    }
}

/// 字反查延迟重查的间隔（微秒）：方向键交给应用后，等应用把光标移到位再读
/// `surroundingText()`；30ms 量级在用户感知上等同实时，又不至于一个按键一次查询。
constexpr uint64_t kCharLookupRefreshUsec = 30 * 1000;

/// 按 TAB 切列（`hufu_client_char_lookup` 的结果串是「拼音\t虎码[\t拆分]」）。
std::vector<std::string> splitTabs(const std::string &s) {
    std::vector<std::string> out;
    if (s.empty()) {
        return out;
    }
    size_t start = 0;
    while (true) {
        const size_t tab = s.find('\t', start);
        if (tab == std::string::npos) {
            out.push_back(s.substr(start));
            return out;
        }
        out.push_back(s.substr(start, tab - start));
        start = tab + 1;
    }
}

/// 本 addon 的配置文件（相对 fcitx5 的 `PkgConfig` 目录，即 `~/.config/fcitx5/`）：
/// 构造时 `fcitx::readAsIni` 读入，状态菜单里的宿主开关用 `fcitx::safeSaveAsIni`
/// 写回——两者必须同路径、同 API 家族，否则「界面上改了但重启就丢」或写进另一个文件。
constexpr const char *kConfigPath = "conf/hufu.conf";

/// ── 按键音效播放（宿主侧）────────────────────────────────────────────────
/// 引擎在 key/select 回包里给音效 tag，wav 字节经 `sound` op 取回（见 hufu_abi.h）。
/// 播放器由本层探测并后台 spawn：音量能映射的映射，不能的忽略（见 `soundPlayerArgs`）。

/// 可用的播放器（按顺序探测：PulseAudio → PipeWire → ALSA → SoX）。
enum class SoundPlayer {
    Unknown, ///< 还没探测过
    None,    ///< 一个都没有（已提示过安装）
    Paplay,  ///< `paplay --volume=0..65536`（线性音量）
    PwPlay,  ///< `pw-play --volume=0..1.0`
    Aplay,   ///< aplay 没有音量选项：音量忽略（按引擎/系统侧音量放）
    SoxPlay, ///< SoX 的 `play -v <0..1>`
};

/// 探测顺序与名字（`play` 是 SoX 的播放前端）。
constexpr const char *kSoundPlayerNames[] = {"paplay", "pw-play", "aplay", "play"};

/// 在 `$PATH` 里按 `kSoundPlayerNames` 顺序找第一个可执行的播放器：
/// 找到则写回绝对路径并返回其种类，找不到返回 `None`。
/// PATH 为空（未设置或清空）等于没有播放器——排障时可以清空 PATH 复现提示。
SoundPlayer probeSoundPlayer(std::string *exe) {
    const char *env = std::getenv("PATH");
    if (env == nullptr || *env == '\0') {
        return SoundPlayer::None;
    }
    std::vector<std::string> dirs;
    std::string cur;
    for (const char *p = env;; ++p) {
        if (*p == ':' || *p == '\0') {
            dirs.push_back(cur.empty() ? "." : cur);
            cur.clear();
            if (*p == '\0') {
                break;
            }
            continue;
        }
        cur.push_back(*p);
    }
    for (const char *name : kSoundPlayerNames) {
        for (const std::string &dir : dirs) {
            const std::string candidate = dir + "/" + name;
            struct stat st = {};
            if (::stat(candidate.c_str(), &st) != 0 || !S_ISREG(st.st_mode) ||
                ::access(candidate.c_str(), X_OK) != 0) {
                continue;
            }
            *exe = candidate;
            if (std::strcmp(name, "paplay") == 0) {
                return SoundPlayer::Paplay;
            }
            if (std::strcmp(name, "pw-play") == 0) {
                return SoundPlayer::PwPlay;
            }
            if (std::strcmp(name, "aplay") == 0) {
                return SoundPlayer::Aplay;
            }
            return SoundPlayer::SoxPlay;
        }
    }
    return SoundPlayer::None;
}

/// 引擎音量（0–100）→ 播放器音量因子（"0.00"–"1.00"）。
std::string volumeFactor(int32_t volume) {
    char buf[16] = {};
    std::snprintf(buf, sizeof(buf), "%.2f", static_cast<double>(volume) / 100.0);
    return buf;
}

/// 播放器参数（`path` 是已落盘的 wav）。音量映射按各播放器自己的选项：
/// paplay 线性 0..65536、pw-play 0..1.0、SoX `-v` 0..1；aplay 无音量选项，
/// 只能按系统/引擎侧音量播放（这里忽略音量，不臆造参数）。
std::vector<std::string> soundPlayerArgs(SoundPlayer player, const std::string &path,
                                         int32_t volume) {
    const int32_t v = std::clamp(volume, 0, 100);
    switch (player) {
    case SoundPlayer::Paplay:
        return {"--volume=" + std::to_string(v * 65536 / 100), path};
    case SoundPlayer::PwPlay:
        return {"--volume=" + volumeFactor(v), path};
    case SoundPlayer::SoxPlay:
        return {"-v", volumeFactor(v), path};
    case SoundPlayer::Unknown:
    case SoundPlayer::None:
    case SoundPlayer::Aplay:
        break;
    }
    return {path};
}

/// 后台 spawn 播放器：fork 两次——中间进程立刻退出并被本进程回收（不留僵尸），
/// 孙进程改挂 init 后 exec 播放器；本进程**不等待**孙进程，UI 线程不被播放阻塞。
/// 标准输入/输出/错误都接到 /dev/null（播放器的话不进 fcitx5 的终端）。
///
/// 路径与 argv 都在 fork **之前**备好：fork 之后只调用异步信号安全函数
///（fork/open/dup2/close/execv/_exit）——这是多线程进程里 fork 仍然安全的前提。
void spawnSoundPlayer(const std::string &exe, const std::vector<std::string> &args) {
    if (exe.empty()) {
        return;
    }
    std::vector<std::string> all;
    all.reserve(args.size() + 1);
    all.push_back(exe); // argv[0]
    all.insert(all.end(), args.begin(), args.end());
    std::vector<char *> argv;
    argv.reserve(all.size() + 1);
    for (std::string &s : all) {
        argv.push_back(const_cast<char *>(s.c_str()));
    }
    argv.push_back(nullptr);

    const pid_t pid = ::fork();
    if (pid < 0) {
        return; // 起不来就算了：音效是锦上添花，不该影响输入
    }
    if (pid == 0) {
        if (::fork() != 0) {
            _exit(0); // 中间层：立刻退出，孙进程改挂 init（由 init 回收）
        }
        const int devnull = ::open("/dev/null", O_RDWR);
        if (devnull >= 0) {
            ::dup2(devnull, STDIN_FILENO);
            ::dup2(devnull, STDOUT_FILENO);
            ::dup2(devnull, STDERR_FILENO);
            if (devnull > STDERR_FILENO) {
                ::close(devnull);
            }
        }
        ::execv(exe.c_str(), argv.data());
        _exit(127); // exec 失败（文件被换掉等）：静默退场
    }
    int status = 0;
    ::waitpid(pid, &status, 0); // 只回收中间层（它立刻退出）
}

/// 音效 wav 的落盘目录：`$XDG_RUNTIME_DIR/hufu-sound`（用户私有运行时目录，首选）；
/// `XDG_RUNTIME_DIR` 缺失时退回 `$TMPDIR`（与 socket 默认路径同口径），再退回 `/tmp`。
std::string soundDirPath() {
    std::string base;
    if (const char *runtime = std::getenv("XDG_RUNTIME_DIR");
        runtime != nullptr && *runtime != '\0') {
        base = runtime;
    } else if (const char *tmp = std::getenv("TMPDIR");
               tmp != nullptr && *tmp != '\0') {
        base = tmp;
    } else {
        base = "/tmp";
    }
    return base + "/hufu-sound";
}

/// tag 只当文件名的一段用：引擎侧白名单是 key/select/commit/page，
/// 这里再限一次字符集（不信任对端，也不让 `/`、`..` 进路径）。
bool isSafeSoundTag(const std::string &tag) {
    if (tag.empty() || tag.size() > 16) {
        return false;
    }
    for (unsigned char c : tag) {
        const bool ok = (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') ||
                        (c >= '0' && c <= '9') || c == '_' || c == '-';
        if (!ok) {
            return false;
        }
    }
    return true;
}

/// 确保落盘目录存在且属于本人（首次创建即 0700）。`/tmp` 回退路径上可能已经有
/// 别人抢先建的同名目录——那种情况直接判失败：宁可不播，也不往别人的目录里写。
bool ensureSoundDir(const std::string &dir) {
    if (::mkdir(dir.c_str(), 0700) == 0) {
        return true;
    }
    struct stat st = {};
    if (errno != EEXIST || ::stat(dir.c_str(), &st) != 0) {
        return false;
    }
    return S_ISDIR(st.st_mode) && st.st_uid == ::geteuid();
}

/// 写 wav 文件（0600）：临时目录可能被别的用户读到，故不给组/他人权限；
/// `O_NOFOLLOW` 防止回退路径上被同名符号链接顶掉（是链接就直接失败）。
bool writeFile0600(const std::string &path, const uint8_t *data, size_t size) {
    const int fd = ::open(path.c_str(),
                          O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC | O_NOFOLLOW, 0600);
    if (fd < 0) {
        return false;
    }
    size_t off = 0;
    bool ok = true;
    while (off < size) {
        const ssize_t n = ::write(fd, data + off, size - off);
        if (n <= 0) {
            ok = false;
            break;
        }
        off += static_cast<size_t>(n);
    }
    ::close(fd);
    return ok;
}

/// 按键音效通道（宿主侧）：把「取 tag → 取 wav → 落盘 → 后台播放」收在一处，
/// 输入上下文相关的东西一概不碰。失败一律静默降级：只记**一次** Warn，不影响输入。
///
/// 本层**不缓存引擎的开关态与音量**：引擎只在 `sound.enabled` 时才在回包里带 tag
///（没带就是没开或本键无音效），音量由客户端每次现取。于是设置页/托盘的任何改动
/// 都是即改即生效——此前宿主缓存开关、客户端缓存音量，都得等开关翻转一次才刷新。
class HufuSoundChannel {
public:
    /// 一次 key/select 之后：取走待处理音效 tag（无论有无音效都要取走，否则会
    /// 残留到下一次按键），取回 wav 并播放。
    void drain(hufu_client *engine) {
        if (engine == nullptr) {
            return;
        }
        const char *tag = hufu_client_take_sound(engine);
        if (tag == nullptr || *tag == '\0') {
            return; // 本次没有音效（引擎侧 sound.enabled 关时就不会有）
        }
        const std::string name(tag);
        const int32_t volume = hufu_client_sound_fetch(engine, name.c_str());
        const uint8_t *data = hufu_client_sound_data(engine);
        const int32_t size = hufu_client_sound_size(engine);
        if (volume < 0 || data == nullptr || size <= 0) {
            warnOnce("取按键音效失败（音效 wav 缺失或 hufu-server 不可达）");
            return;
        }
        const std::string path = ensureSoundFile(name, data, static_cast<size_t>(size));
        if (path.empty()) {
            warnOnce("按键音效落盘失败");
            return;
        }
        play(path, volume);
    }

private:
    /// 一类音效的 wav 落盘路径（目录见 `soundDirPath`）。
    static std::string soundFilePath(const std::string &tag) {
        return soundDirPath() + "/" + tag + ".wav";
    }

    /// 该类音效首次播放前落盘（其后复用同一文件），返回可播路径；空串=写不了。
    std::string ensureSoundFile(const std::string &tag, const uint8_t *data, size_t size) {
        if (!isSafeSoundTag(tag)) {
            return {};
        }
        const std::string path = soundFilePath(tag);
        if (std::find(written_.begin(), written_.end(), tag) != written_.end()) {
            return path;
        }
        if (!ensureSoundDir(soundDirPath()) || !writeFile0600(path, data, size)) {
            return {};
        }
        written_.push_back(tag);
        HUFU_DEBUG() << "hufu: 音效落盘 " << path << "（" << size << " 字节）";
        return path;
    }

    /// 探测播放器（只探一次）并后台播放。
    void play(const std::string &path, int32_t volume) {
        if (player_ == SoundPlayer::Unknown) {
            player_ = probeSoundPlayer(&playerExe_);
            if (player_ == SoundPlayer::None) {
                FCITX_LOGC(hufuLog, Warn)
                    << "hufu: 未找到音频播放器，按键音效无法播放——请安装 "
                       "pulseaudio-utils / pipewire-bin / alsa-utils（或 sox）之一";
            }
            HUFU_DEBUG() << "hufu: 音效播放器 " << playerExe_ << "（种类 "
                         << static_cast<int>(player_) << "）";
        }
        if (player_ == SoundPlayer::None) {
            return; // 已经提示过：不再刷屏
        }
        spawnSoundPlayer(playerExe_, soundPlayerArgs(player_, path, volume));
    }

    /// 同一类失败只记一次 Warn（音效是附加功能，缺数据/缺播放器不该刷日志）。
    void warnOnce(const std::string &reason) {
        if (warnedFetch_) {
            return;
        }
        warnedFetch_ = true;
        FCITX_LOGC(hufuLog, Warn) << "hufu: " << reason << "，已跳过播放";
    }

    /// 播放器探测结果（`Unknown` = 还没探过；`None` = 已探过且没有）
    SoundPlayer player_ = SoundPlayer::Unknown;
    std::string playerExe_;
    /// 已落盘的 tag（每类只写一次）
    std::vector<std::string> written_;
    /// 取音效/落盘失败是否已记过 Warn
    bool warnedFetch_ = false;
};

class HufuEngine;

/// 面板候选：点击（`select`）按页内下标上屏——与数字选重同语义
///（学习、无闪帧）。此前用 `DisplayOnlyCandidateWord`，点击无反应。
///
/// 生命周期：候选列表归 `InputContext` 的输入面板所有，而 IC **晚于** addon 实例
///（含本引擎）析构——`InstancePrivate` 先声明 `icManager_`、后声明 `addonManager_`，
/// 反向析构 ⇒ `~AddonManager`（删 addon 实例）在 IC 之前。引擎释放后，面板里可能
/// 仍留着本对象，用户点一下就走到已释放的引擎上（悬垂 `this` 调用 = UAF）。故这里
/// 只持 `TrackableObjectReference` 弱引用：引擎析构即失效，`select` 里早退。
class HufuCandidateWord : public fcitx::CandidateWord {
public:
    HufuCandidateWord(fcitx::Text text, fcitx::Text comment,
                      fcitx::TrackableObjectReference<HufuEngine> owner,
                      int32_t index)
        : CandidateWord(std::move(text)), owner_(owner), index_(index) {
        setComment(std::move(comment));
    }

    void select(fcitx::InputContext *inputContext) const override;

private:
    fcitx::TrackableObjectReference<HufuEngine> owner_;
    int32_t index_;
};

/// 候选排列（默认跟随 fcitx5 全局「候选竖排」设置）。
///
/// 此前是布尔项 `ForceVertical`（勾选=竖排、不勾=跟随全局）；改成三态后多出「强制横排」，
/// 代价是旧配置里的 `ForceVertical=True` 不再被读取（键名不同，静默回到默认）——
/// 需要竖排的用户在配置页选「竖排」即可，迁移说明见 `platform/linux/README.md`。
enum class HufuCandidateLayout { FollowGlobal, Horizontal, Vertical };
FCITX_CONFIG_ENUM_NAME(HufuCandidateLayout, "跟随全局", "横排", "竖排");
FCITX_CONFIG_ENUM_I18N_ANNOTATION(HufuCandidateLayout, "跟随全局", "横排", "竖排");

/// 枚举注解 + 悬浮说明（`EnumI18n` 与 `Tooltip` 并存；fcitx5 自带注解只支持其一）。
template <typename EnumAnnotation>
struct EnumAnnotationWithTooltip : EnumAnnotation {
    explicit EnumAnnotationWithTooltip(std::string tooltip)
        : tooltip_(std::move(tooltip)) {}

    bool skipDescription() const { return false; }
    bool skipSave() const { return false; }
    void dumpDescription(fcitx::RawConfig &config) const {
        EnumAnnotation::dumpDescription(config);
        config.setValueByPath("Tooltip", tooltip_);
    }

private:
    std::string tooltip_;
};

/// ── fcitx5 设置页 schema（fcitx5-configtool「虎符」页）────────────────────
/// 两类选项：
/// - 宿主项（候选窗内预编辑 / 候选排列）：本层直接生效，只存
///   `~/.config/fcitx5/conf/hufu.conf`。
/// - 引擎项：打开页面时从 hufu-server 拉取（`config_get`），应用时深合并
///   写回（`config_set`）——与 Web 设置页同一份配置，热生效。
/// 说明：中英切换等 Linux 策略项不在此暴露（英文输入交给 fcitx5 布局）。
FCITX_CONFIGURATION(
    HufuBehaviorConfig,
    fcitx::OptionWithAnnotation<bool, fcitx::ToolTipAnnotation> panelPreedit{{
        .parent = this,
        .path{"PanelPreedit"},
        .description{"候选窗内显示编码"},
        .defaultValue = true,
        .annotation{"在候选窗顶部显示编码串；默认开（关闭后组段仍随光标内联显示）。"}}};
    fcitx::OptionWithAnnotation<
        HufuCandidateLayout,
        EnumAnnotationWithTooltip<HufuCandidateLayoutI18NAnnotation>>
        candidateLayout{{
            .parent = this,
            .path{"CandidateLayout"},
            .description{"候选排列"},
            .defaultValue = HufuCandidateLayout::FollowGlobal,
            .annotation{"跟随全局：候选窗排列随 fcitx5 全局「候选竖排」；"
                        "横排：强制横排；竖排：强制竖排。"}}};
    fcitx::Option<int, fcitx::IntConstrain, fcitx::DefaultMarshaller<int>,
                  fcitx::ToolTipAnnotation>
        pageSize{{
            .parent = this,
            .path{"PageSize"},
            .description{"每页候选数"},
            .defaultValue = 4,
            .constrain = fcitx::IntConstrain(1, 10),
            .annotation{"候选列表每页个数（1–10）。"}}};
    fcitx::OptionWithAnnotation<bool, fcitx::ToolTipAnnotation> autoPush{{
        .parent = this,
        .path{"AutoPush"},
        .description{"超最大码长自动上屏"},
        .defaultValue = true,
        .annotation{"编码超过最大码长时，前串首选自动上屏、新键成为新串起点。"}}};
    fcitx::OptionWithAnnotation<bool, fcitx::ToolTipAnnotation> autoSelectUnique{{
        .parent = this,
        .path{"AutoSelectUnique"},
        .description{"满码唯一自动上屏"},
        .defaultValue = false,
        .annotation{"满最大码长且只有一个候选时直接上屏。"}}};
    fcitx::OptionWithAnnotation<bool, fcitx::ToolTipAnnotation> autoClearEmpty{{
        .parent = this,
        .path{"AutoClearEmpty"},
        .description{"空码自动清屏"},
        .defaultValue = false,
        .annotation{"第「最大码长+1」键仍无解才清，且只清前面的码（该键保留为新输入）。"}}};
    fcitx::OptionWithAnnotation<bool, fcitx::ToolTipAnnotation> enterClear{{
        .parent = this,
        .path{"EnterClear"},
        .description{"回车清屏"},
        .defaultValue = false,
        .annotation{"按回车清空当前编码（不提交）。"}}};);

FCITX_CONFIGURATION(
    HufuPunctConfig,
    fcitx::OptionWithAnnotation<bool, fcitx::ToolTipAnnotation> fullShape{{
        .parent = this,
        .path{"FullShape"},
        .description{"全角标点"},
        .defaultValue = true,
        .annotation{"标点输出全角形式（如 `,` → `，`）。"}}};
    fcitx::OptionWithAnnotation<bool, fcitx::ToolTipAnnotation> asciiPunct{{
        .parent = this,
        .path{"AsciiPunct"},
        .description{"中文态英文标点"},
        .defaultValue = false,
        .annotation{"中文输入时标点不做中文映射，直接输出 ASCII。"}}};);

FCITX_CONFIGURATION(
    HufuFilterConfig,
    fcitx::OptionWithAnnotation<bool, fcitx::ToolTipAnnotation> opencc{{
        .parent = this,
        .path{"OpenCC"},
        .description{"启用简繁转换"},
        .defaultValue = false,
        .annotation{"按下方方向转换候选（打简出繁/打繁出简）。"}}};
    fcitx::OptionWithAnnotation<bool, fcitx::ToolTipAnnotation> toTraditional{{
        .parent = this,
        .path{"ToTraditional"},
        .description{"打简出繁"},
        .defaultValue = true,
        .annotation{"勾选=简体→繁体；不勾=繁体→简体。"}}};
    fcitx::OptionWithAnnotation<bool, fcitx::ToolTipAnnotation> emoji{{
        .parent = this,
        .path{"Emoji"},
        .description{"emoji 候选变体"},
        .defaultValue = false,
        .annotation{"前几个候选追加 emoji 注解变体。"}}};
    fcitx::OptionWithAnnotation<bool, fcitx::ToolTipAnnotation> showPinyin{{
        .parent = this,
        .path{"ShowPinyin"},
        .description{"候选显示拼音"},
        .defaultValue = false,
        .annotation{"候选注释显示拼音（需随包拼音注释数据）。"}}};
    fcitx::OptionWithAnnotation<bool, fcitx::ToolTipAnnotation> showUnicode{{
        .parent = this,
        .path{"ShowUnicode"},
        .description{"候选显示 Unicode 分区"},
        .defaultValue = true,
        .annotation{"非基本区字符显示分区名，如 [平假名]。"}}};
    fcitx::OptionWithAnnotation<bool, fcitx::ToolTipAnnotation> showSplit{{
        .parent = this,
        .path{"ShowSplit"},
        .description{"候选显示拆分"},
        .defaultValue = true,
        .annotation{"候选注释显示部件拆分（最多 4 部件）。"}}};);

FCITX_CONFIGURATION(
    HufuSentenceConfig,
    fcitx::OptionWithAnnotation<bool, fcitx::ToolTipAnnotation> sentence{{
        .parent = this,
        .path{"Sentence"},
        .description{"整句模式"},
        .defaultValue = true,
        .annotation{"启用整句组句（需方案名含「整句」或关闭自动启用）。"}}};
    fcitx::OptionWithAnnotation<bool, fcitx::ToolTipAnnotation> rerank{{
        .parent = this,
        .path{"Rerank"},
        .description{"神经重排"},
        .defaultValue = true,
        .annotation{"停顿后用 Qwen3 GGUF 模型对候选重排（需模型文件）。"}}};);

FCITX_CONFIGURATION(
    HufuReverseConfig,
    fcitx::OptionWithAnnotation<bool, fcitx::ToolTipAnnotation> enabled{{
        .parent = this,
        .path{"ReverseEnabled"},
        .description{"启用反查"},
        .defaultValue = true,
        .annotation{"反查前缀进入拼音反查模式（需反查表数据）。"}}};
    fcitx::OptionWithAnnotation<std::string, fcitx::ToolTipAnnotation> prefix{{
        .parent = this,
        .path{"ReversePrefix"},
        .description{"反查前缀"},
        .defaultValue = "`",
        .annotation{"单字符前缀，默认 `。"}}};);

FCITX_CONFIGURATION(
    HufuSoundConfig,
    fcitx::OptionWithAnnotation<bool, fcitx::ToolTipAnnotation> enabled{{
        .parent = this,
        .path{"SoundEnabled"},
        .description{"启用按键音"},
        .defaultValue = false,
        .annotation{"按键/选词/上屏/翻页四类提示音（需音效 wav 数据）。"}}};
    fcitx::Option<int, fcitx::IntConstrain, fcitx::DefaultMarshaller<int>,
                  fcitx::ToolTipAnnotation>
        volume{{
            .parent = this,
            .path{"SoundVolume"},
            .description{"音量"},
            .defaultValue = 50,
            .constrain = fcitx::IntConstrain(0, 100),
            .annotation{"0–100。"}}};);

FCITX_CONFIGURATION(
    HufuKeysConfig,
    fcitx::OptionWithAnnotation<std::string, fcitx::ToolTipAnnotation> secondSelect{{
        .parent = this,
        .path{"SecondSelect"},
        .description{"次选键"},
        .defaultValue = ";",
        .annotation{"单字符，默认 ;（有编码延续时作编码字符）。"}}};
    fcitx::OptionWithAnnotation<std::string, fcitx::ToolTipAnnotation> thirdSelect{{
        .parent = this,
        .path{"ThirdSelect"},
        .description{"三选键"},
        .defaultValue = "'",
        .annotation{"单字符，默认 '。"}}};
    fcitx::OptionWithAnnotation<std::string, fcitx::ToolTipAnnotation> pagingKeys{{
        .parent = this,
        .path{"PagingKeys"},
        .description{"翻页键"},
        .defaultValue = "-=",
        .annotation{"字符序列，前半上翻、后半下翻（默认 -=）。"}}};);

/// 快捷键（宿主项）：键都在本层匹配，只存 `~/.config/fcitx5/conf/hufu.conf`，不进引擎配置。
///
/// 「字反查」按**无修饰的 keysym** 声明（`~` = `asciitilde`）：`~` 物理上是 Shift+`` ` ``，
/// 前端上报的是该 level 的 keysym，fcitx5 对「本身就产字符」的键会去掉 Shift，
/// 故配置态与真实按键同为无修饰形态（匹配时另做 `grave`+Shift 的兼容，见
/// `HufuEngine::charLookupTriggered`）。`AllowModifierLess` 允许无修饰键被保存。
FCITX_CONFIGURATION(
    HufuHotkeyConfig,
    fcitx::Option<fcitx::Key, fcitx::KeyConstrain,
                  fcitx::DefaultMarshaller<fcitx::Key>, fcitx::ToolTipAnnotation>
        charLookup{{
            .parent = this,
            .path{"CharLookupKey"},
            .description{"字反查"},
            .defaultValue =
                fcitx::Key(FcitxKey_asciitilde, fcitx::KeyState::NoState),
            .constrain =
                fcitx::KeyConstrain(fcitx::KeyConstrainFlag::AllowModifierLess),
            .annotation{"显示光标左侧一个汉字的拼音（上排）与虎码（下排，"
                        "有拆分数据时附在虎码后），需应用支持周边文本；"
                        "触发键本身不再作为普通字符输入，清空即关闭本功能。"}}};);

FCITX_CONFIGURATION(
    HufuConfig,
    fcitx::Option<HufuBehaviorConfig> behavior{this, "Behavior", "行为"};
    fcitx::Option<HufuPunctConfig> punct{this, "Punct", "标点"};
    fcitx::Option<HufuFilterConfig> filter{this, "Filter", "滤镜（简繁/注释）"};
    fcitx::Option<HufuSentenceConfig> sentence{this, "Sentence", "整句"};
    fcitx::Option<HufuReverseConfig> reverse{this, "Reverse", "反查"};
    fcitx::Option<HufuSoundConfig> sound{this, "Sound", "音效"};
    fcitx::Option<HufuKeysConfig> keys{this, "Keys", "选重与翻页"};
    fcitx::Option<HufuHotkeyConfig> hotkeys{this, "Hotkey", "快捷键"};);

/// 最近一次 UI 快照（面板预编辑 / 候选与注释 / 高亮 / 辅助文本 / 中英态）。
///
/// 宿主开关（「候选窗显示预编辑」）切换后要**立即**重放当前界面：关闭期间面板里
/// 已经没有预编辑原文，重开时若不重放就得等下一次按键才看得到。故 `applyUpdate`
/// 顺手把快照记在**该输入上下文的属性**上（属性随输入上下文销毁 ⇒ 不留任何
/// 跨调用存活的输入上下文指针）。
/// 只放宿主侧内容：引擎侧沿用虎符既有的 focus 语义（不引入会话 id），多个输入
/// 上下文**共享一个引擎会话**（最后激活者胜）——这是既定取舍，属性里的快照只用于
/// 本输入上下文的面板重放，不改变它。
/// 说明：虎符的 update 回调不下发字节光标、也只有一排 aux，故快照没有这两项——
/// 宿主不臆造引擎没有给过的位置信息。
struct HufuUiSnapshot {
    bool valid = false;
    std::string preedit;
    std::vector<std::string> texts;
    std::vector<std::string> comments;
    int32_t selected = 0;
    std::string aux;
    bool chinese = true;
};

/// 宿主侧字反查视图：显示态 + 两排文案（随输入上下文存活）。
///
/// 记在 UI 快照属性里，是为了让 `render` 保持**唯一**的输入面板写入口：引擎下一次
/// update 也走 render，若两排只写在面板上而不记在这里，重放路径会把上排换回引擎 aux、
/// 下排却留在面板上。
struct HufuCharLookupView {
    bool shown = false;
    std::string up;
    std::string down;
};

/// 每输入上下文属性（fcitx5 `InputContextProperty`）：只存宿主侧 UI 快照与字反查视图。
/// 引擎侧不建会话（见 `HufuUiSnapshot`），故本类不持任何引擎指针、析构也不回调引擎。
class HufuUiState : public fcitx::InputContextProperty {
public:
    HufuUiSnapshot &snapshot() { return snapshot_; }
    HufuCharLookupView &charLookup() { return charLookup_; }
    /// 字反查「已武装」：触发键按下后保持，按方向键移动光标时结果跟随更新；
    /// Esc / 其它按键 / 失焦 / 切换输入法撤防（见 `HufuEngine::keyEvent` 与
    /// `resetSession`）。武装态随输入上下文存活，不跨输入上下文共享。
    bool &charLookupArmed() { return charLookupArmed_; }

private:
    HufuUiSnapshot snapshot_;
    HufuCharLookupView charLookup_;
    bool charLookupArmed_ = false;
};

/// 状态菜单里的普通动作（「重载码表」「打开方案文件夹」）与信息行（「引擎状态」）：
/// 文案每次取用时现算——信息行随引擎可达性与当前方案名变化，宿主不另存一份；
/// `activate` 为空即不可点（信息行点击为无操作）。动作归引擎成员所有，
/// 生命周期见 `~HufuEngine` 的 UAF 契约。
class HufuMenuAction : public fcitx::Action {
public:
    HufuMenuAction(std::function<std::string()> label,
                   std::function<void()> activate = {})
        : label_(std::move(label)), activate_(std::move(activate)) {}

    std::string shortText(fcitx::InputContext * /*inputContext*/) const override {
        return label_();
    }

    std::string icon(fcitx::InputContext * /*inputContext*/) const override {
        return {};
    }

    void activate(fcitx::InputContext * /*inputContext*/) override {
        if (activate_) {
            activate_();
        }
    }

private:
    std::function<std::string()> label_;
    std::function<void()> activate_;
};

/// 状态菜单里的勾选开关（「按键音效」「候选窗显示预编辑」）：勾选态现取宿主缓存，
/// 点击交给宿主处理。
/// 「按键音效」的勾选态之所以不在这里直接问引擎：`isChecked` 会被 UI 线程在每次
/// 刷新菜单时调用，而问引擎是一次 socket 往返——引擎挂起时最坏要等一个读超时，
/// 会冻住 UI（故缓存在成员里，由 `refreshStatus` 取）。
class HufuToggleAction : public fcitx::Action {
public:
    HufuToggleAction(std::string label, std::function<bool()> checked,
                     std::function<void(fcitx::InputContext *)> toggled)
        : label_(std::move(label)),
          checked_(std::move(checked)),
          toggled_(std::move(toggled)) {
        setCheckable(true);
    }

    std::string shortText(fcitx::InputContext * /*inputContext*/) const override {
        return label_;
    }

    std::string icon(fcitx::InputContext * /*inputContext*/) const override {
        return {};
    }

    bool isChecked(fcitx::InputContext * /*inputContext*/) const override {
        return checked_();
    }

    void activate(fcitx::InputContext *inputContext) override {
        toggled_(inputContext);
    }

private:
    std::string label_;
    std::function<bool()> checked_;
    std::function<void(fcitx::InputContext *)> toggled_;
};

/// 引擎 addon。
///
/// 继承 `fcitx::TrackableObject<HufuEngine>`：让「归 IC / 面板所有、可能比引擎活得久」
/// 的 UI 对象（`HufuCandidateWord`）持弱引用，避免悬垂（见该类注释）。
class HufuEngine : public fcitx::InputMethodEngine,
                   public fcitx::TrackableObject<HufuEngine> {
public:
    explicit HufuEngine(fcitx::Instance *instance)
        : instance_(instance),
          uiStateFactory_(
              [](fcitx::InputContext & /*inputContext*/) {
                  return new HufuUiState;
              }) {
        hufu_host host = {};
        host.user = this;
        host.commit = &HufuEngine::commitCallback;
        host.update = &HufuEngine::updateCallback;
        // 默认 socket 路径（$XDG_RUNTIME_DIR/hufu-ime.sock）
        engine_ = hufu_client_new(nullptr, &host);
        const char *status = hufu_client_status(engine_);
        FCITX_LOGC(hufuLog, Info) << "hufu: client created (" << (status ? status : "") << ")";
        if (engine_ != nullptr && hufu_client_ping(engine_) == 0) {
            FCITX_LOGC(hufuLog, Warn) << "hufu: hufu-server 不可达（先启动引擎，按键将直通）";
        }
        // 每输入上下文一份宿主侧 UI 快照（现存与后续新建的都经工厂创建）。
        // 注册成功是属性随输入上下文销毁的前提；失败必须显式可见（否则重放拿不到
        // 快照，只剩「下一次按键才生效」这一条路径）。
        if (!instance_->inputContextManager().registerProperty("hufuUiState",
                                                               &uiStateFactory_)) {
            FCITX_LOGC(hufuLog, Warn)
                << "hufu: UI 快照属性注册失败（hufuUiState 名字冲突？）";
        }
        // 设置页：先读用户已保存值（宿主项），再以引擎配置覆盖引擎映射项
        fcitx::readAsIni(config_, kConfigPath);
        pullConfig();
        setupStatusMenu();
        // 信息行与音效勾选态先取一次（此后在状态区刷新与每次菜单动作后重取）
        refreshStatus(nullptr);
    }

    /// 设置页 schema（fcitx5-configtool「虎符」页）。打开页面时从引擎拉取
    /// 最新值（Web 设置页的改动可同步显示）。
    const fcitx::Configuration *getConfig() const override {
        const_cast<HufuEngine *>(this)->pullConfig();
        return &config_;
    }

    /// 配置工具保存：载入 schema 后写回引擎（深合并，热生效）。
    void setConfig(const fcitx::RawConfig &raw) override {
        config_.load(raw, true);
        pushConfig();
    }

    ~HufuEngine() override {
        // 析构契约（三步，顺序是安全性的前提）：
        // 1) 注销 UI 快照属性工厂：fcitx5 会立刻销毁各输入上下文上已注册的属性
        //   （属性只存宿主侧快照，析构不回调引擎；注销是为了工厂对象本身先于
        //   `InputContextManager` 失效——fcitx5 要求工厂先注销）。
        // 2) 摘掉输入上下文状态区里指向本对象成员的裸指针：状态区归 IC 所有，而 IC
        //   晚于 addon 实例（含本引擎）析构（见 `HufuCandidateWord` 的生命周期注释）；
        //   若本对象成员已析构而某个 IC 的状态区仍挂着 `&menuAction_`（及其子菜单里
        //   的动作），UI 一刷新就是悬垂指针。故在成员仍存活时先逐个 IC `clearGroup`。
        // 3) 客户端最后释放，指针置空——任何迟到的宿主回调都只是无操作。
        // 真机核对顺序：`fcitx5 -r --verbose='hufu=5'` 前台运行后退出，确认本行日志
        // 之后没有针对已卸载 addon 的状态区访问。
        HUFU_DEBUG() << "hufu: ~HufuEngine";
        // 0) 撤掉待处理的字反查重查：定时器归本对象所有，晚了就是悬垂回调。
        cancelCharLookupRefresh();
        uiStateFactory_.unregister();
        clearStatusAreas();
        if (engine_ != nullptr) {
            hufu_client_free(engine_);
            engine_ = nullptr;
        }
    }

    void keyEvent(const fcitx::InputMethodEntry & /*entry*/,
                  fcitx::KeyEvent &keyEvent) override {
        if (keyEvent.isRelease()) {
            return; // 引擎只处理按下
        }
        fcitx::InputContext *inputContext = keyEvent.inputContext();
        const fcitx::Key &raw = keyEvent.rawKey();
        HufuUiState *state = uiState(inputContext);
        // 【字反查·武装期间】Left/Right/Home/End **不消费**：交回 fcitx5 转发给应用
        //（应用光标照常移动，见 `isCharLookupMoveKey` 的取舍说明），随后安排一次
        // 很短的延迟重查，按新光标位置更新两排——用户感知为「边移动边反查」。
        if (state != nullptr && state->charLookupArmed() && isCharLookupMoveKey(raw)) {
            scheduleCharLookupRefresh(inputContext);
            return;
        }
        if (charLookupTriggered(raw)) {
            // 触发键优先于引擎：命中即消费（触发键不再作为普通字符输入）。
            // 触发后**保持武装**：再按一次等于按当前光标位置刷新。
            triggerCharLookup(inputContext);
            keyEvent.filterAndAccept();
            return;
        }
        // 武装期间按 Esc：撤防 + 清两排并消费该键（撤防是显式动作，不落到应用）。
        if (state != nullptr && state->charLookupArmed() &&
            raw.sym() == FcitxKey_Escape) {
            disarmCharLookup(inputContext);
            keyEvent.filterAndAccept();
            return;
        }
        // 其余按键：撤防 + 清两排，随后照既有流程处理该键（不吞键）。
        disarmCharLookup(inputContext);
        // 【Shift 修饰修复 2026-09-19】`key()` 是「归一化」事件：Shift+符号
        // 时 Shift 被并入符号本身（states 里不再有 Shift），引擎会当成
        // 「无 shift 的普通键」——实测 Shift+, 出「，」而非《、Shift+字母
        // 被当编码。`rawKey()` 是布局转换后、保留真实修饰态的原始事件
        //（日志实测：Shift+a → Key(A states=0) / rawKey Key(Shift+A states=1)）。
        const fcitx::Key &key = raw;
        const std::string name = keyNameOf(key);
        if (name.empty()) {
            return; // 不归本引擎：透传
        }
        const auto states = key.states();
        // 【Shift+字母（有编码/候选态）2026-09-19】引擎在编码态对 Shift+字母
        // 返回「吞键」（Windows 语义：防漏进宿主）；Linux 无引擎英文态，
        // 产品口径=「首选上屏 + 打字母」：顶字首选 → 清组段 → 字母交回应用。
        if (states.test(fcitx::KeyState::Shift) && hasComposition_ &&
            name.size() == 1 && name[0] >= 'a' && name[0] <= 'z') {
            // 【context_ 必须置位 2026-09-19】focus() 会同步回调 update，
            // 用它来清候选窗；applyUpdate 在 context_ 为空时直接返回——
            // 漏置位=上屏后候选窗滞留（用户实测复现）。
            context_ = inputContext;
            if (!topCommit_.empty()) {
                inputContext->commitString(topCommit_);
            }
            hufu_client_focus(engine_); // 清引擎组段并同步清 UI
            inputContext->forwardKey(keyEvent.rawKey(), keyEvent.isRelease(),
                                     keyEvent.time());
            context_ = nullptr;
            keyEvent.filterAndAccept();
            return;
        }
        // 行尾提示：周边文本可用时上报（组段逼近右缘的提前上屏放宽用）
        int32_t lineEnd = -1;
        const auto &surrounding = inputContext->surroundingText();
        if (surrounding.isValid()) {
            const auto &text = surrounding.text();
            const auto cursor = static_cast<size_t>(surrounding.cursor());
            const bool atEnd = cursor >= text.size();
            const bool beforeNewline = cursor < text.size() && text[cursor] == '\n';
            lineEnd = (atEnd || beforeNewline) ? 1 : 0;
        }
        context_ = inputContext;
        const int32_t rc = hufu_client_key(
            engine_, name.c_str(), states.test(fcitx::KeyState::Shift) ? 1 : 0,
            states.test(fcitx::KeyState::Ctrl) ? 1 : 0,
            states.test(fcitx::KeyState::Alt) ? 1 : 0,
            states.test(fcitx::KeyState::Super) ? 1 : 0,
            states.test(fcitx::KeyState::CapsLock) ? 1 : 0, lineEnd);
        context_ = nullptr;
        // 音效：取走本键回包里的 tag（启用时取回 wav 并后台播放）。放在按键流程之外：
        // 播放失败/没有播放器都不影响输入，也不碰 UI。
        sound_.drain(engine_);
        if (rc & HUFU_KEY_CONSUMED) {
            keyEvent.filterAndAccept();
        }
    }

    void activate(const fcitx::InputMethodEntry & /*entry*/,
                  fcitx::InputContextEvent &event) override {
        resetSession(event);
        updateStatusArea(event.inputContext());
    }

    void deactivate(const fcitx::InputMethodEntry & /*entry*/,
                    fcitx::InputContextEvent &event) override {
        resetSession(event);
    }

    void reset(const fcitx::InputMethodEntry & /*entry*/,
               fcitx::InputContextEvent &event) override {
        resetSession(event);
    }

    /// 状态栏副模式：中/英（引擎侧中英态）。
    // (subMode 已按 Linux 策略移除)

    /// 鼠标点击候选（页内下标）：与数字选重同语义（学习、无闪帧）。
    /// `context_` 必须在调用期间置位——commit/update 回调靠它清理面板。
    void selectCandidate(fcitx::InputContext *inputContext, int32_t index) {
        context_ = inputContext;
        hufu_client_select(engine_, index);
        context_ = nullptr;
        // 鼠标选重同样带音效（引擎在 select 回包里给 tag）
        sound_.drain(engine_);
    }

private:
    /// 清引擎会话 + UI（activate/deactivate/reset 共用）。
    void resetSession(fcitx::InputContextEvent &event) {
        // 失焦 / 切换输入法 / 重置：撤防字反查并收掉两排（本层视图清掉后，引擎 focus
        // 的 update 会照快照重画面板，不会把旧的两排留在屏幕上）。
        disarmCharLookup(event.inputContext());
        context_ = event.inputContext();
        hufu_client_focus(engine_); // 清会话与文章尾巴，保留中英态
        context_ = nullptr;
    }

    /// 状态菜单：一个「虎符」子菜单（`SimpleAction` + 自定义 `Action` 项），
    /// 构造时建好，本输入法激活时挂到该输入上下文的状态区。
    /// 引擎侧动作用 daemon 既有 op，本层只做薄封装（协议不动）。
    void setupStatusMenu() {
        menuAction_.setShortText("虎符");
        // 状态区图标用本包自带的主题名（与 `conf/hufu.inputmethod.conf` 的 `Icon` 一致，
        // 由 install.sh 装到 hicolor）：不设时 fcitx5 回退到输入法条目图标，而条目图标
        // 若指向别的包（如曾用的 fcitx-tiger）在缺包机器上就是缺图占位。
        menuAction_.setIcon("hufu");
        // 1) 重载码表：引擎侧当前方案原样重载（改码表/补充语料后免重启生效）。
        reloadAction_ = std::make_unique<HufuMenuAction>(
            [] { return std::string("重载码表"); },
            [this] { reloadSchema(); });
        instance_->userInterfaceManager().registerAction("hufu-reload-schema",
                                                         reloadAction_.get());
        // 2) 打开方案文件夹：引擎侧打开当前方案码表目录。
        openDirAction_ = std::make_unique<HufuMenuAction>(
            [] { return std::string("打开方案文件夹"); },
            [this] { openSchemaDir(); });
        instance_->userInterfaceManager().registerAction("hufu-open-schema-dir",
                                                         openDirAction_.get());
        // 3) 按键音效（默认关）：勾选态取引擎（sound_state），点击是引擎侧取反 + 落盘。
        soundAction_ = std::make_unique<HufuToggleAction>(
            "按键音效", [this] { return soundOn_ == 1; },
            [this](fcitx::InputContext *inputContext) {
                toggleSound(inputContext);
            });
        instance_->userInterfaceManager().registerAction("hufu-sound",
                                                         soundAction_.get());
        // 4) 引擎状态（信息行，不可点）：连接状态 + 当前方案名，文案取缓存
        //（`shortText` 会被 UI 线程反复调用，不能在里面做 socket 往返）。
        statusAction_ = std::make_unique<HufuMenuAction>(
            [this] { return statusText_; });
        instance_->userInterfaceManager().registerAction("hufu-status",
                                                         statusAction_.get());
        menu_.addAction(reloadAction_.get());
        menu_.addAction(openDirAction_.get());
        menu_.addAction(soundAction_.get());
        menu_.addAction(statusAction_.get());
        // 5) 候选窗显示预编辑（宿主项，默认开）：本层直接生效，不打扰引擎；
        //    切换后按该输入上下文的最近一次快照立即重放（见 `togglePanelPreedit`）。
        panelPreeditAction_ = std::make_unique<HufuToggleAction>(
            "候选窗显示预编辑",
            [this] { return config_.behavior->panelPreedit.value(); },
            [this](fcitx::InputContext *inputContext) {
                togglePanelPreedit(inputContext);
            });
        instance_->userInterfaceManager().registerAction("hufu-panel-preedit",
                                                         panelPreeditAction_.get());
        menu_.addAction(panelPreeditAction_.get());
        menuAction_.setMenu(&menu_);
        instance_->userInterfaceManager().registerAction("hufu-menu",
                                                         &menuAction_);
    }

    /// 把「虎符」子菜单挂到当前输入上下文的状态区（仅本输入法激活时显示）。
    /// `StatusGroup::InputMethod` 是 fcitx5 留给输入法自己的组，会在
    /// `InputMethodEngine::activate` 前被清空，故这里先 `clearGroup` 再挂，幂等。
    void updateStatusArea(fcitx::InputContext *inputContext) {
        if (inputContext == nullptr) {
            return;
        }
        auto &statusArea = inputContext->statusArea();
        statusArea.clearGroup(fcitx::StatusGroup::InputMethod);
        statusArea.addAction(fcitx::StatusGroup::InputMethod, &menuAction_);
        // 挂上之后取一次信息行/勾选态（时点见 `refreshStatus`）。
        refreshStatus(inputContext);
    }

    /// 摘掉**所有**输入上下文状态区里指向本对象成员的裸指针（`&menuAction_` 及其子菜单）。
    ///
    /// 用 `InputContextManager::foreach`（遍历现存 IC）+ `StatusArea::clearGroup`
    ///（逐个 `removeAction`）。这也**不替代** fcitx5 自带清理：`StatusArea::addAction`
    /// 连了 `Action::ObjectDestroyed`（动作析构时自摘），本调用是在成员仍有效时先把状态区
    /// 清干净，不把安全性寄托在「析构中途才触发的自清」上。
    void clearStatusAreas() {
        if (instance_ == nullptr) {
            return;
        }
        instance_->inputContextManager().foreach([](fcitx::InputContext *ic) {
            ic->statusArea().clearGroup(fcitx::StatusGroup::InputMethod);
            return true;
        });
    }

    /// 状态菜单里「现取」的两项内容：信息行文案与音效勾选态。
    /// 时点：状态区刷新（activate）与每次菜单动作之后——`shortText` / `isChecked`
    /// 会被 UI 线程反复调用，不能在那里做 socket 往返（引擎挂起时最坏等一个读超时）。
    /// `inputContext` 为空（如构造期、托盘调用）时只更新缓存，不通知 UI。
    void refreshStatus(fcitx::InputContext *inputContext) {
        refreshStatusText();
        if (engine_ != nullptr) {
            soundOn_ = hufu_client_sound_state(engine_);
        }
        if (inputContext == nullptr) {
            return;
        }
        if (statusAction_ != nullptr) {
            statusAction_->update(inputContext);
        }
        if (soundAction_ != nullptr) {
            soundAction_->update(inputContext);
        }
    }

    /// 信息行文案 = 连接状态（`ping`；不可达时补 `hufu_client_status` 的失败原因）
    /// + 当前方案名（配置键 `schema.current`）。
    /// 状态串只在失败路径更新（「连接失败: …」「请求失败（已断开）: …」），故只在不可达
    /// 时展示——连上时它还是创建客户端时的那句「未连接」，摆出来会误导。
    void refreshStatusText() {
        if (engine_ == nullptr) {
            statusText_ = "引擎状态：不可用";
            return;
        }
        const bool alive = hufu_client_ping(engine_) == 1;
        const std::string schema = currentSchema();
        statusText_ = alive ? "引擎状态：已连接" : "引擎状态：不可达";
        if (!alive) {
            const char *status = hufu_client_status(engine_);
            if (status != nullptr && *status != '\0') {
                statusText_ += "（" + std::string(status) + "）";
            }
        }
        if (!schema.empty()) {
            statusText_ += " · " + schema;
        }
        HUFU_DEBUG() << "hufu: " << statusText_;
    }

    /// 当前方案/码表名：走引擎配置快照的 `schema.current` 键。
    /// `hufu_client_config_str` 返回客户端内部缓冲（下次调用前有效），故立即拷走；
    /// 引擎不可达或键不存在时返回空串（信息行只显示连接状态）。
    std::string currentSchema() {
        if (engine_ == nullptr || hufu_client_config_refresh(engine_) != 1) {
            return {};
        }
        const char *name = hufu_client_config_str(engine_, "schema.current");
        return name != nullptr ? std::string(name) : std::string();
    }

    /// 「重载码表」：失败只记日志并保持现状（引擎不在线时菜单项不该有任何副作用）。
    ///
    /// 成功后把字反查索引标为「未装载」：码表/注释文件可能刚被换掉，下一次触发热键
    /// 会按同一数据目录重新读取（索引是只读快照，不重新读就会一直用旧数据）。
    void reloadSchema() {
        if (engine_ == nullptr || hufu_client_reload_schema(engine_) != 1) {
            FCITX_LOGC(hufuLog, Warn) << "hufu: 重载码表失败（hufu-server 在跑吗）";
            return;
        }
        charLookupState_ = 0;
    }

    /// 「打开方案文件夹」：同上；文件管理器由引擎侧拉起，本层不碰路径。
    void openSchemaDir() {
        if (engine_ == nullptr || hufu_client_open_schema_dir(engine_) != 1) {
            FCITX_LOGC(hufuLog, Warn)
                << "hufu: 打开方案文件夹失败（hufu-server 在跑吗）";
        }
    }

    /// 「按键音效」：引擎侧取反并落盘（热生效）；返回新态，-1=未知（引擎不在线）
    /// 时保持原勾选态——不本地假翻转，避免菜单显示与引擎实际状态不一致。
    void toggleSound(fcitx::InputContext *inputContext) {
        if (engine_ == nullptr) {
            return;
        }
        const int32_t next = hufu_client_sound_toggle(engine_);
        if (next < 0) {
            FCITX_LOGC(hufuLog, Warn) << "hufu: 音效开关失败（hufu-server 在跑吗）";
        } else {
            soundOn_ = next;
        }
        HUFU_DEBUG() << "hufu: 按键音效 -> " << soundOn_;
        if (inputContext != nullptr && soundAction_ != nullptr) {
            soundAction_->update(inputContext);
        }
    }

    /// 「候选窗显示预编辑」（宿主项，默认开）：改配置 → 落盘 → 按该输入上下文的
    /// 最近一次快照**立即**重放（面板马上显示/隐藏编码，不必等下一次按键）。
    ///
    /// 落盘前先 `fcitx::readAsIni(config_, kConfigPath)` 合并磁盘现状：本层只改这
    /// 一项，若把内存里的整份 schema 直接写回，会覆盖配置页刚保存的其它项（配置页与
    /// 本菜单是两条写入路径）；读进来的值随后被本项覆盖再落盘，等价于「读-改-写」。
    /// 兜底：托盘调用拿不到输入上下文（`inputContext == nullptr`）或该上下文还没出过
    /// UI（无快照）时只落盘，下一次 `applyUpdate` 自然按新值渲染。
    void togglePanelPreedit(fcitx::InputContext *inputContext) {
        fcitx::readAsIni(config_, kConfigPath);
        // `Option::operator->` 是 const 限定（只读视图），写入口是 `mutableValue()`。
        HufuBehaviorConfig *behavior = config_.behavior.mutableValue();
        behavior->panelPreedit.setValue(!behavior->panelPreedit.value());
        if (!fcitx::safeSaveAsIni(config_, kConfigPath)) {
            FCITX_LOGC(hufuLog, Warn) << "hufu: 写入 " << kConfigPath
                                      << " 失败（候选窗显示预编辑）";
        }
        HUFU_DEBUG() << "hufu: 候选窗显示预编辑 -> "
                     << config_.behavior->panelPreedit.value();
        if (HufuUiState *state = uiState(inputContext);
            state != nullptr && state->snapshot().valid) {
            render(inputContext, state->snapshot(), state->charLookup());
        }
        if (inputContext != nullptr && panelPreeditAction_ != nullptr) {
            panelPreeditAction_->update(inputContext);
        }
    }

    /// 该输入上下文对应的宿主侧 UI 快照属性（未注册或尚未创建时为空指针；属性类型
    /// 由工厂的 `PropertyType` 带出，无需强转）。
    HufuUiState *uiState(fcitx::InputContext *inputContext) const {
        if (inputContext == nullptr) {
            return nullptr;
        }
        return inputContext->propertyFor(&uiStateFactory_);
    }

    /// 字反查触发键命中判定（宿主项，默认 `~`；设置页清空该项即关闭本功能）。
    ///
    /// 产字符的键按「实际产生的字符」比对：`~` 物理上是 Shift+`` ` ``，前端可能上报
    /// `asciitilde`，也可能原样上报 `grave`+Shift，两个形态都算命中（Shift 交由
    /// `triggerCharOf` 归一）；不产字符的键（功能键等）按 keysym 比对。Ctrl/Alt/Super
    /// 必须与配置一致，Shift 不参与比对（已由字符归一表达）。
    bool charLookupTriggered(const fcitx::Key &pressed) const {
        const fcitx::Key &configured = config_.hotkeys->charLookup.value();
        if (configured.sym() == FcitxKey_None) {
            return false; // 未绑定（清空）
        }
        const fcitx::KeyStates kModMask = fcitx::KeyStates(fcitx::KeyState::Ctrl) |
                                          fcitx::KeyState::Alt |
                                          fcitx::KeyState::Super;
        if ((pressed.states() & kModMask).toInteger() !=
            (configured.states() & kModMask).toInteger()) {
            return false;
        }
        const char want = triggerCharOf(configured);
        if (want == 0) {
            return pressed.sym() == configured.sym();
        }
        return triggerCharOf(pressed) == want;
    }

    /// 装载字反查索引：**首次触发才装载**（不在构造期读文件/解析词典），
    /// 装载失败只记一次 Warn 并静默关闭本功能（不重试——数据是安装期产物，
    /// 反复重扫只会刷日志）。
    bool ensureCharLookup() {
        if (charLookupState_ != 0) {
            return charLookupState_ == 1;
        }
        const std::string root = hufuRootDir();
        if (engine_ == nullptr) {
            charLookupState_ = -1;
            FCITX_LOGC(hufuLog, Warn) << "hufu: 字反查数据不可用（客户端未创建）";
        } else if (root.empty()) {
            charLookupState_ = -1;
            FCITX_LOGC(hufuLog, Warn)
                << "hufu: 字反查数据不可用（XDG_DATA_HOME 与 HOME 皆未设置）";
        } else if (hufu_client_char_lookup_init(engine_, root.c_str()) != 1) {
            charLookupState_ = -1;
            FCITX_LOGC(hufuLog, Warn)
                << "hufu: 字反查数据不可用（" << root
                << " 下缺 数据/注释/拼音.注释 与 码表/虎码单字/tiger.dict.yaml）";
        } else {
            charLookupState_ = 1;
            HUFU_DEBUG() << "hufu: 字反查索引已装载（" << root << "）";
        }
        return charLookupState_ == 1;
    }

    /// 触发键按下：**武装** + 按当前光标位置查询并显示两排。
    ///
    /// 触发键在 `keyEvent` 里已被消费；这里只在**能查**时武装——数据不可用或应用
    /// 不支持周边文本都不武装（那两种情况下方向键重查也不会有结果，各记一条 Debug，
    /// 与既有的静默降级一致）。光标左侧不是汉字时清两排但**保持武装**：用户多半正是
    /// 要用方向键移到一个字上。
    void triggerCharLookup(fcitx::InputContext *inputContext) {
        HufuUiState *state = uiState(inputContext);
        if (inputContext == nullptr || state == nullptr) {
            return;
        }
        if (!ensureCharLookup()) {
            HUFU_DEBUG() << "hufu: 字反查跳过（数据不可用）";
            return;
        }
        if (!inputContext->capabilityFlags().test(
                fcitx::CapabilityFlag::SurroundingText)) {
            HUFU_DEBUG() << "hufu: 字反查跳过（应用不支持周边文本）";
            return;
        }
        state->charLookupArmed() = true;
        refreshCharLookup(inputContext);
    }

    /// 按**当前**光标位置重查并更新两排（触发键与武装期间的延迟重查共用）。
    /// 周边文本不可用或左侧不是汉字：清两排，但保持武装。
    void refreshCharLookup(fcitx::InputContext *inputContext) {
        HufuUiState *state = uiState(inputContext);
        if (state == nullptr || !state->charLookupArmed()) {
            return;
        }
        uint32_t ucs4 = fcitx::utf8::INVALID_CHAR;
        if (!charLeftOfCursor(inputContext, ucs4)) {
            hideCharLookup(inputContext);
            return;
        }
        const char *row = hufu_client_char_lookup(engine_, ucs4);
        const std::vector<std::string> columns =
            splitTabs(row != nullptr ? row : "");
        // 缺列按 `?` 显示：能查到「这个字没有数据」本身也是信息。
        const std::string pinyin =
            !columns.empty() && !columns[0].empty() ? columns[0] : "?";
        std::string code =
            columns.size() > 1 && !columns[1].empty() ? columns[1] : "?";
        // 拆分（可选第三列）追加在下排；上排排头「咅」= 拼音、下排排头「虍」= 虎码，
        // 两个排头用于一眼分清上下两排（与参照实现的观感一致）。
        if (columns.size() > 2 && !columns[2].empty()) {
            code += " · " + columns[2];
        }
        showCharLookup(inputContext, "咅 " + pinyin, "虍 " + code);
    }

    /// 取光标左侧那个字符（读 `surroundingText()`）。false = 周边文本不可用 /
    /// 光标左侧没有字符 / 不是汉字——三种都不出提示，各记一条 Debug。
    bool charLeftOfCursor(fcitx::InputContext *inputContext, uint32_t &ucs4) {
        const auto &surrounding = inputContext->surroundingText();
        if (!surrounding.isValid()) {
            HUFU_DEBUG() << "hufu: 字反查跳过（周边文本不可用）";
            return false;
        }
        // `cursor()` 是**字符（码点）偏移**（不是字节偏移）：先按字符切出光标左侧
        // 前缀，再取该前缀的最后一个字符。
        const std::string &text = surrounding.text();
        const auto end = fcitx::utf8::nextNChar(text.begin(), surrounding.cursor());
        if (end == text.begin()) {
            HUFU_DEBUG() << "hufu: 字反查跳过（光标左侧没有字符）";
            return false;
        }
        ucs4 = fcitx::utf8::getLastChar(text.begin(), end);
        if (!isHanCodePoint(ucs4)) {
            HUFU_DEBUG() << "hufu: 字反查跳过（光标左侧不是汉字）";
            return false;
        }
        return true;
    }

    /// 安排一次延迟重查（武装期间方向键透传之后）。
    ///
    /// 已有待处理定时器就不再安排——连续按键合并成一次；也**不**把定时器往后推，
    /// 故按住方向键时结果仍持续跟随（每个 30ms 窗口最多重查一次）。
    void scheduleCharLookupRefresh(fcitx::InputContext *inputContext) {
        if (inputContext == nullptr || instance_ == nullptr) {
            return;
        }
        reapCharLookupTimer();
        if (lookupTimer_) {
            return; // 已有待处理：合并
        }
        // 弱引用目标输入上下文：IC 可能先于本引擎析构（见 `HufuUiState` 契约）。
        lookupContext_ = inputContext->watch();
        lookupTimer_ = instance_->eventLoop().addTimeEvent(
            CLOCK_MONOTONIC, fcitx::now(CLOCK_MONOTONIC) + kCharLookupRefreshUsec,
            0, [this](fcitx::EventSourceTime * /*source*/, uint64_t /*usec*/) {
                lookupTimerSpent_ = true; // 单次：标记待回收（见 reapCharLookupTimer）
                refreshCharLookup(lookupContext_.get());
                return true;
            });
        lookupTimer_->setOneShot();
    }

    /// 回收**已触发**的定时器：sd-event 派发期间仍持有事件源，在它的回调里销毁它
    /// 不安全，故回收推迟到回调之外——下一次安排重查或撤防时做。
    void reapCharLookupTimer() {
        if (lookupTimerSpent_) {
            lookupTimer_.reset();
            lookupTimerSpent_ = false;
        }
    }

    /// 撤掉待处理的延迟重查（其它按键 / Esc / 失焦 / 切换输入法 / 析构）：
    /// 未触发的定时器在这里销毁，回调不会再跑。
    void cancelCharLookupRefresh() {
        lookupTimer_.reset();
        lookupTimerSpent_ = false;
        lookupContext_ = fcitx::TrackableObjectReference<fcitx::InputContext>();
    }

    /// 撤防 + 清两排 + 撤掉待处理的延迟重查。
    void disarmCharLookup(fcitx::InputContext *inputContext) {
        cancelCharLookupRefresh();
        if (HufuUiState *state = uiState(inputContext); state != nullptr) {
            state->charLookupArmed() = false;
        }
        hideCharLookup(inputContext);
    }

    /// 显示两排字反查提示：只写 aux 上/下排——不占候选列表、也不伪造 preedit
    /// （引擎的候选与预编辑原样留在面板上）。
    void showCharLookup(fcitx::InputContext *inputContext, const std::string &up,
                        const std::string &down) {
        HufuUiState *state = uiState(inputContext);
        if (state == nullptr) {
            return;
        }
        HufuCharLookupView &view = state->charLookup();
        view.shown = true;
        view.up = up;
        view.down = down;
        inputContext->inputPanel().setAuxUp(fcitx::Text(view.up));
        inputContext->inputPanel().setAuxDown(fcitx::Text(view.down));
        inputContext->updateUserInterface(
            fcitx::UserInterfaceComponent::InputPanel);
        HUFU_DEBUG() << "hufu: 字反查 [" << view.up << "] [" << view.down << "]";
    }

    /// 收掉两排字反查提示（任意键 / Esc / 失焦）：面板还原成「引擎 aux（上排）+
    /// 无下排」——这正是 `render` 在未显示字反查时的写法。
    void hideCharLookup(fcitx::InputContext *inputContext) {
        HufuUiState *state = uiState(inputContext);
        if (state == nullptr || !state->charLookup().shown) {
            return;
        }
        state->charLookup() = HufuCharLookupView();
        const std::string &aux = state->snapshot().aux;
        inputContext->inputPanel().setAuxUp(!aux.empty() ? fcitx::Text(aux)
                                                        : fcitx::Text());
        inputContext->inputPanel().setAuxDown(fcitx::Text());
        inputContext->updateUserInterface(
            fcitx::UserInterfaceComponent::InputPanel);
    }

    static void commitCallback(void *user, const char *text) {
        static_cast<HufuEngine *>(user)->applyCommit(text);
    }

    static void updateCallback(void *user, const char *preedit, const char *raw,
                               const char *const *texts,
                               const char *const *comments,
                               const char *const *commitTexts, int32_t count,
                               int32_t selected, const char *aux,
                               int32_t chinese) {
        static_cast<HufuEngine *>(user)->applyUpdate(preedit, raw, texts,
                                                     comments, commitTexts,
                                                     count, selected, aux,
                                                     chinese);
    }

    /// 上屏：先按引擎回删数清掉已上屏字符（如「1.」→「。」），再提交。
    void applyCommit(const char *text) {
        if (context_ == nullptr || text == nullptr || *text == '\0') {
            return;
        }
        const int32_t back = hufu_client_last_back(engine_);
        if (back > 0) {
            if (context_->capabilityFlags().test(
                    fcitx::CapabilityFlag::SurroundingText)) {
                context_->deleteSurroundingText(-back, back);
            } else {
                for (int32_t i = 0; i < back; ++i) {
                    context_->forwardKey(fcitx::Key(FcitxKey_BackSpace));
                }
            }
        }
        context_->commitString(text);
    }

    /// UI 更新回调：**先记快照（宿主侧），再 render**——分两段是为了宿主开关
    ///（「候选窗显示预编辑」）切换后能按最近一次快照立即重放，不必等下一次按键。
    /// 快照记在该输入上下文的属性上（见 `HufuUiSnapshot`）。
    /// `commitTexts`：候选的实际上屏文本（`显示=>输出` 覆盖时与显示不同），
    /// 顶字（Shift+字母）用。
    void applyUpdate(const char *preedit, const char *raw,
                     const char *const *texts, const char *const *comments,
                     const char *const *commitTexts, int32_t count,
                     int32_t selected, const char *aux, int32_t chinese) {
        if (context_ == nullptr) {
            return;
        }
        const std::string rawString = raw != nullptr ? raw : "";
        const std::string preeditString = preedit != nullptr ? preedit : "";
        const std::string auxString = aux != nullptr ? aux : "";
        // 【切换气泡修复 2026-09-19】没有我方内容时**完全不碰输入面板**：
        // fcitx5 的「输入法信息」气泡画在 InputPanel overlay 上，只在面板
        // 为空时显示；此前 activate/deactivate（无组段也）无条件下发
        // setPreedit/候选/aux + updateUserInterface，把别的输入法/系统刚
        // 弹的气泡顶掉——用户实测：虎符在输入法列表里时，切换气泡全部
        // 不显示（删掉虎符即恢复）。
        const bool hadComposition = hasComposition_;
        const bool emptyState = count <= 0 && rawString.empty() &&
                                preeditString.empty() && auxString.empty();
        hasComposition_ = count > 0 || !rawString.empty();
        topCommit_ = (count > 0 && commitTexts != nullptr &&
                      commitTexts[0] != nullptr)
                         ? commitTexts[0]
                         : "";
        // 【选重闪帧】数字/; 选重上屏：引擎回「raw 空 + 旧候选 + 高亮」的
        // 确认帧（raw/preedit 已清）。Windows 侧靠 150ms 收场钟清窗；
        // Linux 无皮肤动效，直接清（否则候选窗滞留——用户实测反馈）。
        const bool flashFrame = rawString.empty() && count > 0;
        // 1) 记快照。闪帧记成**空**快照：那些候选已经作废，若留在快照里，
        // 宿主开关重放会把它们又画回面板。空态（无组段）也照记——否则属性里
        // 留着上一轮的旧组段，重放时复活。
        HufuUiSnapshot snapshot;
        snapshot.valid = true;
        snapshot.chinese = chinese == 1;
        if (flashFrame) {
            hasComposition_ = false;
            topCommit_.clear();
        } else {
            snapshot.preedit = preeditString;
            snapshot.selected = selected;
            snapshot.aux = auxString;
            for (int32_t i = 0; i < count; ++i) {
                snapshot.texts.emplace_back(
                    texts != nullptr && texts[i] != nullptr ? texts[i] : "");
                snapshot.comments.emplace_back(
                    comments != nullptr && comments[i] != nullptr ? comments[i]
                                                                  : "");
            }
        }
        HufuUiState *state = uiState(context_);
        if (state != nullptr) {
            state->snapshot() = snapshot;
        }
        if (emptyState && !hadComposition) {
            return; // 面板本来就没有我方内容：不发 UI 更新（保护系统 overlay）
        }
        HUFU_DEBUG() << "hufu: 快照 preedit=\"" << snapshot.preedit << "\" 候选="
                     << snapshot.texts.size() << " 高亮=" << snapshot.selected
                     << " 中英=" << (snapshot.chinese ? "中" : "英");
        // 2) 按快照渲染（空快照 = 清面板）；字反查视图一并带上，两排 aux 归它管。
        render(context_, snapshot,
               state != nullptr ? state->charLookup() : HufuCharLookupView());
    }

    /// 按快照刷新一个输入上下文的输入面板（`applyUpdate` 与宿主开关共用）。
    /// `lookup` 是该输入上下文的字反查视图：显示中则两排 aux 归它，否则上排用引擎 aux。
    void render(fcitx::InputContext *inputContext,
                const HufuUiSnapshot &snapshot,
                const HufuCharLookupView &lookup) {
        if (inputContext == nullptr) {
            return;
        }
        const fcitx::Text preeditText(snapshot.preedit);
        // 【候选窗预编辑】宿主项（默认开）：托盘「候选窗显示预编辑」/设置页
        // 行为 → 候选窗内显示编码；关闭后编码仍随光标内联显示。
        inputContext->inputPanel().setPreedit(
            config_.behavior->panelPreedit.value() ? preeditText : fcitx::Text());
        // 客户端内联预编辑：跟随 fcitx5 全局预编辑设置
        inputContext->inputPanel().setClientPreedit(
            inputContext->isPreeditEnabled() ? preeditText : fcitx::Text());
        inputContext->updatePreedit();

        // 候选：无候选置 nullptr 清除（fcitx5 约定，不可留空列表）。
        // 翻页/数字选重由引擎消费，本层不自作分页（页大小=当前页数量）。
        if (snapshot.texts.empty()) {
            inputContext->inputPanel().setCandidateList(nullptr);
        } else {
            auto candidateList = std::make_unique<fcitx::CommonCandidateList>();
            for (size_t i = 0; i < snapshot.texts.size(); ++i) {
                candidateList->append<HufuCandidateWord>(
                    fcitx::Text(snapshot.texts[i]),
                    fcitx::Text(snapshot.comments[i]), watch(),
                    static_cast<int32_t>(i));
            }
            // 设置页「候选排列」：跟随全局时不下发布局提示（由 fcitx5 全局「候选竖排」
            // 决定），横排 / 竖排时强制对应方向。
            const auto layout = config_.behavior->candidateLayout.value();
            if (layout != HufuCandidateLayout::FollowGlobal) {
                candidateList->setLayoutHint(
                    layout == HufuCandidateLayout::Vertical
                        ? fcitx::CandidateLayoutHint::Vertical
                        : fcitx::CandidateLayoutHint::Horizontal);
            }
            candidateList->setPageSize(static_cast<int>(snapshot.texts.size()));
            const int32_t index = std::min(
                std::max(snapshot.selected, 0),
                static_cast<int32_t>(snapshot.texts.size()) - 1);
            candidateList->setGlobalCursorIndex(index);
            inputContext->inputPanel().setCandidateList(std::move(candidateList));
        }
        // aux 两排：字反查显示中时上排「咅 …」、下排「虍 …」；否则上排是引擎 aux、
        // 下排留空（引擎没有第二排，下排只归字反查用）。
        if (lookup.shown) {
            inputContext->inputPanel().setAuxUp(fcitx::Text(lookup.up));
            inputContext->inputPanel().setAuxDown(fcitx::Text(lookup.down));
        } else {
            inputContext->inputPanel().setAuxUp(
                !snapshot.aux.empty() ? fcitx::Text(snapshot.aux) : fcitx::Text());
            inputContext->inputPanel().setAuxDown(fcitx::Text());
        }
        inputContext->updateUserInterface(
            fcitx::UserInterfaceComponent::InputPanel);
    }

    /// 引擎 → schema（引擎映射项）。引擎不可达时保持现状。
    /// 用 RawConfig + load（与配置工具读 ini 同一路径；子配置项不能
    /// 直接 setValue）。宿主项（候选窗内编码/强制竖排）填当前值。
    void pullConfig() {
        if (engine_ == nullptr || hufu_client_config_refresh(engine_) != 1) {
            return;
        }
        const auto getBool = [&](const char *path, bool fallback) {
            const int v = hufu_client_config_bool(engine_, path);
            return v < 0 ? fallback : v == 1;
        };
        const auto getInt = [&](const char *path, int fallback) {
            int64_t v = 0;
            return hufu_client_config_int(engine_, path, &v) == 1
                       ? static_cast<int>(v)
                       : fallback;
        };
        const auto getStr = [&](const char *path, const std::string &fallback) {
            const char *s = hufu_client_config_str(engine_, path);
            return (s != nullptr && *s != '\0') ? std::string(s) : fallback;
        };
        fcitx::RawConfig raw;
        raw.setValueByPath("Behavior/PanelPreedit",
                           jbool(config_.behavior->panelPreedit.value()));
        // 枚举项经 marshaller 取名字符串（与配置页 ini 里的写法一致）。
        {
            fcitx::RawConfig enumValue;
            fcitx::DefaultMarshaller<HufuCandidateLayout>{}.marshall(
                enumValue, config_.behavior->candidateLayout.value());
            raw.setValueByPath("Behavior/CandidateLayout", enumValue.value());
        }
        raw.setValueByPath("Behavior/PageSize",
                           std::to_string(getInt("candidates.page_size", 4)));
        raw.setValueByPath("Behavior/AutoPush",
                           jbool(getBool("input.auto_push", true)));
        raw.setValueByPath(
            "Behavior/AutoSelectUnique",
            jbool(getBool("input.auto_select_unique", false)));
        raw.setValueByPath(
            "Behavior/AutoClearEmpty",
            jbool(getBool("input.auto_clear_empty", false)));
        raw.setValueByPath("Behavior/EnterClear",
                           jbool(getBool("input.enter_clear", false)));
        raw.setValueByPath("Punct/FullShape",
                           jbool(getBool("punct.full_shape", true)));
        raw.setValueByPath("Punct/AsciiPunct",
                           jbool(getBool("input.ascii_punct", false)));
        raw.setValueByPath("Filter/OpenCC",
                           jbool(getBool("opencc.enabled", false)));
        raw.setValueByPath("Filter/ToTraditional",
                           jbool(getBool("opencc.to_traditional", true)));
        raw.setValueByPath("Filter/Emoji",
                           jbool(getBool("opencc.emoji", false)));
        raw.setValueByPath(
            "Filter/ShowPinyin",
            jbool(getBool("candidates.show_pinyin_comment", false)));
        raw.setValueByPath(
            "Filter/ShowUnicode",
            jbool(getBool("candidates.show_unicode_comment", true)));
        raw.setValueByPath("Filter/ShowSplit",
                           jbool(getBool("candidates.show_split", true)));
        raw.setValueByPath("Sentence/Sentence",
                           jbool(getBool("sentence.enabled", true)));
        raw.setValueByPath("Sentence/Rerank",
                           jbool(getBool("sentence.rerank.enabled", true)));
        raw.setValueByPath("Reverse/ReverseEnabled",
                           jbool(getBool("reverse.enabled", true)));
        raw.setValueByPath("Reverse/ReversePrefix",
                           getStr("reverse.prefix", "`"));
        raw.setValueByPath("Sound/SoundEnabled",
                           jbool(getBool("sound.enabled", false)));
        raw.setValueByPath("Sound/SoundVolume",
                           std::to_string(getInt("sound.volume", 50)));
        raw.setValueByPath("Keys/SecondSelect",
                           getStr("candidates.second_select", ";"));
        raw.setValueByPath("Keys/ThirdSelect",
                           getStr("candidates.third_select", "'"));
        raw.setValueByPath("Keys/PagingKeys",
                           getStr("candidates.paging_keys", "-="));
        config_.load(raw, true);
    }

    /// schema → 引擎（JSON 补丁；仅引擎映射项；深合并，热生效）。
    void pushConfig() {
        if (engine_ == nullptr) {
            return;
        }
        const auto &b = config_.behavior.value();
        const auto &p = config_.punct.value();
        const auto &f = config_.filter.value();
        const auto &s = config_.sentence.value();
        const auto &r = config_.reverse.value();
        const auto &so = config_.sound.value();
        const auto &k = config_.keys.value();
        const auto firstChar = [](const std::string &v, char fallback) {
            return v.empty() ? fallback : v[0];
        };
        std::string patch = "{";
        patch += "\"candidates\":{";
        patch += "\"page_size\":" + std::to_string(b.pageSize.value()) + ",";
        patch += "\"second_select\":\"" +
                 jesc(std::string(1, firstChar(k.secondSelect.value(), ';'))) +
                 "\",";
        patch += "\"third_select\":\"" +
                 jesc(std::string(1, firstChar(k.thirdSelect.value(), '\''))) +
                 "\",";
        patch += "\"paging_keys\":\"" + jesc(k.pagingKeys.value()) + "\",";
        patch += "\"show_pinyin_comment\":" + std::string(jbool(f.showPinyin.value())) + ",";
        patch += "\"show_unicode_comment\":" + std::string(jbool(f.showUnicode.value())) + ",";
        patch += "\"show_split\":" + std::string(jbool(f.showSplit.value())) + "},";
        patch += "\"input\":{";
        patch += "\"auto_push\":" + std::string(jbool(b.autoPush.value())) + ",";
        patch += "\"auto_select_unique\":" + std::string(jbool(b.autoSelectUnique.value())) + ",";
        patch += "\"auto_clear_empty\":" + std::string(jbool(b.autoClearEmpty.value())) + ",";
        patch += "\"enter_clear\":" + std::string(jbool(b.enterClear.value())) + ",";
        patch += "\"ascii_punct\":" + std::string(jbool(p.asciiPunct.value())) + "},";
        patch += "\"punct\":{\"full_shape\":" + std::string(jbool(p.fullShape.value())) + "},";
        patch += "\"opencc\":{\"enabled\":" + std::string(jbool(f.opencc.value())) +
                 ",\"to_traditional\":" + std::string(jbool(f.toTraditional.value())) +
                 ",\"emoji\":" + std::string(jbool(f.emoji.value())) + "},";
        patch += "\"sentence\":{\"enabled\":" + std::string(jbool(s.sentence.value())) +
                 ",\"rerank\":{\"enabled\":" + std::string(jbool(s.rerank.value())) + "}},";
        patch += "\"reverse\":{\"enabled\":" + std::string(jbool(r.enabled.value())) +
                 ",\"prefix\":\"" + jesc(r.prefix.value()) + "\"},";
        patch += "\"sound\":{\"enabled\":" + std::string(jbool(so.enabled.value())) +
                 ",\"volume\":" + std::to_string(so.volume.value()) + "}}";
        if (hufu_client_config_patch(engine_, patch.c_str()) != 1) {
            FCITX_LOGC(hufuLog, Warn) << "hufu: 配置写入引擎失败（hufu-server 在跑吗）";
        }
    }

    fcitx::Instance *instance_;
    hufu_client *engine_ = nullptr;
    fcitx::InputContext *context_ = nullptr;
    /// fcitx5 设置页 schema（fcitx5-configtool）
    HufuConfig config_;
    /// 每输入上下文宿主侧 UI 快照的工厂（见 `HufuUiState`；构造时注册）
    fcitx::FactoryFor<HufuUiState> uiStateFactory_;
    /// 有编码或候选（更新回调维护；Shift+字母「顶字」判定用）
    bool hasComposition_ = false;
    /// 首选候选的实际上屏文本（含 `显示=>输出` 覆盖；顶字用）
    std::string topCommit_;
    /// 状态区菜单：「虎符」子菜单与四项（见 `setupStatusMenu`）
    fcitx::Menu menu_;
    fcitx::SimpleAction menuAction_;
    std::unique_ptr<HufuMenuAction> reloadAction_;
    std::unique_ptr<HufuMenuAction> openDirAction_;
    std::unique_ptr<HufuToggleAction> soundAction_;
    std::unique_ptr<HufuMenuAction> statusAction_;
    /// 宿主项开关「候选窗显示预编辑」（不进引擎选项，直接生效的配置项）
    std::unique_ptr<HufuToggleAction> panelPreeditAction_;
    /// 「引擎状态」行缓存文案（`shortText` 不做 socket 往返，见 `refreshStatus`）
    std::string statusText_;
    /// 按键音效勾选态缓存：1=开 / 0=关 / -1=未知（未取到；菜单显示为未勾选）
    int32_t soundOn_ = -1;
    /// 按键音效播放通道（宿主侧；启用态由上面的刷新点写入）
    HufuSoundChannel sound_;
    /// 字反查索引装载状态：0=未装载（首次触发才装载）/ 1=可用 / -1=不可用（静默关闭）
    int32_t charLookupState_ = 0;
    /// 字反查武装期间「方向键透传后延迟重查」的定时器（`nullptr`=没有待处理的重查）
    std::unique_ptr<fcitx::EventSourceTime> lookupTimer_;
    /// `lookupTimer_` 是否已触发（单次定时器触发后等回调之外回收，见 `reapCharLookupTimer`）
    bool lookupTimerSpent_ = false;
    /// 延迟重查的目标输入上下文（弱引用：IC 可能先于本引擎析构）
    fcitx::TrackableObjectReference<fcitx::InputContext> lookupContext_;
};

/// 点击候选 = 上屏（页内下标；语义同数字选重）。
void HufuCandidateWord::select(fcitx::InputContext *inputContext) const {
    // 引擎（addon 实例）可能已先于本候选对象析构（IC 晚于 addon，见类注释）：
    // 弱引用失效即直接返回，**不触碰**已释放的引擎。
    HufuEngine *owner = owner_.get();
    if (owner == nullptr) {
        return;
    }
    owner->selectCandidate(inputContext, index_);
}

class HufuFactory : public fcitx::AddonFactory {
public:
    fcitx::AddonInstance *create(fcitx::AddonManager *manager) override {
        return new HufuEngine(manager->instance());
    }
};

} // namespace

FCITX_ADDON_FACTORY(HufuFactory);
