#!/usr/bin/env python3
"""Windows-only, isolated released v1.9.9.9 downloader compatibility probe.
Never calls /api/update/open, installs, or registers. Existing HuFu blocks execution.
"""
import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import threading
import time
import urllib.request
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

BASELINE_SHA = '2638031e7a1f922ac1b8379e49cf995bced3b29edf243b7b11d86c6a9a6dcc91'
CLSID = '{8F5C2A10-3E77-4B9C-A1D4-9E0B7C2F5A88}'
LIMIT = 64 * 1024 * 1024

def sha(path):
    with path.open('rb') as f: return hashlib.file_digest(f, 'sha256').hexdigest()

def safety_check():
    import winreg
    found = []
    for hive, name in [(winreg.HKEY_CURRENT_USER, 'HKCU'), (winreg.HKEY_LOCAL_MACHINE, 'HKLM')]:
        for view in [winreg.KEY_WOW64_32KEY, winreg.KEY_WOW64_64KEY]:
            for key in [rf'Software\Classes\CLSID\{CLSID}', rf'Software\Microsoft\CTF\TIP\{CLSID}', r'Software\HuFu']:
                try:
                    with winreg.OpenKey(hive, key, 0, winreg.KEY_READ | view): found.append(f'{name}:{view}:{key}')
                except FileNotFoundError: pass
    query = "@(Get-CimInstance Win32_Process -Filter \"Name='hufu-server.exe'\" | Select-Object ProcessId,ExecutablePath) | ConvertTo-Json -Compress"
    process = subprocess.run(['powershell.exe','-NoProfile','-Command',query],capture_output=True,check=True)
    if process.stdout.strip():
        rows = json.loads(process.stdout.decode('utf-8-sig'))
        if rows: found.append({'running_servers':rows})
    # Released render warm-up has fixed legacy fallbacks; do not let it launch them.
    for root in [Path(r'C:\Windows\SystemIME\HuFu'), Path(r'D:\HuFu')]:
        if (root/'hufu-render-helper.exe').exists(): found.append(str(root/'hufu-render-helper.exe'))
    return found

def downloads_dir():
    guid = (ctypes.c_byte * 16).from_buffer_copy(uuid.UUID('374de290-123f-4565-9164-39c4925e467b').bytes_le)
    result = ctypes.c_void_p()
    get = ctypes.windll.shell32.SHGetKnownFolderPath
    get.argtypes = [ctypes.c_void_p,ctypes.c_uint32,ctypes.c_void_p,ctypes.POINTER(ctypes.c_void_p)]
    get.restype = ctypes.c_long
    if get(ctypes.byref(guid),0,None,ctypes.byref(result)) < 0: raise RuntimeError('Cannot resolve Downloads safely')
    try: return Path(ctypes.wstring_at(result))
    finally:
        ctypes.windll.ole32.CoTaskMemFree.argtypes=[ctypes.c_void_p]
        ctypes.windll.ole32.CoTaskMemFree(result)

