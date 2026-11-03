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
  { title: '中英切换', desc: 'Shift / Ctrl+空格 / Caps 行为 / Esc 撤回', route: 'input', group: '输入', icon: 'keyboard', keys: '中英 英文 中文 切换 shift caps ctrl 空格 esc 撤回 上屏 切换英文时保留已输入编码并上屏', sub: 'switch' },
  { title: '编码与上屏', desc: '编码字母表、最大码长、自动上屏、混输', route: 'input', group: '输入', icon: 'keyboard', keys: '编码 码长 上屏 混输 自动 字母表 顶屏 自动上屏', sub: 'input' },
  { title: '候选与标点', desc: '每页候选、翻页键、次选三选、全半角', route: 'input', group: '输入', icon: 'grid', keys: '候选 每页 翻页 次选 三选 标点 全角 半角 配对', sub: 'candidates' },
  { title: '音效与音量', desc: '按键 / 选词 / 上屏 / 翻页提示音与音量', route: 'input', group: '输入', icon: 'play', keys: '音效 声音 音量 提示音 按键 上屏 选词 翻页', sub: 'sound' },
  { title: '繁简与 Emoji', desc: 'OpenCC 简繁转换、Emoji 候选', route: 'input', group: '输入', icon: 'spark', keys: '简繁 繁体 简体 opencc emoji 表情 转换', sub: 'sound' },
  { title: '剪贴板上屏', desc: 'Ctrl+Shift+V 插入剪贴板文本', route: 'input', group: '输入', icon: 'download', keys: '剪贴板 粘贴 ctrl shift v 白名单 启用 Ctrl+Shift+V 剪贴板上屏', sub: 'advanced' },

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
  { title: '满码长唯一自动上屏', desc: '满码且唯一候选时自动上屏', route: 'input', group: '编码', icon: 'keyboard', keys: '满 满最大码长 满最大码长唯一自动上屏 码长 唯一 自动上屏 候选', sub: 'input' },
  { title: '空码自动清屏', desc: '满码 +1 键仍无解时清前码', route: 'input', group: '编码', icon: 'keyboard', keys: '空码 清屏 无解 前码 输入', sub: 'input' },
  { title: '回车清屏', desc: '回车清空当前编码', route: 'input', group: '编码', icon: 'keyboard', keys: '回车 enter 清屏 清空', sub: 'input' },
  { title: '中英混输', desc: '中文编码里保留英文字母大小写', route: 'input', group: '编码', icon: 'keyboard', keys: '中英 混输 中英混输（保留大小写） 英文 大小写 保留', sub: 'input' },
  { title: '中文态使用英文标点', desc: '中文态也输出英文标点', route: 'input', group: '编码', icon: 'keyboard', keys: '中文 英文 标点 全角 半角 符号', sub: 'input' },
  { title: '斜杠键顿号直出', desc: '斜杠输出顿号 / 命名空间', route: 'input', group: '编码', icon: 'keyboard', keys: '斜杠 顿号 直出 /=顿号直出 命名空间', sub: 'input' },
  { title: '反斜杠键顿号直出', desc: '反斜杠输出顿号 / 命名空间', route: 'input', group: '编码', icon: 'keyboard', keys: '反斜杠 顿号 直出 \=顿号直出 命名空间', sub: 'input' },
  { title: '分号引导', desc: '分号作为引导键进入符号/命令', route: 'input', group: '编码', icon: 'keyboard', keys: '分号 引导 快符 ;=引导快符 符号 命令', sub: 'input' },
  { title: '通配符', desc: '启用通配字符并设置具体字符', route: 'input', group: '编码', icon: 'keyboard', keys: '通配符 编码通配符 通配字符 通配 字符 wildcard 模糊', sub: 'input' },
  { title: '编码伪装', desc: '把编码显示成其他形式', route: 'input', group: '编码', icon: 'keyboard', keys: '编码 伪装 编码伪装前缀 前缀 显示 隐藏', sub: 'input' },

  // 输入 → 候选
  { title: '每页候选数', desc: '1–10，默认 4', route: 'input', group: '候选', icon: 'grid', keys: '每页 候选 数量 2 3 4 5 每页候选数', sub: 'candidates' },
  { title: '翻页键', desc: '逐字符生效，如 - = 或 , .', route: 'input', group: '候选', icon: 'grid', keys: '翻页 按键 上一页 下一页 - = , .', sub: 'candidates' },
  { title: '次选键 / 三选键', desc: '第二、第三候选的选择键', route: 'input', group: '候选', icon: 'grid', keys: '次选 三选 第二 第三 选重 候选 键', sub: 'candidates' },
  { title: '显示拼音注释', desc: '候选旁显示拼音/注音注释', route: 'input', group: '候选', icon: 'grid', keys: '显示 拼音 注音 注释 拆解 候选', sub: 'candidates' },
  { title: '显示 Unicode 注释', desc: '候选旁显示 Unicode 编码', route: 'input', group: '候选', icon: 'grid', keys: '显示 unicode 注释 编码 候选 拆解', sub: 'candidates' },
  { title: '显示拆分', desc: '候选旁显示拆解/拆分提示', route: 'input', group: '候选', icon: 'grid', keys: '显示 拆分 拆解 部件 候选 注音', sub: 'candidates' },
  { title: '同码异字高亮', desc: '同码且 ≥3 字的候选互标差异', route: 'input', group: '候选', icon: 'grid', keys: '同码 异字 高亮 差异 候选', sub: 'candidates' },
  { title: '候选延时', desc: '0=立即显示，上限 2000ms', route: 'input', group: '候选', icon: 'grid', keys: '候选 延时 候选延时（毫秒） 显示 延迟 毫秒', sub: 'candidates' },
  { title: '注释延时', desc: '延时展开拼音/拆分注释', route: 'input', group: '候选', icon: 'grid', keys: '注释 延时 注释延时（毫秒） 拼音 拆分 毫秒', sub: 'candidates' },
  { title: '全角标点', desc: '中文态标点使用全角形态', route: 'input', group: '候选', icon: 'grid', keys: '全角 半角 标点 标点与全半角 符号', sub: 'candidates' },
  { title: '成对引号自动配对', desc: '输入左引号/括号自动补右半', route: 'input', group: '候选', icon: 'grid', keys: '成对 引号 括号 自动 配对', sub: 'candidates' },

  // 输入 → 声音 / 高级
  { title: '按键音效', desc: '按键提示音开关', route: 'input', group: '声音', icon: 'play', keys: '按键 音效 启用按键音 声音 提示音', sub: 'sound' },
  { title: '选词音效', desc: '选词时播放提示音', route: 'input', group: '声音', icon: 'play', keys: '选词 音效 声音 提示音', sub: 'sound' },
  { title: '上屏音效', desc: '上屏时播放提示音', route: 'input', group: '声音', icon: 'play', keys: '上屏 音效 声音 提示音', sub: 'sound' },
  { title: '翻页音效', desc: '翻页时播放提示音', route: 'input', group: '声音', icon: 'play', keys: '翻页 音效 声音 提示音', sub: 'sound' },
  { title: '音量', desc: '0–100，统一控制音效音量', route: 'input', group: '声音', icon: 'play', keys: '音量 声音 音效 大小', sub: 'sound' },
  { title: 'OpenCC 简繁转换', desc: '简体 ↔ 繁体转换', route: 'input', group: '简繁', icon: 'spark', keys: '简繁 繁体 简体 opencc 转换 启用 OpenCC 转换', sub: 'sound' },
  { title: 'Emoji 候选', desc: '输入时展示 Emoji 候选', route: 'input', group: '简繁', icon: 'spark', keys: 'emoji 表情 候选 Emoji 注解候选 符号', sub: 'sound' },
  { title: '运行状态与键位映射', desc: '查看引擎/方案状态、已映射按键', route: 'input', group: '高级', icon: 'sliders', keys: '运行状态 引擎 方案 键位 映射 快捷键', sub: 'advanced' },

  // 输入 → 组合键 / 细节（补全搜索盲区 2026-11-03）
  { title: '调频按住键', desc: '按住组合键对当前候选调频，可录制/停用', route: 'input', group: '高级', icon: 'sliders', keys: '调频 调频键 按住 键 频率 候选调频 组合键 录制 停用 freq freq_key', sub: 'advanced' },
  { title: '删词按住键', desc: '按住组合键删除候选/用户词，可录制/停用', route: 'input', group: '高级', icon: 'sliders', keys: '删词 删词键 删除 按住 键 组合键 录制 停用 delete delete_key', sub: 'advanced' },
  { title: '导出码表快捷键', desc: '按住组合键快速导出当前方案码表', route: 'input', group: '高级', icon: 'sliders', keys: '导出 导出码表 码表 快捷键 按住 键 组合键 录制 export export_key', sub: 'advanced' },
  { title: '组合键录制', desc: '调频/删词/导出的按住组合键如何录制', route: 'input', group: '高级', icon: 'keyboard', keys: '组合键 录制 按住 调频 删词 导出 快捷键 单键 ctrl shift alt tab 停用', sub: 'advanced' },
  { title: '进程白名单', desc: 'Ctrl+Shift+V 剪贴板上屏的进程白名单', route: 'input', group: '高级', icon: 'download', keys: '进程 白名单 剪贴板 粘贴 ctrl shift v 程序 exe', sub: 'advanced' },
  { title: '音效包', desc: '替换音效包目录中的整组提示音', route: 'input', group: '声音', icon: 'play', keys: '音效包 音效 声音 目录 替换 提示音', sub: 'sound' },
  { title: '转换方向', desc: 'OpenCC 简→繁或繁→简', route: 'input', group: '简繁', icon: 'spark', keys: '转换 方向 简繁 繁体 简体 简转繁 繁转简 opencc', sub: 'sound' },
  { title: '拆分方案', desc: '候选拆解使用的拆分方案', route: 'input', group: '候选', icon: 'grid', keys: '拆分 方案 拆解 部件 候选 注释', sub: 'candidates' },
  { title: '选重键', desc: '次选/三选键，按选重位选择候选', route: 'input', group: '候选', icon: 'grid', keys: '选重 选重键 次选 三选 第二 第三 候选 键', sub: 'candidates' },

  // 皮肤 → 工作台细节
  { title: '候选窗外观', desc: '字体、字号、圆角、透明度、阴影、间距、布局', route: 'skin', group: '外观', icon: 'palette', keys: '候选窗 外观 候选字号 候选字体 候选高度 候选间距 高亮区比例 首选字号比例 其余字号比例 边框厚度 边框 前导序号 字重 整体透明度 高亮透明度 阴影 布局 竖排 皮肤' },
  { title: '候选窗动效', desc: '动效总开关、动效速度、选重暂留、出现/收起', route: 'motion', group: '外观', icon: 'play', keys: '候选窗 动效 动画 速度 选重 暂留 高亮 开关 节奏 收起 出现' },
  { title: '高亮跟随放大', desc: '字号强调跟随当前选中项；高亮候选放大、其余缩小', route: 'motion', group: '外观', icon: 'spark', keys: '高亮 跟随 放大 缩小 字号 强调 选中 高亮区放大 首选放大 其余缩小 逐候选' },
  { title: '候选宽度与序号', desc: '候选窗总宽、序号开关、序号显示种类（数字/中文/罗马）', route: 'skin', group: '外观', icon: 'palette', keys: '候选宽度 宽度 序号 开关 显示 数字 中文 罗马 标签 label_style 自适应' },
  { title: '皮肤恢复出厂与列表', desc: '搜索皮肤、恢复出厂、删除 .ssf', route: 'skin', group: '外观', icon: 'palette', keys: '皮肤 列表 搜索 恢复 出厂 删除 ssf 当前 导入' },

  // 方案 / 数据 / 挂件 / 整句补充
  { title: '导出码表与用户调整', desc: '把置顶/隐藏/加词/调频合并进原始码表导出', route: 'schemes', group: '引擎', icon: 'grid', keys: '导出 导出码表 码表 用户调整 置顶 隐藏 加词 调频 合并 反查 拆分' },
  { title: '用户词库与用户调整', desc: '用户词库、用户调整、码表目录', route: 'data', group: '数据', icon: 'db', keys: '用户词库 用户调整 加词 调频 码表 目录 数据 备份' },
  { title: '挂件尺寸与贴法', desc: '挂件高度、贴边、透明度、延时、动图', route: 'buddy', group: '外观', icon: 'image', keys: '挂件 尺寸 高度 大小 贴边 透明度 延时 动图 候选' },
  { title: '整句延迟与超时', desc: '整句延迟、提前上屏阈值、重排超时', route: 'sentence', group: '引擎', icon: 'spark', keys: '整句 延迟 超时 top_k 提前 上屏 阈值 速度 模型' },
  { title: '反查 / 删词 / 导出组合键', desc: '旧页录制式组合键；键位编辑器里只读展示', route: 'keymap', group: '键位', icon: 'keyboard', keys: '反查 删词 导出 组合键 快捷键 只读 键位 录制' },

  // 键位 / 外观 / 引擎
  { title: '键位编辑', desc: '主键盘、小键盘、Shift+符号自定义', route: 'keymap', group: '键位', icon: 'keyboard', keys: '键位 映射 快捷键 主键盘 小键盘 键盘分页 shift 符号 自定义 录制' },
  { title: '皮肤颜色与圆角', desc: '窗底色、候选色、圆角、字号', route: 'skin', group: '外观', icon: 'palette', keys: '皮肤 颜色 圆角 字号 候选窗 外观 主题 深色 浅色' },
  { title: '导入 .ssf / 新建皮肤', desc: '导入搜狗/虎娘皮肤，或克隆当前皮肤', route: 'skin', group: '外观', icon: 'palette', keys: '皮肤 导入 ssf 新建 克隆 外观 主题' },
  { title: '候选挂件', desc: '挂件图片、贴法、透明度、延时', route: 'buddy', group: '外观', icon: 'image', keys: '挂件 图片 贴边 透明度 延时 候选 动图 启用挂件 挂件不透明度 与候选框间距 延时消失秒数 抠图容差 边缘羽化 运行时水平翻转' },
  { title: '整句开关与提前上屏', desc: '整句开关、自动启用、提前上屏阈值', route: 'sentence', group: '引擎', icon: 'spark', keys: '整句 开关 整句输入总开关 自动 提前 上屏 阈值 组句 提前上屏稳定键数 稳定 N 键' },
  { title: '神经重排', desc: 'Qwen 小模型重排、top_k、超时', route: 'sentence', group: '引擎', icon: 'spark', keys: '神经 重排 qwen 模型 top_k 重排候选数 超时 排序 停顿重排 停顿判定毫秒' },
  { title: '整句模型路径', desc: '5-gram 模型与重排模型路径', route: 'sentence', group: '引擎', icon: 'folder', keys: '整句 模型 路径 文件 bin gguf 模型目录' },
  { title: '整句权重与预设', desc: '12 项权重、稳健/均衡/灵敏预设', route: 'sentence', group: '引擎', icon: 'sliders', keys: '整句 权重 预设 稳健 均衡 灵敏 参数 恢复默认 恢复出厂' },
  { title: '方案管理', desc: '方案切换、码表导出、反查拆分', route: 'schemes', group: '引擎', icon: 'grid', keys: '方案 码表 切换 导出 反查 拆分 当前 筛选方案 反查方案 反查引导前缀 关闭反查时按引导键' },

  // 同步 / 数据 / 关于
  { title: '云同步与 WebDAV', desc: 'WebDAV、设备、方案同步、云端文件', route: 'sync', group: '同步', icon: 'cloud', keys: '同步 云 webdav 设备 云端 备份 坚果云 应用密码' },
  { title: '云同步教程', desc: '应用密码、测试连接、第二台设备', route: 'sync', group: '同步', icon: 'help', keys: '同步 教程 应用密码 坚果云 客户端 网页 测试 连接 卸载' },
  { title: '同步方案勾选', desc: '选择要同步的方案与自动同步', route: 'sync', group: '同步', icon: 'cloud', keys: '同步 方案 勾选 自动同步 云端 文件 设备列表 本地自动备份' },
  { title: '数据导出 / 导入', desc: '导出 JSON、导入恢复全部配置', route: 'data', group: '数据', icon: 'db', keys: '数据 导出 导入 json 备份 恢复 配置 数据与备份 选择虎符设置文件' },
  { title: '数据 / 安装 / 码表目录', desc: '打开本机数据、安装、码表目录', route: 'data', group: '数据', icon: 'folder', keys: '数据 目录 安装 码表 打开 路径 文件夹' },
  { title: '关于与更新', desc: '版本、安装目录、开源仓库', route: 'about', group: '关于', icon: 'info', keys: '关于 版本 更新 github 仓库 目录 升级' },
  { title: '打赏支持', desc: '扫码打赏，支持作者继续开发', route: 'donate', group: '关于', icon: 'heart', keys: '打赏 支持 捐赠 赞赏 二维码 咖啡' },
];
