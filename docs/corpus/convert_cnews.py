# -*- coding: utf-8 -*-
# cnews.train.txt (label\tcontent) → THUCNews_<类目>.jsonl（prepare_corpus_50k.py 的输入格式）
import json, os
base = os.path.dirname(os.path.abspath(__file__))
out_counts = {}
with open(os.path.join(base, 'cnews.train.txt'), encoding='utf-8') as f:
    handles = {}
    for line in f:
        line = line.rstrip('\n')
        if '\t' not in line:
            continue
        label, content = line.split('\t', 1)
        if label not in handles:
            handles[label] = open(os.path.join(base, f'THUCNews_{label}.jsonl'), 'w', encoding='utf-8')
            out_counts[label] = 0
        handles[label].write(json.dumps({'content': content}, ensure_ascii=False) + '\n')
        out_counts[label] += 1
    for h in handles.values():
        h.close()
print('converted:', out_counts)