export type Route =
  | 'overview'
  | 'schemes'
  | 'input'
  | 'keymap'
  | 'skin'
  | 'motion'
  | 'buddy'
  | 'sentence'
  | 'data'
  | 'sync'
  | 'about'
  | 'donate';

export type ThemeMode = 'system' | 'dark' | 'light';

export interface About {
  data_dir: string;
  exe_dir: string;
  github: string;
  version: string;
}

export interface EngineState {
  current_schema: string;
  model_present: boolean;
  schemas: string[];
  sentence_active: boolean;
  state: {
    chinese?: boolean;
    full_shape?: boolean;
    caps_lock?: boolean;
    mode?: string;
    [key: string]: unknown;
  };
}

export interface Config {
  appearance?: {
    anim?: boolean;
    anim_flash?: boolean;
    anim_speed?: number;
    font_size?: number;
    per_cand_font?: boolean;
    first_font_scale?: number;
    other_font_scale?: number;
    /** 【2026-11-03】高亮跟随放大：逐候选字号跟随当前选中项（默认 false=旧行为）。 */
    hl_font_scale?: boolean;
    skin?: string;
    skin_kind?: string;
    status_capsule?: boolean;
  };
  candidates?: Record<string, unknown>;
  general?: Record<string, unknown>;
  [key: string]: unknown;
}

export interface SkinMeta {
  id: string;
  name: string;
  kind: 'json' | 'ssf';
  preview?: boolean;
  author?: string;
  /** 列表/概览共用的皮肤代表色（服务端 /api/skins 返回；SSF 无此字段）。 */
  swatch?: string;
  accent?: string;
}

export interface SkinList {
  current: string;
  current_kind: string;
  skins: SkinMeta[];
}

export interface SkinColors {
  [key: string]: string;
}

export interface SkinLayout {
  font_point?: number;
  font_face?: string;
  corner_radius?: number;
  candidate_spacing?: number;
  line_spacing?: number;
  margin_x?: number;
  margin_y?: number;
  hilite_padding?: number;
  hilite_spacing?: number;
  hilited_corner_radius?: number;
  horizontal?: boolean;
  /** 编码内联到应用；旧版皮肤契约，缺省 true（非 config.json 全局字段）。 */
  inline_preedit?: boolean;
  border_width?: number;
  label_style?: string;
  label_format?: string;
  label_font_point?: number;
  max_height?: number;
  min_width?: number;
  width?: number;
  shadow_radius?: number;
  shadow_offset_x?: number;
  shadow_offset_y?: number;
  [key: string]: unknown;
}

export interface SkinMaterial {
  kind?: string;
  master_alpha?: number;
  hilite_alpha?: number;
  shadow_alpha?: number;
  border_alpha?: number;
  border_width?: number;
  border_color?: string;
  hilite_on?: boolean;
  [key: string]: unknown;
}

export interface Skin {
  id: string;
  name: string;
  author?: string;
  dark?: boolean;
  colors: SkinColors;
  layout: SkinLayout;
  material: SkinMaterial;
  overlay?: Record<string, unknown>;
  [key: string]: unknown;
}

export interface SyncProgress {
  active: boolean;
  cur: number;
  cur_name: string;
  down_bytes: number;
  down_files: number;
  total: number;
  up_bytes: number;
  up_files: number;
}

export interface SyncStatus {
  configured: boolean;
  device: string;
  enabled: boolean;
  last_error: string | null;
  last_sync: number;
  phase: string;
  progress: SyncProgress;
  selected: string[];
  stopped: boolean;
  url: string;
  user: string;
}

export interface SyncScheme {
  cloud: boolean;
  files: number;
  local: boolean;
  name: string;
  selected: boolean;
}

export interface SyncSchemes {
  ok: boolean;
  schemes: SyncScheme[];
}

export interface Bootstrap {
  about: About;
  state: EngineState;
  config: Config;
  skins: SkinList;
  skin: Skin | null;
  sync: SyncStatus;
  syncSchemes: SyncSchemes;
  online: boolean;
  errors: string[];
}

