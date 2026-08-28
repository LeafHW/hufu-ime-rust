# -*- coding: utf-8 -*-
"""
语料准备脚本：把下载的中文语料整理成「整句录入准确率测试」用的 1 万句测试集。
- 切句：按句末标点（。！？…；换行）切分
- 清洗：只保留汉字（去掉英文/数字/URL/表情/标点/空白）
- 过滤：每句 4~30 个汉字（整句虎 4 码以上取首选的合理区间）
- 去重
- 混合抽样：LCSTS / THUCNews / 评论 三类按比例随机抽，固定随机种子可复现
输出：
  corpus/test_sentences.txt            每行一句（纯汉字），共 10000 句
  corpus/test_sentences_with_source.txt 每行：来源<TAB>句子
  corpus/语料统计.txt                   各来源抽取量与长度分布
"""
import csv
import json
import os
import random
import re

BASE = os.path.dirname(os.path.abspath(__file__))

# 切句标点：句末/分号/省略号/换行；逗号顿号不切（保留在句内再清洗掉）
SENT_SPLIT = re.compile(r"[。！？；…\n]+")
# 清洗：只保留汉字
HAN = re.compile(r"[^\u4e00-\u9fff]")
# 含英文/数字的句子无法用码表录入，整句丢弃（避免删字符后留下"年月日""块钱"类残句）
ASCII_OR_DIGIT = re.compile(r"[A-Za-z0-9]")
# 人名过滤：常见姓氏/复姓 + 1~2 字名字 + 谓词（说/称/表示/认为/获…）
SURNAMES = ("王李张刘陈杨黄赵吴周徐孙马朱胡郭何高林罗郑梁谢宋唐许韩冯邓曹彭曾肖田董袁"
            "潘于蒋蔡余杜叶程苏魏吕丁任沈姚卢姜崔钟谭陆汪范金石廖贾夏韦付方白邹孟熊秦"
            "邱江尹薛闫段雷侯龙史陶黎贺顾毛郝龚邵万钱严覃武戴莫孔向汤")
DOUBLE_SURNAMES = ("欧阳|司马|诸葛|司徒|上官|东方|皇甫|尉迟|公孙|长孙|宇文|南宫|端木|夏侯"
                   "|令狐|轩辕|独孤|呼延|慕容|钟离|闾丘|万俟")
PREDICATES = ("说|称|表示|认为|指出|强调|透露|介绍|告诉|回应|报道|坦言|自称|感叹|评价"
              "|获|当选|接受|出席")
NAME_PAT = re.compile(r"(?:" + DOUBLE_SURNAMES + r"|[" + SURNAMES + r"])[\u4e00-\u9fff]{1,2}(?:"
                      + PREDICATES + r")")
# 误伤豁免：这些词里的"姓+字+谓词"不是人名
NAME_EXEMPT = ("可以说", "可以说是", "没想到", "觉得")

# 生僻字过滤：字必须在整句虎字频 rank<=3500 的常用字表里
_BASE = os.path.dirname(os.path.abspath(__file__))
COMMON_CHARS = set()
_cp = os.path.join(_BASE, "common_chars.txt")
if os.path.exists(_cp):
    COMMON_CHARS = set(open(_cp, encoding="utf-8").read())

