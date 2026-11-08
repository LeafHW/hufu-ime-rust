# -*- coding: utf-8 -*-
"""生成《候选窗皮肤梳理》对照页。
读取真机皮肤目录与 docs/design 里的新方向方案，产出单文件 HTML。
用法：python gen-skin-review.py
"""
import json, glob, os, sys, io

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, '..', '..'))
LIVE = r'D:\HuFu\数据\皮肤'

def load_live():
    skins = []
    for p in sorted(glob.glob(os.path.join(LIVE, '*.json'))):
        try:
            with open(p, encoding='utf-8') as f:
                skins.append(json.load(f))
        except Exception as e:
            print('跳过', p, e, file=sys.stderr)
    return skins

# ── 颜色工具 ────────────────────────────────────────────────
def parse(h):
    h = (h or '').lstrip('#')
    if len(h) not in (6, 8):
        return (20, 22, 26, 255)
    r, g, b = int(h[0:2], 16), int(h[2:4], 16), int(h[4:6], 16)
    a = int(h[6:8], 16) if len(h) == 8 else 255
    return (r, g, b, a)

def over(fg, bg):
    """把 fg 按自身 alpha 叠到不透明 bg 上，返回 6 位 hex。"""
    r, g, b, a = parse(fg)
    br, bg_, bb, _ = parse(bg)
    k = a / 255.0
    return '#%02X%02X%02X' % (
        round(r * k + br * (1 - k)),
        round(g * k + bg_ * (1 - k)),
        round(b * k + bb * (1 - k)))

def lum(h):
    h = (h or '#000000').lstrip('#')[:6]
    r, g, b = [int(h[i:i + 2], 16) / 255 for i in (0, 2, 4)]
    f = lambda c: c / 12.92 if c <= 0.03928 else ((c + 0.055) / 1.055) ** 2.4
    return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b)

def contrast(a, b):
    l1, l2 = lum(a), lum(b)
    if l1 < l2:
        l1, l2 = l2, l1
    return round((l1 + 0.05) / (l2 + 0.05), 2)

