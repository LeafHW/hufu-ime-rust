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

    // 【Linux 策略】不实现 subMode()：中/英副模式属引擎自带英文输入，
    // Linux 上英文由 fcitx5 键盘布局输入法提供，状态栏不再显示中/英。

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
                               const char *const *comments, int32_t count,
                               int32_t selected, const char *aux,
                               int32_t chinese) {
        static_cast<HufuEngine *>(user)->applyUpdate(preedit, raw, texts,
                                                     comments, count, selected,
                                                     aux, chinese);
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
    void applyUpdate(const char *preedit, const char *raw,
                     const char *const *texts, const char *const *comments,
                     int32_t count, int32_t selected, const char *aux,
                     int32_t chinese) {
        if (context_ == nullptr) {
            return;
        }
        const std::string rawString = raw != nullptr ? raw : "";
        // 【选重闪帧】数字/; 选重上屏：引擎回「raw 空 + 旧候选 + 高亮」的
        // 确认帧（raw/preedit 已清）。Windows 侧靠 150ms 收场钟清窗；
        // Linux 无皮肤动效，直接清（否则候选窗滞留——用户实测反馈）。
        if (rawString.empty() && count > 0) {
            context_->inputPanel().setCandidateList(nullptr);
            context_->inputPanel().setPreedit(fcitx::Text());
            context_->inputPanel().setClientPreedit(fcitx::Text());
            context_->inputPanel().setAuxUp(fcitx::Text());
            context_->updatePreedit();
            context_->updateUserInterface(
                fcitx::UserInterfaceComponent::InputPanel);
            return;
        }
        const std::string preeditString = preedit != nullptr ? preedit : "";
        const fcitx::Text preeditText(preeditString);
        context_->inputPanel().setPreedit(preeditText);
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
                candidateList->append<fcitx::DisplayOnlyCandidateWord>(
                    fcitx::Text(t), fcitx::Text(c));
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

    fcitx::Instance *instance_;
    hufu_client *engine_ = nullptr;
    fcitx::InputContext *context_ = nullptr;
};

class HufuFactory : public fcitx::AddonFactory {
public:
    fcitx::AddonInstance *create(fcitx::AddonManager *manager) override {
        return new HufuEngine(manager->instance());
    }
};

} // namespace

FCITX_ADDON_FACTORY(HufuFactory);
