// 虎符（hufu-ime）fcitx5 addon 的 C++ 薄壳：只做 fcitx5 接口适配，
// 按键经 C ABI（libhufu_fcitx5_client，Rust）走 Unix socket 到 hufu-server。
#include <fcitx/addonfactory.h>
#include <fcitx/addoninstance.h>
#include <fcitx/addonmanager.h>
#include <fcitx/candidatelist.h>
#include <fcitx/event.h>
#include <fcitx/inputcontext.h>
#include <fcitx/inputmethodengine.h>
#include <fcitx/inputmethodentry.h>
#include <fcitx/inputpanel.h>
#include <fcitx/instance.h>
#include <fcitx/surroundingtext.h>
#include <fcitx/text.h>
#include <fcitx/userinterface.h>
#include <fcitx-config/configuration.h>
#include <fcitx-config/iniparser.h>
#include <fcitx-config/option.h>
#include <fcitx-utils/capabilityflags.h>
#include <fcitx-utils/key.h>
#include <fcitx-utils/keysym.h>
#include <fcitx-utils/log.h>

#include <algorithm>
#include <memory>
#include <string>

#include "hufu_abi.h"

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

class HufuEngine;

/// 面板候选：点击（`select`）按页内下标上屏——与数字选重同语义
///（学习、无闪帧）。此前用 `DisplayOnlyCandidateWord`，点击无反应。
class HufuCandidateWord : public fcitx::CandidateWord {
public:
    HufuCandidateWord(fcitx::Text text, fcitx::Text comment,
                      HufuEngine *owner, int32_t index)
        : CandidateWord(std::move(text)), owner_(owner), index_(index) {
        setComment(std::move(comment));
    }

    void select(fcitx::InputContext *inputContext) const override;

private:
    HufuEngine *owner_;
    int32_t index_;
};

/// ── fcitx5 设置页 schema（fcitx5-configtool「虎符」页）────────────────────
/// 两类选项：
/// - 宿主项（候选窗内预编辑 / 强制竖排）：本层直接生效，只存
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
        .defaultValue = false,
        .annotation{"在候选窗顶部显示编码串；默认关（组段仍随光标内联显示）。"}}};
    fcitx::OptionWithAnnotation<bool, fcitx::ToolTipAnnotation> forceVertical{{
        .parent = this,
        .path{"ForceVertical"},
        .description{"强制竖排候选"},
        .defaultValue = false,
        .annotation{"勾选=强制竖排；不勾=跟随 fcitx5 全局候选排列设置。"}}};
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

FCITX_CONFIGURATION(
    HufuConfig,
    fcitx::Option<HufuBehaviorConfig> behavior{this, "Behavior", "行为"};
    fcitx::Option<HufuPunctConfig> punct{this, "Punct", "标点"};
    fcitx::Option<HufuFilterConfig> filter{this, "Filter", "滤镜（简繁/注释）"};
    fcitx::Option<HufuSentenceConfig> sentence{this, "Sentence", "整句"};
    fcitx::Option<HufuReverseConfig> reverse{this, "Reverse", "反查"};
    fcitx::Option<HufuSoundConfig> sound{this, "Sound", "音效"};
    fcitx::Option<HufuKeysConfig> keys{this, "Keys", "选重与翻页"};);

class HufuEngine : public fcitx::InputMethodEngine {
public:
    explicit HufuEngine(fcitx::Instance *instance) : instance_(instance) {
        hufu_host host = {};
        host.user = this;
        host.commit = &HufuEngine::commitCallback;
        host.update = &HufuEngine::updateCallback;
        // 默认 socket 路径（$XDG_RUNTIME_DIR/hufu-ime.sock）
        engine_ = hufu_client_new(nullptr, &host);
        const char *status = hufu_client_status(engine_);
        FCITX_INFO() << "hufu: client created (" << (status ? status : "") << ")";
        if (engine_ != nullptr && hufu_client_ping(engine_) == 0) {
            FCITX_WARN() << "hufu: hufu-server 不可达（先启动引擎，按键将直通）";
        }
        // 设置页：先读用户已保存值（宿主项），再以引擎配置覆盖引擎映射项
        fcitx::readAsIni(config_, "conf/hufu.conf");
        pullConfig();
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
        if (engine_ != nullptr) {
            hufu_client_free(engine_);
        }
    }

    void keyEvent(const fcitx::InputMethodEntry & /*entry*/,
                  fcitx::KeyEvent &keyEvent) override {
        if (keyEvent.isRelease()) {
            return; // 引擎只处理按下
        }
        fcitx::InputContext *inputContext = keyEvent.inputContext();
        // 【Shift 修饰修复 2026-09-19】`key()` 是「归一化」事件：Shift+符号
        // 时 Shift 被并入符号本身（states 里不再有 Shift），引擎会当成
        // 「无 shift 的普通键」——实测 Shift+, 出「，」而非《、Shift+字母
        // 被当编码。`rawKey()` 是布局转换后、保留真实修饰态的原始事件
        //（日志实测：Shift+a → Key(A states=0) / rawKey Key(Shift+A states=1)）。
        const fcitx::Key &key = keyEvent.rawKey();
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
        if (rc & HUFU_KEY_CONSUMED) {
            keyEvent.filterAndAccept();
        }
    }