# 地名（省级/常见城市/常见国家地区）
PLACES = ("北京|上海|天津|重庆|河北|山西|辽宁|吉林|黑龙江|江苏|浙江|安徽|福建|江西|山东|河南|"
          "湖北|湖南|广东|海南|四川|贵州|云南|陕西|甘肃|青海|台湾|内蒙古|广西|西藏|宁夏|新疆|"
          "香港|澳门|深圳|广州|杭州|南京|武汉|成都|西安|苏州|青岛|大连|厦门|宁波|济南|郑州|"
          "长沙|沈阳|哈尔滨|长春|合肥|福州|南昌|昆明|贵阳|兰州|太原|石家庄|乌鲁木齐|呼和浩特|"
          "西宁|银川|拉萨|海口|南宁|"
          "洛杉矶|纽约|伦敦|巴黎|东京|莫斯科|悉尼|柏林|罗马|首尔|曼谷|迪拜|华盛顿|芝加哥|"
          "波士顿|旧金山|温哥华|多伦多|墨尔本|威尼斯|佛罗伦萨|巴塞罗那|马德里|维也纳|阿姆斯特丹|"
          "布鲁塞尔|日内瓦|苏黎世|哥本哈根|斯德哥尔摩|奥斯陆|赫尔辛基|里斯本|雅典|伊斯坦布尔|"
          "开罗|内罗毕|约翰内斯堡|孟买|新德里|雅加达|马尼拉|河内|吉隆坡|平壤|大阪|京都|横滨|"
          "名古屋|釜山|惠灵顿|奥克兰|墨西哥城|圣保罗|里约热内卢|布宜诺斯艾利斯|利马|波哥大|"
          "圣地亚哥|巴格达|德黑兰|特拉维夫|耶路撒冷|加尔各答|金奈|卡拉奇|达卡|科伦坡|仰光|"
          "万象|金边|乌兰巴托|加德满都|伊斯兰堡|喀布尔|"
          "美国|英国|法国|德国|日本|韩国|俄罗斯|乌克兰|澳大利亚|加拿大|印度|巴西|意大利|"
          "西班牙|土耳其|伊朗|伊拉克|朝鲜|越南|泰国|新加坡|马来西亚|菲律宾|印尼|荷兰|瑞士|"
          "瑞典|挪威|芬兰|丹麦|波兰|希腊|埃及|南非|墨西哥|阿根廷|智利|巴基斯坦|阿富汗|"
          "尼日利亚|埃塞俄比亚|肯尼亚|沙特|阿联酋|卡塔尔|以色列|约旦|叙利亚|黎巴嫩|塞浦路斯|"
          "捷克|匈牙利|罗马尼亚|保加利亚|塞尔维亚|克罗地亚|斯洛文尼亚|斯洛伐克|爱沙尼亚|"
          "拉脱维亚|立陶宛|白俄罗斯|摩尔多瓦|格鲁吉亚|亚美尼亚|阿塞拜疆|哈萨克斯坦|"
          "乌兹别克斯坦|土库曼斯坦|吉尔吉斯斯坦|塔吉克斯坦|蒙古|尼泊尔|不丹|斯里兰卡|"
          "马尔代夫|缅甸|老挝|柬埔寨|文莱|东帝汶|巴布亚新几内亚|斐济|新西兰|孟加拉国|"
          "阿尔巴尼亚|波黑|北马其顿|黑山|卢森堡|爱尔兰|冰岛|葡萄牙|安道尔|摩纳哥|马耳他")
PLACE_PAT = re.compile(PLACES)
# 机构/品牌后缀
ORG_PAT = re.compile(r"[\u4e00-\u9fff]{1,8}(?:大学|学院|公司|集团|股份|有限|银行|医院|中心|"
                     r"基金|控股|科技|网络|传媒|报社|电视台|协会|委员会|证券|航空|石油|钢铁|"
                     r"研究所|研究院|工程|建筑|地产|电器|电子|数码|汽车|食品|生物|医药|化工)")

def load_lcsts(path, limit=None):
    rows = []
    with open(path, encoding="utf-8") as f:
        for i, line in enumerate(f):
            if limit and i >= limit:
                break
            line = line.strip()
            if not line:
                continue
            try:
                obj = json.loads(line)
            except Exception:
                continue
            text = obj.get("short_text") or obj.get("text") or obj.get("content") or obj.get("summary") or ""
            if text:
                rows.append(("LCSTS", text))
    return rows

