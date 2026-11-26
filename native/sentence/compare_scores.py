#!/usr/bin/env python3
"""Compare isolated C-ABI scoring runs; never starts the input-method server.
Run on Windows so both native ARM64 and emulated x64 probes can execute.
"""
import argparse, hashlib, json, math, subprocess
from pathlib import Path

def sha(path):
    h=hashlib.sha256()
    with path.open('rb') as f:
        for b in iter(lambda:f.read(1024*1024),b''):h.update(b)
    return h.hexdigest()

def main():
    p=argparse.ArgumentParser()
    for name in ['baseline-probe','baseline-dll','candidate-probe','candidate-dll','model','cases','output']:
        p.add_argument('--'+name,type=Path,required=True)
    p.add_argument('--absolute-tolerance',type=float,default=1e-4)
    p.add_argument('--relative-tolerance',type=float,default=1e-6)
    a=p.parse_args();a.output.mkdir(parents=True,exist_ok=True)
    inputs={name:{'path':str(getattr(a,name)), 'sha256':sha(getattr(a,name))} for name in ['baseline_probe','baseline_dll','candidate_probe','candidate_dll','model','cases']}
    scores=[]
    for label in ['baseline','candidate']:
        tsv=a.output/(label+'.tsv')
        run=subprocess.run([str(getattr(a,label+'_probe')),str(getattr(a,label+'_dll')),str(a.model),str(a.cases),str(tsv)],capture_output=True,timeout=900)
        (a.output/(label+'.stdout')).write_bytes(run.stdout);(a.output/(label+'.stderr')).write_bytes(run.stderr)
        if run.returncode:raise SystemExit(f'{label} probe failed with {run.returncode}; see {a.output}')
        rows=[line.split('\t') for line in tsv.read_text().splitlines()]
        if any(int(index)!=n for n,(index,_) in enumerate(rows)):raise SystemExit('non-contiguous output')
        scores.append([float(value) for _,value in rows])
    x,y=scores
    count=len([line for line in a.cases.read_text(encoding='utf-8').splitlines() if line])
    if len(x)!=count or len(y)!=count or not count:raise SystemExit('incomplete score output')
    finite=all(math.isfinite(v) for v in x+y)
    differences=[abs(u-v) for u,v in zip(x,y)]
    numeric=finite and all(math.isclose(u,v,rel_tol=a.relative_tolerance,abs_tol=a.absolute_tolerance) for u,v in zip(x,y))
    orders=[(sorted(range(len(x[i:i+5])),key=lambda n:-x[i+n]), sorted(range(len(y[i:i+5])),key=lambda n:-y[i+n])) for i in range(0,count,5)]
    rank_mismatches=[i for i,(a,b) in enumerate(orders) if a!=b]
    top1_mismatches=[i for i,(a,b) in enumerate(orders) if a[0]!=b[0]]
    rank_equal=not rank_mismatches
    report={'schema':1,'inputs':inputs,'cases':count,'group_size':5,'finite':finite,'max_absolute_difference':max(differences),'absolute_tolerance':a.absolute_tolerance,'relative_tolerance':a.relative_tolerance,'numeric_passed':numeric,'ranking_passed':rank_equal,'ranking_mismatch_groups':rank_mismatches,'top1_mismatch_groups':top1_mismatches,'passed':numeric and rank_equal,'installed':False}
    (a.output/'comparison.json').write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
    print(json.dumps(report,ensure_ascii=False));raise SystemExit(0 if report['passed'] else 1)
if __name__=='__main__':main()