# ── 新方向：全部从设置窗 token 派生 ──────────────────────────
NEW = [
  {
    "id": "proposal-jinmo", "name": "错金 · 墨", "family": "错金",
    "note": "继承设置窗暗色 token：墨底 #131417，金 #e7b75c 做选中块，暖白 #f0ede6 做正文。",
    "dark": True,
    "colors": {
      "back_color": "#1A1D22FF", "border_color": "#2A2E36FF", "text_color": "#E7B75CFF",
      "candidate_text_color": "#F0EDE6FF", "candidate_back_color": "#1A1D2200",
      "comment_text_color": "#9D978BFF", "label_color": "#9D978BFF",
      "hilited_candidate_text_color": "#1A1408FF", "hilited_candidate_back_color": "#E7B75CFF",
      "hilited_candidate_label_color": "#6B4C08FF", "hilited_comment_text_color": "#6B4C08FF",
      "hilited_label_color": "#6B4C08FF", "hilited_mark_color": "#1A1408FF",
      "shadow_color": "#00000066", "hilited_text_color": "#FFFFFFFF",
      "capsule_text_color": "#F0EDE6FF", "capsule_back_color": "#20242AFF"
    },
    "layout": {"horizontal": False, "inline_preedit": True, "font_face": "霞鹜文楷 GB 屏幕阅读版",
      "font_point": 16.0, "label_font_point": 12.0, "label_format": "%s.", "corner_radius": 9.0,
      "hilited_corner_radius": 6.0, "border_width": 1.0, "margin_x": 8.0, "margin_y": 6.0,
      "spacing": 6.0, "candidate_spacing": 4.0, "hilite_spacing": 2.0, "hilite_padding": 6.0,
      "line_spacing": 4.0, "min_width": 150.0, "width": 0.0, "shadow_radius": 14.0,
      "shadow_offset_x": 0.0, "shadow_offset_y": 2.0, "mark_text": "", "label_style": "digit"},
    "material": {"kind": "solid", "tint": "#1A1D2200", "opacity": 1.0, "master_alpha": 1.0,
      "hilite_alpha": 1.0, "hilite_on": True, "border_alpha": 1.0}
  },
  {
    "id": "proposal-jinzhi", "name": "错金 · 纸", "family": "错金",
    "note": "亮色对应款：纸底 #FFFDF8，青铜 #8A5E12 做实心选中块，深墨字。",
    "dark": False,
    "colors": {
      "back_color": "#FFFDF8FF", "border_color": "#E2DCCEFF", "text_color": "#A8741AFF",
      "candidate_text_color": "#211E18FF", "candidate_back_color": "#FFFDF800",
      "comment_text_color": "#6B6458FF", "label_color": "#A0988AFF",
      "hilited_candidate_text_color": "#FDF6E7FF", "hilited_candidate_back_color": "#8A5E12FF",
      "hilited_candidate_label_color": "#F0DFBEFF", "hilited_comment_text_color": "#EBDCBCFF",
      "hilited_label_color": "#F0DFBEFF", "hilited_mark_color": "#FFFDF8FF",
      "shadow_color": "#3C2D1422", "hilited_text_color": "#8A5E12FF",
      "capsule_text_color": "#211E18FF", "capsule_back_color": "#F3EFE5FF"
    },
    "layout": {"horizontal": False, "inline_preedit": True, "font_face": "霞鹜文楷 GB 屏幕阅读版",
      "font_point": 16.0, "label_font_point": 12.0, "label_format": "%s.", "corner_radius": 9.0,
      "hilited_corner_radius": 6.0, "border_width": 1.0, "margin_x": 8.0, "margin_y": 6.0,
      "spacing": 6.0, "candidate_spacing": 4.0, "hilite_spacing": 2.0, "hilite_padding": 6.0,
      "line_spacing": 4.0, "min_width": 150.0, "width": 0.0, "shadow_radius": 14.0,
      "shadow_offset_x": 0.0, "shadow_offset_y": 2.0, "mark_text": "", "label_style": "digit"},
    "material": {"kind": "solid", "tint": "#FFFDF800", "opacity": 1.0, "master_alpha": 1.0,
      "hilite_alpha": 1.0, "hilite_on": True, "border_alpha": 1.0}
  },
  {
    "id": "proposal-xuanmo", "name": "玄墨 · 反白", "family": "中性",
    "note": "完全无色相：底 #111214，选中块反白成 #E8EAED，像编辑器里的选区。换壁纸也不影响。",
    "dark": True,
    "colors": {
      "back_color": "#111214FF", "border_color": "#24262AFF", "text_color": "#E8EAEDFF",
      "candidate_text_color": "#E8EAEDFF", "candidate_back_color": "#11121400",
      "comment_text_color": "#868C96FF", "label_color": "#6E747D7F",
      "hilited_candidate_text_color": "#111214FF", "hilited_candidate_back_color": "#E8EAEDFF",
      "hilited_candidate_label_color": "#5A6068FF", "hilited_comment_text_color": "#5A6068FF",
      "hilited_label_color": "#5A6068FF", "hilited_mark_color": "#111214FF",
      "shadow_color": "#00000077", "hilited_text_color": "#FFFFFFFF",
      "capsule_text_color": "#E8EAEDFF", "capsule_back_color": "#1B1D20FF"
    },
    "layout": {"horizontal": False, "inline_preedit": True, "font_face": "霞鹜文楷 GB 屏幕阅读版",
      "font_point": 16.0, "label_font_point": 12.0, "label_format": "%s.", "corner_radius": 8.0,
      "hilited_corner_radius": 6.0, "border_width": 1.0, "margin_x": 8.0, "margin_y": 6.0,
      "spacing": 6.0, "candidate_spacing": 4.0, "hilite_spacing": 2.0, "hilite_padding": 6.0,
      "line_spacing": 4.0, "min_width": 150.0, "width": 0.0, "shadow_radius": 12.0,
      "shadow_offset_x": 0.0, "shadow_offset_y": 2.0, "mark_text": "", "label_style": "digit"},
    "material": {"kind": "solid", "tint": "#11121400", "opacity": 1.0, "master_alpha": 1.0,
      "hilite_alpha": 1.0, "hilite_on": True, "border_alpha": 1.0}
  },
  {
    "id": "proposal-suzhi", "name": "素纸 · 墨", "family": "中性",
    "note": "亮色对应款：宣纸底 #FAF8F3，选中块用墨 #2E2A25 实心，米色字。无金色，最安静的一支。",
    "dark": False,
    "colors": {
      "back_color": "#FAF8F3FF", "border_color": "#E7E1D6FF", "text_color": "#23211DFF",
      "candidate_text_color": "#23211DFF", "candidate_back_color": "#FAF8F300",
      "comment_text_color": "#8B8478FF", "label_color": "#A39B8D7F",
      "hilited_candidate_text_color": "#FAF8F3FF", "hilited_candidate_back_color": "#2E2A25FF",
      "hilited_candidate_label_color": "#C3BAABFF", "hilited_comment_text_color": "#C3BAABFF",
      "hilited_label_color": "#C3BAABFF", "hilited_mark_color": "#FAF8F3FF",
      "shadow_color": "#3C2D1418", "hilited_text_color": "#2E2A25FF",
      "capsule_text_color": "#23211DFF", "capsule_back_color": "#F0EBE1FF"
    },
    "layout": {"horizontal": False, "inline_preedit": True, "font_face": "霞鹜文楷 GB 屏幕阅读版",
      "font_point": 16.0, "label_font_point": 12.0, "label_format": "%s.", "corner_radius": 10.0,
      "hilited_corner_radius": 7.0, "border_width": 1.0, "margin_x": 8.0, "margin_y": 6.0,
      "spacing": 6.0, "candidate_spacing": 4.0, "hilite_spacing": 2.0, "hilite_padding": 6.0,
      "line_spacing": 4.0, "min_width": 150.0, "width": 0.0, "shadow_radius": 14.0,
      "shadow_offset_x": 0.0, "shadow_offset_y": 2.0, "mark_text": "", "label_style": "digit"},
    "material": {"kind": "solid", "tint": "#FAF8F300", "opacity": 1.0, "master_alpha": 1.0,
      "hilite_alpha": 1.0, "hilite_on": True, "border_alpha": 1.0}
  },
]