def load_thucnews(paths):
    rows = []
    for p in paths:
        name = os.path.basename(p).replace("THUCNews_", "").replace(".jsonl", "")
        with open(p, encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                try:
                    obj = json.loads(line)
                except Exception:
                    continue
                content = obj.get("content") or ""
                if content:
                    rows.append((name, content))
    return rows

def load_reviews(paths):
    rows = []
    for p in paths:
        name = os.path.basename(p)
        with open(p, encoding="utf-8-sig") as f:
            reader = csv.DictReader(f)
            for rec in reader:
                text = (rec.get("review") or "").strip()
                if text:
                    rows.append((name, text))
    return rows

def has_name(text):
    if any(w in text for w in NAME_EXEMPT):
        return False
    if NAME_PAT.search(text):
        return True
    if PLACE_PAT.search(text):
        return True
    if ORG_PAT.search(text):
        return True
    return False


def is_common(text):
    return all(ch in COMMON_CHARS for ch in text)


def split_and_clean(text):
    """按句末标点切句；含英文/数字、人名/地名/机构、生僻字的句子整句丢弃；
    清洗成纯汉字。返回 4~30 字且全为常用字的句子列表。"""
    out = []
    for piece in SENT_SPLIT.split(text):
        if ASCII_OR_DIGIT.search(piece):
            continue
        clean = HAN.sub("", piece)
        if 4 <= len(clean) <= 30 and not has_name(clean) and is_common(clean):
            out.append(clean)
    return out

def main():
    random.seed(4321)
    src = []

    lcsts_path = os.path.join(BASE, "LCSTS_train.jsonl")
    if os.path.exists(lcsts_path):
        src.extend(load_lcsts(lcsts_path))
    print("LCSTS raw rows:", len(src))

    thuc = sorted(p for p in os.listdir(BASE) if p.startswith("THUCNews_") and p.endswith(".jsonl"))
    src.extend(load_thucnews([os.path.join(BASE, p) for p in thuc]))
    print("THUCNews files:", len(thuc))

    reviews = [
        os.path.join(BASE, "waimai_10k.csv"),
        os.path.join(BASE, "ChnSentiCorp_htl_all.csv"),
        os.path.join(BASE, "online_shopping", "online_shopping_10_cats.csv"),
    ]
    for r in reviews:
        if os.path.exists(r):
            src.extend(load_reviews([r]))
    print("total raw rows:", len(src))

    # 切句 + 清洗 + 去重
    seen = set()
    cleaned = []  # (source, sentence)
    for src_name, text in src:
        for s in split_and_clean(text):
            if s not in seen:
                seen.add(s)
                cleaned.append((src_name, s))
    print("cleaned unique sentences:", len(cleaned))

    # 按来源分组，混合抽样（固定配比：LCSTS 3500 / THUCNews 4500 / 评论 2000）
    groups = {}
    for src_name, s in cleaned:
        if src_name == "LCSTS":
            key = "LCSTS"
        elif src_name.startswith("THUCNews") or src_name in (
                "体育", "娱乐", "家居", "彩票", "房产", "教育", "时尚",
                "时政", "游戏", "社会", "科技", "股票", "财经", "星座"):
            key = "THUCNews"
        else:
            key = "REVIEW"
        groups.setdefault(key, []).append((src_name, s))
    print("group sizes:", {k: len(v) for k, v in groups.items()})

    quota = {"LCSTS": 15000, "THUCNews": 30000, "REVIEW": 10000}
    picked = []
    for key, n in quota.items():
        pool = groups.get(key, [])
        k = min(n, len(pool))
        picked.extend(random.sample(pool, k))
    random.shuffle(picked)
    picked = picked[:50000]

    with open(os.path.join(BASE, "test_sentences_50k.txt"), "w", encoding="utf-8") as f:
        for _, s in picked:
            f.write(s + "\n")
    with open(os.path.join(BASE, "test_sentences_50k_with_source.txt"), "w", encoding="utf-8") as f:
        for src_name, s in picked:
            f.write(f"{src_name}\t{s}\n")

    # 统计
    from collections import Counter
    src_count = Counter(src_name for src_name, _ in picked)
    lens = Counter(len(s) for _, s in picked)
    with open(os.path.join(BASE, "语料统计_50k.txt"), "w", encoding="utf-8") as f:
        f.write(f"total: {len(picked)}\n")
        f.write("\nsource distribution:\n")
        for k, v in src_count.most_common():
            f.write(f"  {k}: {v}\n")
        f.write("\nlength distribution (hanzi count):\n")
        for k in sorted(lens):
            f.write(f"  {k}: {lens[k]}\n")
    print("DONE, picked:", len(picked))

if __name__ == "__main__":
    main()
