/*
 * 虎符（hufu-ime）fcitx5 addon 的 C ABI（Rust 侧实现，C++ 薄壳调用）。
 * 头文件与 `hufu-fcitx5-client/src/lib.rs` 的导出符号一一对应。
 */
#ifndef HUFU_ABI_H_
#define HUFU_ABI_H_

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* 不透明客户端句柄（惰性连接 hufu-server，断线自动重连）。 */
typedef struct hufu_client hufu_client;

/*
 * 宿主回调表（C++ 薄壳实现；函数指针可为 NULL）。
 *
 * commit: 立即上屏文本（UTF-8，NUL 结尾）。
 * update: UI 状态快照——preedit（UTF-8，NUL 结尾）+ 候选数组（文本/注释，
 *         各 NUL 结尾）+ 候选数（0 时必须清除候选列表）+ 高亮索引（页内
 *         0 起）+ aux 提示 + 中英态（1=中）。
 *         回调期指针有效，宿主须同步拷走。
 */
typedef struct hufu_host {
    void *user;
    void (*commit)(void *user, const char *utf8);
    void (*update)(void *user, const char *preedit_utf8, const char *raw_utf8,
                   const char *const *candidate_texts,
                   const char *const *candidate_comments,
                   int32_t candidate_count, int32_t candidate_selected,
                   const char *aux_utf8, int32_t chinese);
} hufu_host;

/* hufu_client_key 返回值位掩码。 */
#define HUFU_KEY_CONSUMED 0x1
/* 回删数占 8..15 位（提交前需回删的已上屏字符数，如「1.」→「。」）。 */
#define HUFU_KEY_BACK_SHIFT 8
#define HUFU_KEY_BACK_MASK 0xff00

/* 创建客户端；sock_path 为 NULL/空串时用默认
 * （$XDG_RUNTIME_DIR/hufu-ime.sock，回退 /tmp/hufu-ime.sock）。
 * 引擎未运行时也返回非 NULL（首次按键失败即透传，之后自动重连）。 */
hufu_client *hufu_client_new(const char *sock_path, const hufu_host *host);
void hufu_client_free(hufu_client *client);

/* 一次按键：返回位掩码（HUFU_KEY_CONSUMED | back << HUFU_KEY_BACK_SHIFT）。
 * line_end：1=光标在行尾，0=不在，-1=未知。 */
int32_t hufu_client_key(hufu_client *client, const char *key, int32_t shift,
                        int32_t ctrl, int32_t alt, int32_t meta, int32_t caps,
                        int32_t line_end);

/* 清引擎会话并同步清 UI（activate/deactivate/reset 用）。 */
void hufu_client_reset(hufu_client *client);
/* 焦点切换：清会话与文章尾巴（保留中英态）。 */
void hufu_client_focus(hufu_client *client);

/* ping：1=引擎可达（排障用）。 */
int32_t hufu_client_ping(hufu_client *client);

/* 最近状态串（UTF-8，NUL 结尾；诊断用，随客户端存活）。 */
const char *hufu_client_status(const hufu_client *client);

/* 最近中英态：1=中（subMode 显示用）。 */
int32_t hufu_client_chinese(const hufu_client *client);

/* 最近一次按键的回删数（commit 回调内读取，先回删再上屏）。 */
int32_t hufu_client_last_back(const hufu_client *client);

#ifdef __cplusplus
}
#endif

#endif /* HUFU_ABI_H_ */
