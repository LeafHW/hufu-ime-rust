import json, os, shutil, subprocess, tempfile, time
from pathlib import Path
source=Path(r'C:\Workspace\hufu-ime-rust\_tmp\arm64\build\payload-e080f8d8385448229d9fe2f9a4e33973\arm64')
manifest=json.loads((source/'package-architecture.json').read_text(encoding='utf-8'))
sandbox=Path(tempfile.mkdtemp(prefix='hufu-move-diagnosis-'))
results=[]
for name,probes in [('no-probe',[]),('native-arm64',['hufu-tsf-smoke.exe']),('x86',['hufu-tsf-smoke32.exe']),('emulated-x64',['hufu-tsf-smoke-x64.exe'])]:
    stage=sandbox/name;stage.mkdir()
    for f in manifest['files']:
        if f['role']!='program':shutil.copyfile(source/f['path'],stage/f['path'])
    calls=[]
    for tool in probes:
        dll='hufu_tsf32.dll' if '32' in tool else 'hufu_tsf.dll'
        run=subprocess.run([str(stage/tool),'probe-dll',str(stage/dll)],capture_output=True)
        calls.append({'exe':tool,'exit':run.returncode,'stderr':run.stderr.decode('utf-8',errors='replace')})
    result={'name':name,'probes':calls,'source':str(stage)}
    try:
        stage.rename(sandbox/(name+'-published'))
        result['rename']='ok'
    except OSError as error:
        result['rename']=str(error)
        time.sleep(2)
        try:
            stage.rename(sandbox/(name+'-published'))
            result['retry_after_2s']='ok'
        except OSError as retry:result['retry_after_2s']=str(retry)
    results.append(result)
    print(json.dumps(result,ensure_ascii=False),flush=True)
Path(r'C:\Workspace\hufu-ime-rust\_tmp\arm64\registration-fix\move-diagnosis.json').write_text(json.dumps({'sandbox':str(sandbox),'results':results},ensure_ascii=False,indent=2),encoding='utf-8')