def score(s):
    """按页面背景（亮/暗两种）分别量正文与选中块反差，取较差的一侧。"""
    def on(page):
        bg = over(s['colors']['back_color'], page)
        body = contrast(over(s['colors']['candidate_text_color'], bg), bg) if False else contrast(s['colors']['candidate_text_color'], bg)
        hl = over(s['colors']['hilited_candidate_back_color'], page)
        block = contrast(hl, bg)
        cmt = contrast(s['colors']['comment_text_color'], bg)
        return body, cmt, block
    light = on('#FFFFFF')
    dark = on('#2B2B2E')
    body = min(light[0], dark[0])
    cmt = min(light[1], dark[1])
    block = min(light[2], dark[2])
    alpha = parse(s['colors']['back_color'])[3]
    return {'body': body, 'comment': cmt, 'block': block, 'alpha': alpha}

def main():
    skins = load_live()
    for s in skins:
        s['_score'] = score(s)
    for s in NEW:
        s['_score'] = score(s)

    payload = json.dumps({'live': skins, 'new': NEW}, ensure_ascii=False)
    out = os.path.join(HERE, '皮肤梳理预览.html')
    with open(out, 'w', encoding='utf-8') as f:
        f.write(HTML.replace('__DATA__', payload))
    print('已生成', out)
    # 顺带把评分表打到控制台
    print('\n%-14s %-18s %6s %6s %6s %5s' % ('id', 'name', '正文', '注释', '选中块', '底色A'))
    for s in sorted(skins, key=lambda x: x['_score']['block']):
        c = s['_score']
        print('%-14s %-18s %6.2f %6.2f %6.2f %5d' % (s['id'][5:], s['name'], c['body'], c['comment'], c['block'], c['alpha']))