def free_port():
    with socket.socket() as s:
        s.bind(('127.0.0.1',0)); return s.getsockname()[1]

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--baseline-app',type=Path,required=True)
    p.add_argument('--output',type=Path,required=True)
    p.add_argument('--latest',default='1.9.9.10',help='Version advertised by the local test manifest')
    p.add_argument('--artifact',type=Path,help='Optional final universal EXE: download/hash only; never execute')
    a=p.parse_args()
    a.baseline_app=a.baseline_app.resolve();a.output=a.output.resolve()
    if a.artifact:a.artifact=a.artifact.resolve()
    if os.name!='nt': raise SystemExit('Run with Windows Python; this test checks Windows registry/processes first.')
    if a.output.exists(): raise SystemExit('Output must be a new isolated directory')
    a.output.mkdir(parents=True)
    report={'schema':1,'baseline_sha256':sha(a.baseline_app/'hufu-server.exe'),'installed':False,'tests':[]}
    checks=safety_check(); report['preflight_blockers']=checks
    if checks or report['baseline_sha256']!=BASELINE_SHA:
        (a.output/'report.json').write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf-8')
        raise SystemExit('Safety preflight blocked; no server started.')
    if (a.baseline_app/'version.txt').read_text(encoding='utf-8-sig').strip()!='1.9.9.9': raise SystemExit('Wrong released version')
    dl=downloads_dir()
    if not dl.is_dir(): raise SystemExit('Existing Downloads directory required; will not create or change KnownFolder')
    # Only these two files are allowed outside the isolated root, each unique and absent.
    filename='hufu-update-test-'+uuid.uuid4().hex+'.exe'
    dest=dl/filename; part=dl/('.'+filename+'.part')
    if dest.exists() or part.exists(): raise SystemExit('UUID filename collision')
    isolated_dest=a.output/'USERPROFILE'/'Downloads'/filename
    isolated_part=isolated_dest.with_name('.'+filename+'.part')
    owned_paths=[dest,part,isolated_dest,isolated_part]
    report['download_paths']=[str(p) for p in owned_paths]
    app=a.output/'app';app.mkdir()
    shutil.copyfile(a.baseline_app/'hufu-server.exe',app/'hufu-server.exe')
    shutil.copyfile(a.baseline_app/'version.txt',app/'version.txt')
    data=a.output/'data';data.mkdir()
    schema=a.output/'schemas'/'Test';schema.mkdir(parents=True)
    (schema/'main.txt').write_text('#hufu-dict v1 name=test\na\t甲\n',encoding='utf-8')
    (data/'config.json').write_text(json.dumps({'schema':{'dir':str(schema.parent),'current':'Test'}}),encoding='utf-8')
    (data/'同步.json').write_text('{"enabled":false}',encoding='utf-8')
    env=os.environ.copy()
    for key in ['LOCALAPPDATA','APPDATA','USERPROFILE','TEMP','TMP','HOME']:
        root=a.output/key;root.mkdir();env[key]=str(root)
    for key in list(env):
        if key.lower().endswith('_proxy'):env.pop(key)
    env['NO_PROXY']='127.0.0.1,localhost';env['HUFU_DATA']=str(data)
    state={'manifest':{},'path':None,'requests':[]}
    class Handler(BaseHTTPRequestHandler):
        def log_message(self,*args): pass
        def do_GET(self):
            state['requests'].append(self.path)
            if self.path=='/update.json':
                b=json.dumps(state['manifest']).encode();self.send_response(200);self.send_header('Content-Length',str(len(b)));self.end_headers();self.wfile.write(b)
            elif self.path=='/ok/'+filename:
                path=state['path'];self.send_response(200);self.send_header('Content-Length',str(path.stat().st_size));self.end_headers()
                try:
                    with path.open('rb') as f:shutil.copyfileobj(f,self.wfile,64*1024)
                except (BrokenPipeError,ConnectionResetError,ConnectionAbortedError):pass
            else:self.send_error(503,'intentional first-mirror failure')
    http=ThreadingHTTPServer(('127.0.0.1',0),Handler)
    origin=f'http://127.0.0.1:{http.server_port}'
    env['HUFU_UPDATE_MANIFEST']=origin+'/update.json'
    threading.Thread(target=http.serve_forever,daemon=True).start()
    port=free_port();api=f'http://127.0.0.1:{port}'
    opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
    def request(route,post=False):
        req=urllib.request.Request(api+route,data=b'' if post else None,headers={'Origin':api})
        with opener.open(req,timeout=5) as r:return json.load(r)
    small=a.output/'fixture.bin';small.write_bytes(b'HUFU-UPDATE-ISOLATED\0'*256)
    def manifest(path,bad=False,fallback=False):
        state['path']=path
        urls=([origin+'/fail/'+filename] if fallback else [])+[origin+'/ok/'+filename]
        state['manifest']={'latest':a.latest,'min':'1.0.0','page_url':origin+'/page','size':path.stat().st_size,'sha256':'0'*64 if bad else sha(path),'sources':[{'url':u} for u in urls]}
    manifest(small)
    proc=None
    try:
        with (a.output/'server.stdout').open('wb') as out,(a.output/'server.stderr').open('wb') as err:
            proc=subprocess.Popen([str(app/'hufu-server.exe'),'--data',str(data),'--port',str(port)],cwd=app,env=env,stdout=out,stderr=err)
            for _ in range(100):
                if proc.poll() is not None:raise RuntimeError('Baseline server exited before HTTP readiness')
                try:check=request('/api/update/check?fresh=1');break
                except (OSError,ValueError):time.sleep(.1)
            else:raise RuntimeError('Baseline HTTP did not start at requested isolated port')
            assert check['has_update'] and check['current']=='1.9.9.9' and check['latest']==a.latest,check
            report['tests'].append({'name':'new-version-discovery','passed':True,'response':check})
            def run_case(name,path,success,bad=False,fallback=False):
                if any(p.exists() for p in owned_paths):raise RuntimeError('Previous test file not cleaned')
                manifest(path,bad,fallback);start=len(state['requests']);request('/api/update/download/start',True)
                for _ in range(1200):
                    status=request('/api/update/download/status')
                    if status['phase'] in ['done','error','cancelled']:break
                    time.sleep(.1)
                else:raise RuntimeError('Download did not finish')
                urls=state['requests'][start:]
                ok=(status['phase']=='done') if success else status['phase']=='error'
                actual=None
                if success:
                    actual_path=Path(status['dest'])
                    if actual_path not in [dest,isolated_dest]:raise RuntimeError('Unexpected download location; refusing filesystem access')
                    actual=sha(actual_path) if actual_path.is_file() else None
                    ok=ok and actual==sha(path)
                else:ok=ok and not any(p.exists() for p in owned_paths)
                if fallback:ok=ok and '/fail/'+filename in urls and '/ok/'+filename in urls
                result={'name':name,'passed':ok,'status':status,'requests':urls,'download_sha256':actual}
                report['tests'].append(result)
                for owned in owned_paths:
                    if owned.exists():owned.unlink()
                if not ok:raise RuntimeError(f'{name} failed: {status}')
            run_case('wrong-sha-rejected',small,False,bad=True)
            run_case('first-mirror-fallback',small,True,fallback=True)
            exact=a.output/'exact-64m.bin'
            with exact.open('wb') as f:f.truncate(LIMIT)
            run_case('exact-64m-accepted',exact,True)
            over=a.output/'over-64m.bin'
            with over.open('wb') as f:f.truncate(LIMIT+1)
            run_case('over-64m-rejected',over,False)
            if a.artifact:
                if not 1024<=a.artifact.stat().st_size<=LIMIT:raise RuntimeError('Final artifact violates released downloader size contract')
                run_case('final-universal-exe-download',a.artifact,True,fallback=True)
                report['artifact']={'path':str(a.artifact),'sha256':sha(a.artifact)}
            report['passed']=all(t['passed'] for t in report['tests'])
    finally:
        if proc and proc.poll() is None:
            proc.terminate();proc.wait(timeout=15)
        http.shutdown();http.server_close()
        for path in owned_paths:
            if path.exists():path.unlink()
        report['download_cleanup']=not any(p.exists() for p in owned_paths)
        report['postflight_blockers']=safety_check()
        report['http_requests']=state['requests']
        report['server_exitcode']=proc.returncode if proc else None
        (a.output/'report.json').write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
    print(json.dumps({'passed':report.get('passed',False),'report':str(a.output/'report.json')},ensure_ascii=False))
if __name__=='__main__':main()