    void activate(const fcitx::InputMethodEntry & /*entry*/,
                  fcitx::InputContextEvent &event) override {
        resetSession(event);
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
    }

private:
    /// 清引擎会话 + UI（activate/deactivate/reset 共用）。
    void resetSession(fcitx::InputContextEvent &event) {
        context_ = event.inputContext();
        hufu_client_focus(engine_); // 清会话与文章尾巴，保留中英态
        context_ = nullptr;
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

    /// UI 快照：preedit（面板 + 客户端内联）+ 候选列表 + aux。
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
        if (emptyState && !hadComposition) {
            return; // 面板本来就没有我方内容：不发 UI 更新（保护系统 overlay）
        }
        // 【选重闪帧】数字/; 选重上屏：引擎回「raw 空 + 旧候选 + 高亮」的
        // 确认帧（raw/preedit 已清）。Windows 侧靠 150ms 收场钟清窗；
        // Linux 无皮肤动效，直接清（否则候选窗滞留——用户实测反馈）。
        if (rawString.empty() && count > 0) {
            hasComposition_ = false;
            topCommit_.clear();
            context_->inputPanel().setCandidateList(nullptr);
            context_->inputPanel().setPreedit(fcitx::Text());
            context_->inputPanel().setClientPreedit(fcitx::Text());
            context_->inputPanel().setAuxUp(fcitx::Text());
            context_->updatePreedit();
            context_->updateUserInterface(
                fcitx::UserInterfaceComponent::InputPanel);
            return;
        }
        const fcitx::Text preeditText(preeditString);
        // 【候选窗预编辑】默认关（组段走客户端内联）；设置页可开
        // （fcitx5-configtool → 虎符 → 行为 → 候选窗内显示编码）。
        context_->inputPanel().setPreedit(
            config_.behavior->panelPreedit.value() ? preeditText : fcitx::Text());
        // 客户端内联预编辑：跟随 fcitx5 全局预编辑设置
        context_->inputPanel().setClientPreedit(
            context_->isPreeditEnabled() ? preeditText : fcitx::Text());
        context_->updatePreedit();

        // 候选：无候选置 nullptr 清除（fcitx5 约定，不可留空列表）。
        // 翻页/数字选重由引擎消费，本层不自作分页（页大小=当前页数量）。
        if (count <= 0) {
            context_->inputPanel().setCandidateList(nullptr);
        } else {
            auto candidateList = std::make_unique<fcitx::CommonCandidateList>();
            for (int32_t i = 0; i < count; ++i) {
                const char *t = texts != nullptr && texts[i] != nullptr ? texts[i] : "";
                const char *c =
                    comments != nullptr && comments[i] != nullptr ? comments[i] : "";
                candidateList->append<HufuCandidateWord>(
                    fcitx::Text(t), fcitx::Text(c), this, i);
            }
            // 设置页「强制竖排候选」：仅勾选时下发（不勾=跟随 fcitx5 全局）
            if (config_.behavior->forceVertical.value()) {
                candidateList->setLayoutHint(fcitx::CandidateLayoutHint::Vertical);
            }
            candidateList->setPageSize(count);
            const int32_t index = std::min(std::max(selected, 0), count - 1);
            candidateList->setGlobalCursorIndex(index);
            context_->inputPanel().setCandidateList(std::move(candidateList));
        }
        context_->inputPanel().setAuxUp(aux != nullptr && *aux != '\0'
                                             ? fcitx::Text(aux)
                                             : fcitx::Text());
        context_->updateUserInterface(fcitx::UserInterfaceComponent::InputPanel);
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
        raw.setValueByPath("Behavior/ForceVertical",
                           jbool(config_.behavior->forceVertical.value()));
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
            FCITX_WARN() << "hufu: 配置写入引擎失败（hufu-server 在跑吗）";
        }
    }

    fcitx::Instance *instance_;
    hufu_client *engine_ = nullptr;
    fcitx::InputContext *context_ = nullptr;
    /// fcitx5 设置页 schema（fcitx5-configtool）
    HufuConfig config_;
    /// 有编码或候选（更新回调维护；Shift+字母「顶字」判定用）
    bool hasComposition_ = false;
    /// 首选候选的实际上屏文本（含 `显示=>输出` 覆盖；顶字用）
    std::string topCommit_;
};

/// 点击候选 = 上屏（页内下标；语义同数字选重）。
void HufuCandidateWord::select(fcitx::InputContext *inputContext) const {
    owner_->selectCandidate(inputContext, index_);
}

class HufuFactory : public fcitx::AddonFactory {
public:
    fcitx::AddonInstance *create(fcitx::AddonManager *manager) override {
        return new HufuEngine(manager->instance());
    }
};

} // namespace

FCITX_ADDON_FACTORY(HufuFactory);
