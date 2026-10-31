import type { Route } from '../types';

export interface SearchItem {
  title: string;
  desc: string;
  route: Route;
  group: string;
  icon: string;
  keys: string;
  sub?: string;
}

export const SEARCH_ITEMS: SearchItem[] = [
  // 常用入口
  { title: '中英切换', desc: 'Shift / Ctrl+空格 / Caps 行为 / Esc 撤回', route: 'input', group: '输入', icon: 'keyboard', keys: '中英 英文 中文 切换 shift caps ctrl 空格 esc 撤回 上屏', sub: 'switch' },
  { title: '编码与上屏', desc: '编码字母表、最大码长、自动上屏、混输', route: 'input', group: '输入', icon: 'keyboard', keys: '编码 码长 上屏 混输 自动 字母表 顶屏 自动上屏', sub: 'input' },
  { title: '候选与标点', desc: '每页候选、翻页键、次选三选、全半角', route: 'input', group: '输入', icon: 'grid', keys: '候选 每页 翻页 次选 三选 标点 全角 半角 配对', sub: 'candidates' },
  { title: '音效与音量', desc: '按键 / 选词 / 上屏 / 翻页提示音与音量', route: 'input', group: '输入', icon: 'play', keys: '音效 声音 音量 提示音 按键 上屏 选词 翻页', sub: 'sound' },
  { title: '繁简与 Emoji', desc: 'OpenCC 简繁转换、Emoji 候选', route: 'input', group: '输入', icon: 'spark', keys: '简繁 繁体 简体 opencc emoji 表情 转换', sub: 'sound' },
  { title: '剪贴板上屏', desc: 'Ctrl+Shift+V 插入剪贴板文本', route: 'input', group: '输入', icon: 'download', keys: '剪贴板 粘贴 ctrl shift v 白名单', sub: 'advanced' },

  // 输入 → 切换
  { title: 'Shift 切换中英', desc: '单击 Shift 在中/英之间切换', route: 'input', group: '切换', icon: 'keyboard', keys: 'shift 中英 切换 左右 shift 映射', sub: 'switch' },
  { title: 'Ctrl+空格 切换中英', desc: '用 Ctrl+空格 切换中英文', route: 'input', group: '切换', icon: 'keyboard', keys: 'ctrl 空格 中英 切换 快捷键', sub: 'switch' },
  { title: 'Caps 行为', desc: '锁定大写 / 清屏 / 切英文 / 无操作', route: 'input', group: '切换', icon: 'keyboard', keys: 'caps 大写 锁定 清屏 切英文 无操作', sub: 'switch' },
  { title: 'Ctrl+M 最近方案', desc: '在当前方案与上一方案间切换', route: 'input', group: '切换', icon: 'grid', keys: 'ctrl m 最近 方案 切换 上一次', sub: 'switch' },
  { title: 'Esc 撤回上次上屏', desc: '空态 + 焦点有光标时撤回', route: 'input', group: '切换', icon: 'keyboard', keys: 'esc 撤回 撤销 上屏 回退', sub: 'switch' },
  { title: '重复上屏包括符号', desc: '控制重复上屏是否包含标点符号', route: 'input', group: '切换', icon: 'keyboard', keys: '重复 上屏 符号 标点 历史', sub: 'switch' },
  { title: '切换英文保留编码', desc: 'Shift/Caps/Ctrl+空格 同一口径', route: 'input', group: '切换', icon: 'keyboard', keys: '切换 英文 保留 编码 上屏 语言栏', sub: 'switch' },

  // 输入 → 编码
  { title: '编码字母表', desc: '码元集合；字母表内字符不可作通配符', route: 'input', group: '编码', icon: 'keyboard', keys: '编码 字母表 码元 26 27 字符 通配', sub: 'input' },
  { title: '最大码长', desc: '1–10；超出后按下一项决定顶屏', route: 'input', group: '编码', icon: 'keyboard', keys: '最大 码长 4 顶屏 长度', sub: 'input' },
  { title: 'Tab 键行为', desc: '清屏 或 选重导航', route: 'input', group: '编码', icon: 'keyboard', keys: 'tab 键 清屏 选重 导航 下一候选 空格上屏', sub: 'input' },
  { title: '超最大码长自动上屏', desc: '码长超过上限时自动上屏', route: 'input', group: '编码', icon: 'keyboard', keys: '超 最大码长 自动上屏 顶屏', sub: 'input' },
  { title: '满码长唯一自动上屏', desc: '满码且唯一候选时自动上屏', route: 'input', group: '编码', icon: 'keyboard', keys: '满 码长 唯一 自动上屏 候选', sub: 'input' },
  { title: '空码自动清屏', desc: '满码 +1 键仍无解时清前码', route: 'input', group: '编码', icon: 'keyboard', keys: '空码 清屏 无解 前码 输入', sub: 'input' },
  { title: '回车清屏', desc: '回车清空当前编码', route: 'input', group: '编码', icon: 'keyboard', keys: '回车 enter 清屏 清空', sub: 'input' },
  { title: '中英混输', desc: '中文编码里保留英文字母大小写', route: 'input', group: '编码', icon: 'keyboard', keys: '中英 混输 英文 大小写 保留', sub: 'input' },
  { title: '中文态使用英文标点', desc: '中文态也输出英文标点', route: 'input', group: '编码', icon: 'keyboard', keys: '中文 英文 标点 全角 半角 符号', sub: 'input' },
  { title: '斜杠键顿号直出', desc: '斜杠输出顿号 / 命名空间', route: 'input', group: '编码', icon: 'keyboard', keys: '斜杠 顿号 直出 命名空间', sub: 'input' },
  { title: '反斜杠键顿号直出', desc: '反斜杠输出顿号 / 命名空间', route: 'input', group: '编码', icon: 'keyboard', keys: '反斜杠 顿号 直出 命名空间', sub: 'input' },
  { title: '分号引导', desc: '分号作为引导键进入符号/命令', route: 'input', group: '编码', icon: 'keyboard', keys: '分号 引导 符号 命令', sub: 'input' },
  { title: '通配符', desc: '启用通配字符并设置具体字符', route: 'input', group: '编码', icon: 'keyboard', keys: '通配符 通配 字符 wildcard 模糊', sub: 'input' },
  { title: '编码伪装', desc: '把编码显示成其他形式', route: 'input', group: '编码', icon: 'keyboard', keys: '编码 伪装 显示 隐藏', sub: 'input' },

  // 输入 → 候选
  { title: '每页候选数', desc: '1–10，默认 4', route: 'input', group: '候选', icon: 'grid', keys: '每页 候选 数量 2 3 4 5', sub: 'candidates' },
  { title: '翻页键', desc: '逐字符生效，如 - = 或 , .', route: 'input', group: '候选', icon: 'grid', keys: '翻页 按键 上一页 下一页 - = , .', sub: 'candidates' },
  { title: '次选键 / 三选键', desc: '第二、第三候选的选择键', route: 'input', group: '候选', icon: 'grid', keys: '次选 三选 第二 第三 候选 键', sub: 'candidates' },
  { title: '显示拼音注释', desc: '候选旁显示拼音/注音注释', route: 'input', group: '候选', icon: 'grid', keys: '显示 拼音 注音 注释 拆解 候选', sub: 'candidates' },
  { title: '显示 Unicode 注释', desc: '候选旁显示 Unicode 编码', route: 'input', group: '候选', icon: 'grid', keys: '显示 unicode 注释 编码 候选 拆解', sub: 'candidates' },
  { title: '显示拆分', desc: '候选旁显示拆解/拆分提示', route: 'input', group: '候选', icon: 'grid', keys: '显示 拆分 拆解 部件 候选 注音', sub: 'candidates' },
  { title: '同码异字高亮', desc: '同码且 ≥3 字的候选互标差异', route: 'input', group: '候选', icon: 'grid', keys: '同码 异字 高亮 差异 候选', sub: 'candidates' },
  { title: '候选延时', desc: '0=立即显示，上限 2000ms', route: 'input', group: '候选', icon: 'grid', keys: '候选 延时 显示 延迟 毫秒', sub: 'candidates' },
  { title: '注释延时', desc: '延时展开拼音/拆分注释', route: 'input', group: '候选', icon: 'grid', keys: '注释 延时 拼音 拆分 毫秒', sub: 'candidates' },
  { title: '全角标点', desc: '中文态标点使用全角形态', route: 'input', group: '候选', icon: 'grid', keys: '全角 半角 标点 符号', sub: 'candidates' },
  { title: '成对引号自动配对', desc: '输入左引号/括号自动补右半', route: 'input', group: '候选', icon: 'grid', keys: '成对 引号 括号 自动 配对', sub: 'candidates' },

  // 输入 → 声音 / 高级
  { title: '按键音效', desc: '按键提示音开关', route: 'input', group: '声音', icon: 'play', keys: '按键 音效 声音 提示音', sub: 'sound' },
  { title: '选词音效', desc: '选词时播放提示音', route: 'input', group: '声音', icon: 'play', keys: '选词 音效 声音 提示音', sub: 'sound' },
  { title: '上屏音效', desc: '上屏时播放提示音', route: 'input', group: '声音', icon: 'play', keys: '上屏 音效 声音 提示音', sub: 'sound' },
  { title: '翻页音效', desc: '翻页时播放提示音', route: 'input', group: '声音', icon: 'play', keys: '翻页 音效 声音 提示音', sub: 'sound' },
  { title: '音量', desc: '0–100，统一控制音效音量', route: 'input', group: '声音', icon: 'play', keys: '音量 声音 音效 大小', sub: 'sound' },
  { title: 'OpenCC 简繁转换', desc: '简体 ↔ 繁体转换', route: 'input', group: '简繁', icon: 'spark', keys: '简繁 繁体 简体 opencc 转换', sub: 'sound' },
  { title: 'Emoji 候选', desc: '输入时展示 Emoji 候选', route: 'input', group: '简繁', icon: 'spark', keys: 'emoji 表情 候选 符号', sub: 'sound' },
  { title: '运行状态与键位映射', desc: '查看引擎/方案状态、已映射按键', route: 'input', group: '高级', icon: 'sliders', keys: '运行状态 引擎 方案 键位 映射 快捷键', sub: 'advanced' },

  // 键位 / 外观 / 引擎
  { title: '键位编辑', desc: '主键盘、小键盘、Shift+符号自定义', route: 'keymap', group: '键位', icon: 'keyboard', keys: '键位 映射 快捷键 主键盘 小键盘 shift 符号 自定义 录制' },
  { title: '皮肤颜色与圆角', desc: '窗底色、候选色、圆角、字号', route: 'skin', group: '外观', icon: 'palette', keys: '皮肤 颜色 圆角 字号 候选窗 外观 主题 深色 浅色' },
  { title: '导入 .ssf / 新建皮肤', desc: '导入搜狗/虎娘皮肤，或克隆当前皮肤', route: 'skin', group: '外观', icon: 'palette', keys: '皮肤 导入 ssf 新建 克隆 外观 主题' },
  { title: '候选挂件', desc: '挂件图片、贴法、透明度、延时', route: 'buddy', group: '外观', icon: 'image', keys: '挂件 图片 贴边 透明度 延时 候选 动图' },
  { title: '整句开关与提前上屏', desc: '整句开关、自动启用、提前上屏阈值', route: 'sentence', group: '引擎', icon: 'spark', keys: '整句 开关 自动 提前 上屏 阈值 组句' },
  { title: '神经重排', desc: 'Qwen 小模型重排、top_k、超时', route: 'sentence', group: '引擎', icon: 'spark', keys: '神经 重排 qwen 模型 top_k 超时 排序' },
  { title: '整句模型路径', desc: '5-gram 模型与重排模型路径', route: 'sentence', group: '引擎', icon: 'folder', keys: '整句 模型 路径 文件 bin gguf 模型目录' },
  { title: '整句权重与预设', desc: '12 项权重、稳健/均衡/灵敏预设', route: 'sentence', group: '引擎', icon: 'sliders', keys: '整句 权重 预设 稳健 均衡 灵敏 参数' },
  { title: '方案管理', desc: '方案切换、码表导出、反查拆分', route: 'schemes', group: '引擎', icon: 'grid', keys: '方案 码表 切换 导出 反查 拆分 当前' },

  // 同步 / 数据 / 关于
  { title: '云同步与 WebDAV', desc: 'WebDAV、设备、方案同步、云端文件', route: 'sync', group: '同步', icon: 'cloud', keys: '同步 云 webdav 设备 云端 备份 坚果云 应用密码' },
  { title: '云同步教程', desc: '应用密码、测试连接、第二台设备', route: 'sync', group: '同步', icon: 'help', keys: '同步 教程 应用密码 坚果云 客户端 网页 测试 连接 卸载' },
  { title: '同步方案勾选', desc: '选择要同步的方案与自动同步', route: 'sync', group: '同步', icon: 'cloud', keys: '同步 方案 勾选 自动同步 云端 文件' },
  { title: '数据导出 / 导入', desc: '导出 JSON、导入恢复全部配置', route: 'data', group: '数据', icon: 'db', keys: '数据 导出 导入 json 备份 恢复 配置' },
  { title: '数据 / 安装 / 码表目录', desc: '打开本机数据、安装、码表目录', route: 'data', group: '数据', icon: 'folder', keys: '数据 目录 安装 码表 打开 路径 文件夹' },
  { title: '关于与更新', desc: '版本、安装目录、开源仓库', route: 'about', group: '关于', icon: 'info', keys: '关于 版本 更新 github 仓库 目录 升级' },
  { title: '打赏支持', desc: '扫码打赏，支持作者继续开发', route: 'donate', group: '关于', icon: 'heart', keys: '打赏 支持 捐赠 赞赏 二维码 咖啡' },
];
