// SPDX-FileCopyrightText: 2026 明雅流风 <crrvx@outlook.com>
// SPDX-License-Identifier: GPL-3.0-or-later

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
                   const char *const *candidate_commit_texts,
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

/* 鼠标点击候选：index 为页内下标（当前候选窗列表序号，0 起）；1=已处理。
 * 语义与数字选重一致（学习、无闪帧即时上屏）。 */
int32_t hufu_client_select(hufu_client *client, int32_t index);

/* ── 配置读写（fcitx5 设置页用；Windows 前端不使用）───────────────────── */

/* 拉取引擎配置快照（打开设置页时调用）：1=成功。 */
int32_t hufu_client_config_refresh(hufu_client *client);

/* 配置补丁：json 为 JSON 对象字符串（如 {"candidates":{"page_size":5}}），
 * 客户端侧「读-改-写」深合并后写回引擎（热生效）：1=成功。 */
int32_t hufu_client_config_patch(hufu_client *client, const char *json);

/* 配置读取 bool：1/0，-1=未知（未拉取或路径不存在）。path 形如
 * "candidates.show_split"。 */
int32_t hufu_client_config_bool(const hufu_client *client, const char *path);

/* 配置读取整数：1=成功（写 *out），0=未知/失败。 */
int32_t hufu_client_config_int(const hufu_client *client, const char *path,
                               int64_t *out);

/* 配置读取字符串：NUL 结尾（空串=未知；下次调用前有效）。 */
const char *hufu_client_config_str(hufu_client *client, const char *path);

/* ── 状态区菜单动作（fcitx5 托盘「虎符」子菜单用；Windows 前端不使用）────── */

/* 「重载码表」：当前方案原样重载（改码表/补充语料后免重启 server 生效）：
 * 1=成功，0=失败（引擎不在线、方案缺失等；宿主保持现状）。 */
int32_t hufu_client_reload_schema(hufu_client *client);

/* 「打开方案文件夹」：请引擎打开当前方案码表目录：
 * 1=成功，0=失败（引擎不在线、方案目录不存在）。 */
int32_t hufu_client_open_schema_dir(hufu_client *client);

/* 「按键音效」开关：引擎侧取反并落盘（热生效）：1=开，0=关，
 * -1=未知（引擎不在线或回包异常）——宿主据此保持原勾选态。 */
int32_t hufu_client_sound_toggle(hufu_client *client);

/* 「按键音效」当前态（勾选态显示用）：1=开，0=关，-1=未知（引擎不在线）。 */
int32_t hufu_client_sound_state(hufu_client *client);

/* ── 字反查（纯宿主侧：宿主取光标左侧汉字，本库按数据目录查拼音/虎码/拆分）────── */

/* 装载字反查索引：data_dir = 数据根目录
 * （${XDG_DATA_HOME:-$HOME/.local/share}/hufu，由宿主解析后传入；本库不读环境变量）。
 * 1=可用（拼音注释或码表至少一份读到数据），0=不可用（目录/文件缺失、参数非法）。
 * 可重复调用（按新目录重载，失败即清空索引）。 */
int32_t hufu_client_char_lookup_init(hufu_client *client, const char *data_dir);

/* 查一个字符的「拼音\t虎码[\t拆分]」三列（缺项为空列；整字无数据为空串）。
 * 返回 NUL 结尾指针，指向客户端内部缓冲——**下次对同一客户端调用本函数前有效**，
 * 宿主须同步拷走；未初始化时返回空串（client 为 NULL 时返回 NULL）。 */
const char *hufu_client_char_lookup(hufu_client *client, uint32_t ucs4);

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