HTML = r"""<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>虎符 · 候选窗皮肤梳理</title>
<style>
:root{
  --ink:#131417;--ink2:#17181c;--surface:#1a1d22;--surface2:#20242a;
  --line:#2a2e36;--line2:#383d47;
  --text:#f0ede6;--text2:#c6c1b5;--text3:#9d978b;
  --gold:#e7b75c;--gold2:#f3c877;--goldsoft:rgba(231,183,92,.12);
  --red:#e06a5a;--green:#63c08d;
}
*{box-sizing:border-box}
html,body{margin:0;background:#0b0c0e;color:var(--text);
  font:14px/1.6 "Segoe UI Variable Text","Segoe UI","Microsoft YaHei UI",sans-serif;
  -webkit-font-smoothing:antialiased}
.wrap{max-width:1180px;margin:0 auto;padding:56px 32px 120px}
h1{font-size:26px;font-weight:650;letter-spacing:.01em;margin:0 0 6px}
h2{font-size:17px;font-weight:600;margin:64px 0 4px;letter-spacing:.01em}
h2:before{content:"";display:inline-block;width:3px;height:15px;background:var(--gold);
  border-radius:2px;margin-right:10px;vertical-align:-2px}
.sub{color:var(--text3);font-size:13px;margin:0 0 26px;max-width:760px}
.sub b{color:var(--text2);font-weight:600}
.sub em{font-style:normal;color:var(--gold2)}
code{font-family:"Cascadia Mono",Consolas,monospace;font-size:12.5px;
  background:var(--surface2);padding:1.5px 5px;border-radius:4px;color:var(--gold2)}

/* ── 候选窗 ─────────────────────────────────────────── */
.cand{border-radius:var(--r,9px);padding:var(--py,6px) var(--px,8px) calc(var(--py,6px) + 2px);
  width:196px;position:relative;text-align:left}
.cand .row{display:flex;align-items:center;height:30px;padding:0 6px;border-radius:6px}
.cand .idx{font-size:11px;margin-right:7px;font-variant-numeric:tabular-nums;min-width:13px}
.cand .cmt{font-size:10px;margin-left:auto;opacity:.82}

/* 背景台 */
.stage{border-radius:12px;padding:34px 26px;display:flex;flex-wrap:wrap;gap:22px;
  justify-content:flex-start;align-items:flex-start;border:1px solid var(--line)}
.stage.warm{background:
  radial-gradient(circle at 18% 22%, #4a3a28 0, transparent 46%),
  radial-gradient(circle at 78% 74%, #1f2a3a 0, transparent 52%),
  linear-gradient(135deg,#2a2620,#16181c 60%,#24201a)}
.stage.light{background:
  radial-gradient(circle at 22% 26%, #e6dcc8 0, transparent 48%),
  radial-gradient(circle at 80% 76%, #cfd8e2 0, transparent 54%),
  linear-gradient(135deg,#f2ece0,#e8e6e2 60%,#efe7d9)}
.stage.dark{background:
  radial-gradient(circle at 20% 24%, #23262c 0, transparent 50%),
  linear-gradient(135deg,#0e1013,#171a1f 60%,#101215)}

.card{display:flex;flex-direction:column;gap:9px;width:196px}
.card .meta{font-size:12px;color:var(--text3);display:flex;gap:6px;align-items:baseline;flex-wrap:wrap}
.card .meta b{color:var(--text2);font-weight:600;font-size:12.5px}
.badge{font-size:10.5px;padding:1px 6px;border-radius:4px;background:var(--surface2);
  color:var(--text3);border:1px solid var(--line2)}
.badge.bad{color:var(--red);border-color:rgba(224,106,90,.35);background:rgba(224,106,90,.1)}
.badge.ok{color:var(--green);border-color:rgba(99,192,141,.3);background:rgba(99,192,141,.09)}

/* 诊断条 */
.bars{display:flex;flex-direction:column;gap:2px;margin-top:18px}
.bar{display:grid;grid-template-columns:120px 1fr 54px;align-items:center;gap:12px;
  padding:5px 10px;border-radius:6px;font-size:12.5px}
.bar:nth-child(odd){background:rgba(255,255,255,.018)}
.bar .nm{color:var(--text2);overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.bar .track{display:block;height:7px;border-radius:4px;background:var(--surface2);position:relative;overflow:hidden}
.bar .fill{display:block;height:100%;border-radius:4px;background:linear-gradient(90deg,#8a5e12,var(--gold))}
.bar.dim .fill{background:linear-gradient(90deg,#5a2222,var(--red))}
.bar .v{text-align:right;font-variant-numeric:tabular-nums;color:var(--text3);font-size:12px}
.bar.dim .v{color:var(--red)}
.thresh{font-size:12px;color:var(--text3);margin-top:10px}
.thresh b{color:var(--gold2)}

/* 规范表 */
table{border-collapse:collapse;width:100%;font-size:13px;margin-top:14px}
th,td{text-align:left;padding:8px 12px;border-bottom:1px solid var(--line)}
th{color:var(--text3);font-weight:600;font-size:12px;letter-spacing:.04em}
td{color:var(--text2)}
td code{font-size:12px}

.note{background:var(--goldsoft);border:1px solid rgba(231,183,92,.22);border-left:3px solid var(--gold);
  border-radius:8px;padding:14px 18px;margin:18px 0;font-size:13.5px;color:var(--text2)}
.note b{color:var(--gold2)}
.ctl{display:flex;gap:8px;margin:16px 0 22px}
.ctl button{background:var(--surface);border:1px solid var(--line2);color:var(--text2);
  padding:6px 14px;border-radius:7px;font-size:12.5px;cursor:pointer;font-family:inherit}
.ctl button.on{background:var(--goldsoft);border-color:rgba(231,183,92,.4);color:var(--gold2)}
</style>
</head>
<body>
<div class="wrap">
  <h1>候选窗皮肤梳理</h1>
  <p class="sub">数据源：真机 <code>D:\HuFu\数据\皮肤</code>（打包时整目录覆盖打包源，这套就是发出去的那套）。<br>
  所有候选窗都按 <code>hufu-skin</code> 的真实字段渲染，字号、圆角、内边距、选中块逐项取自各自 JSON。</p>

  <h2 id="sec1">一、现状：21 款并排</h2>
  <p class="sub">同一块背景、同一字号，只看一件事：<b>你的眼睛能不能一眼落在选中项上</b>。
  下面组里默认放在「暖色桌面」上——真实使用最常见的场景。</p>
  <div class="ctl" id="ctl1"></div>
  <div class="stage warm" id="stage1"></div>

  <h2 id="sec2">二、诊断：选中块与底色的反差</h2>
  <p class="sub">这是「选中项在哪」的唯一客观指标。行业里一个实心选中块至少要 <b>1.6</b> 才扫得出来，
  低于 1.3 基本等于没有。这 21 款里绝大多数在 1.2–1.5。</p>
  <div class="bars" id="bars"></div>
  <p class="thresh">另：<code>alpha</code> 是 <code>back_color</code> 的底色不透明度。Windows 已不提供背景模糊，
  凡是低于 <b>0xE0</b> 的，桌面会原样透进来，字色与底的对比度取决于壁纸——等于不可控。</p>

  <h2 id="sec3">三、新方向：从设置窗 token 派生</h2>
  <p class="sub">设置窗 CSS 里已经有一套 token：墨 <code>#131417</code>、金 <code>#e7b75c</code>、暖白 <code>#f0ede6</code>。
  候选窗直接继承它，就自然跟整套软件是一家人。四款两个明暗配对，都是 <b>不透明底 + 实心选中块</b>。</p>
  <div class="ctl" id="ctl2"></div>
  <div class="stage warm" id="stage2"></div>

  <h2 id="sec4">四、规范</h2>
  <table>
    <tr><th>层级</th><th>角色</th><th>规则</th><th>门槛</th></tr>
    <tr><td>1</td><td>底色 <code>back_color</code></td><td>必须不透明（alpha = <code>FF</code>）。要半透明就得有模糊，而模糊已经退役。</td><td>—</td></tr>
    <tr><td>2</td><td>选中块 <code>hilited_candidate_back_color</code></td><td>实心，与底反差不低于 3:1。深色底用金色，浅色底用青铜或墨。</td><td>≥ 3.0</td></tr>
    <tr><td>3</td><td>正文 <code>candidate_text_color</code></td><td>始终可读，不随选中与否大幅改变字重。</td><td>≥ 7.0</td></tr>
    <tr><td>4</td><td>注释 <code>comment_text_color</code></td><td>比正文低一档，但拼音在 10px 下也得认得出。</td><td>≥ 4.5</td></tr>
    <tr><td>5</td><td>序号 <code>label_color</code></td><td>未选中时故意压低，选中时随块走。允许低于正文。</td><td>≥ 2.5</td></tr>
  </table>
  <div class="note"><b>命名规则</b>：<code>色名 · 变体</code>，不再混用「写意色名」和「功能描述」。<br>
  例如「迷雾改」「玻璃横排」「全透明横排」这类名字应当退役；「晨白 · 朱砂」「夜航 · 银沙」这种已经在正确的路上。</div>
</div>

<script>
const DATA = __DATA__;

const ROWS = [
  {t:'我们', c:'wǒ men'}, {t:'我', c:'wǒ'}, {t:'我军', c:'wǒ jūn'},
  {t:'War', c:''}, {t:'哇', c:'wa'}, {t:'蛙', c:'wā'}
];

function hex8(h){ h=(h||'').replace('#',''); if(h.length!==6&&h.length!==8) h='14161AFF'; if(h.length===6) h+='FF'; return h; }
function rgba(h, k){
  h = hex8(h);
  const r=parseInt(h.slice(0,2),16), g=parseInt(h.slice(2,4),16), b=parseInt(h.slice(4,6),16);
  const a=parseInt(h.slice(6,8),16)/255;
  return `rgba(${r},${g},${b},${(a*k).toFixed(3)})`;
}
function labelFor(n, style){
  if(style==='zh') return ['一','二','三','四','五','六','七','八','九','十'][n-1]||String(n);
  if(style==='roman') return ['Ⅰ','Ⅱ','Ⅲ','Ⅳ','Ⅴ','Ⅵ','Ⅶ','Ⅷ','Ⅸ','Ⅹ'][n-1]||String(n);
  return n===10?'0':String(n);
}

function cand(s){
  const c=s.colors||{}, l=s.layout||{}, m=s.material||{};
  const master = m.master_alpha ?? 1;
  const hlA = m.hilite_alpha ?? 1;
  const font = Math.max(12, Number(l.font_point)||16);
  const radius = Number(l.corner_radius)||9;
  const px = Number(l.margin_x)||8, py = Number(l.margin_y)||6;
  const rowPad = Number(l.hilite_padding)||6;
  const rowH = Math.round(font*1.85);
  const bw = Number(m.border_width ?? l.border_width ?? 1);
  const face = l.font_face || '"Microsoft YaHei UI",sans-serif';

  const el = document.createElement('div');
  el.className='cand';
  el.style.cssText = `
    background:${rgba(c.back_color, master)};
    border:${bw>0? bw+'px solid '+rgba(c.border_color, 1) : 'none'};
    border-radius:${radius}px; padding:${py}px ${px}px ${py+2}px;
    box-shadow:0 ${Number(l.shadow_offset_y)||2}px ${Number(l.shadow_radius)||12}px ${rgba(c.shadow_color,1)};
    font-family:${face}; font-size:${font}px; color:${c.candidate_text_color};`;

  ROWS.forEach((row,i)=>{
    const active = i===0 && m.hilite_on!==false;
    const r=document.createElement('div');
    r.className='row';
    r.style.cssText=`height:${rowH}px;border-radius:${Number(l.hilited_corner_radius)||6}px;
      padding:0 ${Math.max(6,rowPad)}px;
      ${active?`background:${rgba(c.hilited_candidate_back_color,hlA)}`:''}`;
    r.style.color = active ? c.hilited_candidate_text_color : c.candidate_text_color;
    const idx=labelFor(i+1,l.label_style);
    r.innerHTML = `<span class="idx" style="color:${active?c.hilited_candidate_label_color:c.label_color}">${idx}${(l.label_format||'%s.').replace('%s','')}</span>`
      + `<span>${row.t}</span>`
      + (row.c?`<span class="cmt" style="color:${active?c.hilited_comment_text_color:c.comment_text_color}">${row.c}</span>`:'');
    el.appendChild(r);
  });
  return el;
}

function card(s, opts={}){
  const box=document.createElement('div'); box.className='card';
  box.appendChild(cand(s));
  const sc=s._score||{};
  const meta=document.createElement('div'); meta.className='meta';
  const isNew = !!opts.isNew;
  meta.innerHTML = `<b>${s.name}</b>`
    + (isNew?'':`<span class="badge ${sc.block<1.6?'bad':'ok'}">选中块 ${sc.block.toFixed(2)}</span>`)
    + (sc.alpha<224?`<span class="badge bad">底色 ${sc.alpha.toString(16).toUpperCase()}</span>`:'');
  box.appendChild(meta);
  return box;
}

/* ── 一、现状 ── */
const stage1=document.getElementById('stage1');
const ctl1=document.getElementById('ctl1');
const BGS=[['warm','暖色桌面'],['light','浅色文档'],['dark','深色终端']];
BGS.forEach(([k,t],i)=>{
  const b=document.createElement('button'); b.textContent=t; if(i===0)b.classList.add('on');
  b.onclick=()=>{ stage1.className='stage '+k; [...ctl1.children].forEach(x=>x.classList.remove('on')); b.classList.add('on'); };
  ctl1.appendChild(b);
});
DATA.live.forEach(s=>stage1.appendChild(card(s)));

/* ── 二、诊断条 ── */
const bars=document.getElementById('bars');
[...DATA.live].sort((a,b)=>a._score.block-b._score.block).forEach(s=>{
  const sc=s._score;
  const row=document.createElement('div'); row.className='bar'+(sc.block<1.6?' dim':'');
  const pct=Math.min(100,(sc.block/4)*100);
  row.innerHTML=`<span class="nm">${s.name}</span>
    <span class="track"><span class="fill" style="width:${pct}%"></span></span>
    <span class="v">${sc.block.toFixed(2)}</span>`;
  bars.appendChild(row);
});

/* ── 三、新方向 ── */
const stage2=document.getElementById('stage2');
const ctl2=document.getElementById('ctl2');
BGS.forEach(([k,t],i)=>{
  const b=document.createElement('button'); b.textContent=t; if(i===0)b.classList.add('on');
  b.onclick=()=>{ stage2.className='stage '+k; [...ctl2.children].forEach(x=>x.classList.remove('on')); b.classList.add('on'); };
  ctl2.appendChild(b);
});
DATA.new.forEach(s=>stage2.appendChild(card(s,{isNew:true})));
</script>
</body>
</html>
"""

if __name__ == '__main__':
    main()