export const fallbackSkin: Skin = {
  id: 'hufu-moyan',
  name: '墨言',
  author: 'HuFu',
  dark: true,
  colors: {
    back_color: '#1B1E24F2',
    border_color: '#3B4049FF',
    candidate_text_color: '#ECE9E2FF',
    comment_text_color: '#8D8980FF',
    label_color: '#8D8980FF',
    text_color: '#E7B75CFF',
    hilited_text_color: '#E7B75CFF',
    hilited_candidate_back_color: '#E7B75CFF',
    hilited_candidate_text_color: '#1A1408FF',
    hilited_candidate_label_color: '#1A1408FF',
    hilited_label_color: '#1A1408FF',
    shadow_color: '#000000FF',
    preedit_back_color: '#00000000',
    candidate_back_color: '#00000000',
  },
  layout: {
    font_point: 17,
    corner_radius: 10,
    candidate_spacing: 8,
    line_spacing: 4,
    margin_x: 10,
    margin_y: 8,
    hilite_padding: 6,
    hilite_spacing: 2,
    hilited_corner_radius: 8,
    border_width: 1,
    label_style: 'digit',
    label_format: '%s.',
  },
  material: {
    kind: 'solid',
    master_alpha: 0.94,
    hilite_alpha: 1,
    shadow_alpha: 0.55,
    border_width: 1,
    border_color: '#3B4049',
    hilite_on: true,
  },
};

export const fallbackBootstrap: Bootstrap = {
  about: {
    /* 【离线版本号 2026-11-02】以前写死 '1.9.6'，实机已到 1.9.8：离线或
       /api/about 请求失败时会显示过期版本号。构建期没有版本注入通道
       （vite.config.ts 无 define、package.json 0.1.0 与发行版本无关），
       最小改动是显示占位符「—」；在线时 /api/about 读 version.txt 的真值
       会覆盖此处，不会长期停在占位符上。 */
    version: '—',
    github: 'https://github.com/LeafHW/hufu-ime-rust',
    data_dir: 'D:\\HuFu\\数据',
    exe_dir: 'D:\\HuFu',
  },
  state: {
    current_schema: '虎整句',
    model_present: true,
    schemas: ['个人自用', '五笔', '夜莺', '虎整句', '虎码单字', '虎码字词'],
    sentence_active: true,
    state: { chinese: true, full_shape: true, mode: 'Normal' },
  },
  config: {
    appearance: {
      anim: true,
      anim_speed: 1,
      font_size: 17.6,
      per_cand_font: true,
      first_font_scale: 1,
      other_font_scale: 0.7,
      hl_font_scale: false,
      skin: 'hufu-moyan',
      skin_kind: 'json',
    },
  },
  skins: {
    current: 'hufu-moyan',
    current_kind: 'json',
    skins: [
      { id: 'hufu-moyan', name: '墨言', kind: 'json' },
      { id: 'hufu-hupo', name: '琥珀', kind: 'json' },
      { id: 'hufu-yuebai', name: '月白', kind: 'json' },
      { id: '虎娘-雨竹听风', name: '雨竹听风', kind: 'ssf' },
    ],
  },
  skin: fallbackSkin,
  sync: {
    /* SY-03：离线兜底不再编造账号/设备名/“已同步”状态——全部置空，
       Sync 页据 data.online 显示离线提示并禁用表单。 */
    configured: false,
    device: '',
    enabled: false,
    last_error: null,
    last_sync: 0,
    phase: 'idle',
    progress: { active: false, cur: 0, cur_name: '', down_bytes: 0, down_files: 0, total: 0, up_bytes: 0, up_files: 0 },
    selected: [],
    stopped: false,
    url: '',
    user: '',
  },
  syncSchemes: {
    ok: true,
    schemes: [
      { cloud: true, files: 5, local: true, name: '个人自用', selected: true },
      { cloud: false, files: 2, local: true, name: '五笔', selected: false },
      { cloud: false, files: 6, local: true, name: '夜莺', selected: false },
      { cloud: true, files: 6, local: true, name: '虎整句', selected: true },
    ],
  },
  online: false,
  errors: [],
};
