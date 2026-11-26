#!/usr/bin/env python3
"""Render final ARM64 helper/facade in a Chinese-path isolated copy; no registration."""
import argparse,hashlib,json,os,shutil,struct,subprocess,time,urllib.request
from pathlib import Path
from test_legacy_update import safety_check,free_port,sha

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--payload',required=True,type=Path)
    p.add_argument('--resources',required=True,type=Path)
    p.add_argument('--output',required=True,type=Path)
    p.add_argument('--max-skins',type=int,default=4)
    p.add_argument('--ssf-source',type=Path,help='Explicit source for the SSF files embedded in the shipped server')
    a=p.parse_args()
    if os.name!='nt':raise SystemExit('Windows Python required')
    a.payload=a.payload.resolve();a.resources=a.resources.resolve();a.output=a.output.resolve()
    if a.output.exists():raise SystemExit('Output must be new')
    a.output.mkdir(parents=True)
    report={'schema':1,'installed':False,'tests':[],'preflight_blockers':safety_check()}
    if report['preflight_blockers']:
        (a.output/'report.json').write_text(json.dumps(report,indent=2));raise SystemExit('Existing HuFu blocks isolated pipe use')
    manifest=json.loads((a.payload/'package-architecture.json').read_text(encoding='utf-8-sig'))
    if manifest['system_architecture']!='arm64':raise SystemExit('Native ARM64 payload required')
    app=a.output/'中文路径-虎符验证';app.mkdir()
    hashes={}
    for entry in manifest['files']:
        src=a.payload/entry['path'];dst=app/entry['path']
        if sha(src)!=entry['sha256']:raise SystemExit('Source payload hash mismatch')
        dst.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(src,dst);hashes[entry['path']]=entry['sha256']
    shutil.copyfile(a.payload/'package-architecture.json',app/'package-architecture.json')
    report['payload_manifest_sha256']=sha(a.payload/'package-architecture.json')
    report['binary_hashes']={k:v for k,v in hashes.items() if k.endswith(('.dll','.exe'))}
    data=app/'数据';skins=data/'皮肤';skins.mkdir(parents=True)
    sources=sorted((a.ssf_source.resolve() if a.ssf_source else a.resources/'数据'/'皮肤').glob('*.ssf'))[:a.max_skins]
    if not sources:raise SystemExit('No release SSF resources')
    source_hashes={str(f):sha(f) for f in sources}
    for src in sources:shutil.copyfile(src,skins/src.name)
    schema=app/'码表'/'Test';schema.mkdir(parents=True)
    (schema/'main.txt').write_text('#hufu-dict v1 name=test\na\t甲\n',encoding='utf-8')
    (data/'config.json').write_text(json.dumps({'schema':{'dir':str(schema.parent),'current':'Test'}}),encoding='utf-8')
    (data/'同步.json').write_text('{"enabled":false}',encoding='utf-8')
    env={k:v for k,v in os.environ.items() if not k.upper().startswith('HUFU')}
    for key in ['LOCALAPPDATA','APPDATA','USERPROFILE','TEMP','TMP','HOME']:
        dest=a.output/key;dest.mkdir();env[key]=str(dest)
    env.update(HUFU_DATA=str(data),HUFU_DEV_DATA=str(data),HUFU_DEV_SERVER=str(app/'hufu-server.exe'),
        HUFU_RENDER_HELPER=str(app/'hufu-render-helper.exe'),HUFU_RENDER_DLL=str(app/'hufu_tsf.dll'),
        HUFU_TSF_SMOKE='1',HUFU_SKIN_DUMP_V2='1')
    port=free_port();api=f'http://127.0.0.1:{port}'
    opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
    proc=None
    try:
        with (a.output/'server.stdout').open('wb') as out,(a.output/'server.stderr').open('wb') as err:
            proc=subprocess.Popen([str(app/'hufu-server.exe'),'--data',str(data),'--port',str(port)],cwd=app,env=env,stdout=out,stderr=err)
            for _ in range(150):
                if proc.poll() is not None:raise RuntimeError('Isolated server exited')
                try:
                    with opener.open(api+'/api/health',timeout=2) as response:response.read()
                    break
                except urllib.error.HTTPError:break # A 404 still proves our listening server.
                except OSError:time.sleep(.1)
            else:raise RuntimeError('HTTP readiness timeout')
            for src in sources:
                output=a.output/(src.stem+'.bmp');env['HUFU_SKIN_DUMP_ID']=src.stem
                run=subprocess.run([str(app/'hufu-render-helper.exe'),str(output),str(app/'hufu_tsf.dll')],cwd=app,env=env,capture_output=True,timeout=45)
                (a.output/(src.stem+'.stderr')).write_bytes(run.stderr)
                item={'skin':src.name,'source_sha256':source_hashes[str(src)],'returncode':run.returncode}
                if output.exists():
                    b=output.read_bytes();offset=struct.unpack_from('<I',b,10)[0];w,h=struct.unpack_from('<ii',b,18)
                    colors=len(set(b[i:i+4] for i in range(offset,len(b),4)))
                    item.update(bmp_sha256=sha(output),width=w,height=h,pixel_colors=colors,bytes=len(b))
                    item['passed']=run.returncode==0 and b[:2]==b'BM' and w>0 and h>0 and len(b)==offset+w*h*4 and colors>8
                else:item['passed']=False
                report['tests'].append(item)
                if not item['passed']:raise RuntimeError(f'Render failed: {item}')
            report['passed']=True
    finally:
        if proc and proc.poll() is None:proc.terminate();proc.wait(timeout=15)
        # Rendering may warm helper children; do not kill by image name or touch other processes.
        time.sleep(.5)
        report['postflight_blockers']=safety_check()
        report['source_resources_unchanged']=all(sha(Path(f))==h for f,h in source_hashes.items())
        report['source_payload_unchanged']=all(sha(a.payload/f)==h for f,h in hashes.items())
        report['passed']=bool(report.get('passed')) and not report['postflight_blockers'] and report['source_resources_unchanged'] and report['source_payload_unchanged']
        (a.output/'report.json').write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
    print(json.dumps({'passed':report.get('passed',False),'report':str(a.output/'report.json')},ensure_ascii=False))
    if not report['passed']:raise SystemExit(1)
if __name__=='__main__':main()
