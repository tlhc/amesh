"""Exercise original Codex TUI with an isolated amesh and a local model fixture."""
import argparse
import asyncio
import hashlib
import http.server
import json
import os
import pathlib
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
import uuid

from support.codex_terminal import Terminal
from websockets.asyncio.client import unix_connect

ORIGINAL_CODEX = os.environ.get('AMESH_E2E_CODEX', '/opt/homebrew/Caskroom/codex/0.153.4/bin/codex')
ORIGINAL_SHA = 'b973d440acac501fd2594a43e7ca9ce41e0a65b9dfb28d0d7a7837c99e1261e3'

def ModelServer(portFile):
    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *args): pass
        def do_POST(self):
            self.rfile.read(int(self.headers.get('Content-Length','0')))
            item={'id':'msg_'+uuid.uuid4().hex,'type':'message','role':'assistant','status':'completed','content':[{'type':'output_text','text':'Received.','annotations':[]}]}
            response={'id':'resp_'+uuid.uuid4().hex,'status':'completed','output':[item], 'usage':{'input_tokens':10,'output_tokens':2,'total_tokens':12}}
            events=[{'type':'response.created','response':dict(response,status='in_progress',output=[])},
                    {'type':'response.output_item.added','output_index':0,'item':dict(item,status='in_progress',content=[])},
                    {'type':'response.output_text.delta','item_id':item['id'],'output_index':0,'content_index':0,'delta':'Received.'},
                    {'type':'response.output_item.done','output_index':0,'item':item},
                    {'type':'response.completed','response':response}]
            data=''.join('data: '+json.dumps(event)+'\n\n' for event in events).encode()
            self.send_response(200); self.send_header('Content-Type','text/event-stream'); self.send_header('Content-Length',str(len(data))); self.end_headers(); self.wfile.write(data)
    server=http.server.HTTPServer(('127.0.0.1',0),Handler)
    pathlib.Path(portFile).write_text(str(server.server_port))
    server.serve_forever()

def Sha(path):
    with open(path,'rb') as f: return hashlib.file_digest(f,'sha256').hexdigest()

async def Run(amesh):
    if not os.path.isfile(ORIGINAL_CODEX):
        print('skip: Codex baseline not present:', ORIGINAL_CODEX)
        return
    zshrc=pathlib.Path(os.environ.get('AMESH_E2E_ZSHRC', str(pathlib.Path.home()/'.zshrc')))
    if not zshrc.is_file():
        print('skip: no zshrc for cxcr wrappers:', zshrc)
        return
    shell=zshrc.read_text()
    if '_codex_live_developer_instructions()' not in shell or 'cxcr()' not in shell:
        print('skip: zshrc missing cxcr wrappers')
        return
    resolved=shutil.which('codex')
    if not resolved:
        print('skip: codex not on PATH')
        return
    assert Sha(ORIGINAL_CODEX)==ORIGINAL_SHA, 'Codex binary differs from the original baseline'
    assert os.path.realpath(resolved)==ORIGINAL_CODEX, 'cxr resolves a different Codex binary'
    root=pathlib.Path(tempfile.mkdtemp(prefix='amesh-bind-',dir='/tmp'))
    cwd=root/root.name; cwd.mkdir()
    home=root/'home'; home.mkdir()
    codeHome=home/'.codex'; codeHome.mkdir()
    config=codeHome/'config.toml'
    wrappers=shell[shell.index('_codex_live_developer_instructions()'):shell.index('cxcr()')]
    wrappers+=shell[shell.index('cxcr()'):].splitlines()[0]+'\n'
    wrapperPath=root/'wrappers.zsh'; wrapperPath.write_text(wrappers)
    env={k:v for k,v in os.environ.items() if k not in ['CODEX_THREAD_ID','CODEX_SESSION_ID','AMESH_PEER_ID','AMESH_TOKEN','AMESH_CIRCLE','OPENAI_API_KEY','OPENAI_BASE_URL']}
    with socket.socket() as listener:
        listener.bind(('127.0.0.1',0)); bind='127.0.0.1:'+str(listener.getsockname()[1])
    env.update(HOME=str(home),CODEX_HOME=str(codeHome),AMESH_BIND=bind,AMESH_STATE=str(root/'state.db'),TERM='xterm-256color',NO_PROXY='127.0.0.1,localhost,::1')
    processes=[]; terminals=[]; report={'root':str(root),'codex':ORIGINAL_CODEX,'codex_sha256':Sha(ORIGINAL_CODEX),'amesh':amesh,'amesh_sha256':Sha(amesh),'wrappers_sha256':hashlib.sha256(wrappers.encode()).hexdigest()}
    report.update(harness_sha256=Sha(__file__), terminal_sha256=Sha(pathlib.Path(__file__).parent/'support/codex_terminal.py'), model='local SSE fixture')
    http=urllib.request.build_opener(urllib.request.ProxyHandler({}))
    def Peers(): return json.load(http.open('http://'+bind+'/peers',timeout=3))
    def Spawn(args,name):
        log=(root/(name+'.log')).open('w')
        process=subprocess.Popen(args,env=env,cwd=cwd,stdin=subprocess.PIPE,stdout=log,stderr=log,start_new_session=True); log.close(); processes.append(process); return process
    async def Until(predicate,label,seconds=35):
        deadline=time.monotonic()+seconds
        while time.monotonic()<deadline:
            for terminal in terminals: terminal.read()
            value=predicate()
            if value: return value
            await asyncio.sleep(.1)
        raise AssertionError(label)
    def NewTui(name, resume=False):
        command='source "$1"; shift; '+('cxcr' if resume else 'cxr')+' --no-alt-screen "$@"'
        terminal=Terminal(['/bin/zsh','-f','-c',command,'zsh',str(wrapperPath)],env,str(cwd))
        terminals.append(terminal); return terminal
    def Notify(peer,marker):
        subprocess.run([amesh,'peer','notify',peer,marker,'--from-peer','identity-test'],env=env,cwd=cwd,check=True,stdout=subprocess.DEVNULL)
    def McpPid():
        lines=subprocess.check_output(['ps','-axo','pid=,ppid=,comm=,args='],text=True).splitlines()
        ids=[]
        for line in lines:
            row=line.strip().split(None,3)
            if len(row)==4 and row[1]==str(server.pid) and row[3]==amesh+' mcp': ids.append(int(row[0]))
        return sorted(ids)
    def SaveScreen(terminal,name):
        terminal.read(); text=terminal.text(); (root/(name+'.screen.txt')).write_text(text)
        assert 'OpenAI Codex' in text and 'mock-model' in text, 'TUI capture not ready'
        assert 'amesh_whoami' not in text, 'identity probe polluted TUI'
        return text
    async def Connect():
        ws=await unix_connect(str(codeHome/'app-server-control/app-server-control.sock'),compression=None,proxy=None,max_size=16*1024*1024)
        return ws
    rpcId=0
    async def Rpc(ws,method,params):
        nonlocal rpcId
        rpcId+=1; requestId=rpcId
        await ws.send(json.dumps({'id':requestId,'method':method,'params':params}))
        while True:
            response=json.loads(await asyncio.wait_for(ws.recv(),20))
            if response.get('id')==requestId:
                if response.get('error'): raise AssertionError(response)
                return response['result']
    try:
        Spawn([sys.executable,str(pathlib.Path(__file__).resolve()),'--model-server',str(root/'model-port')],'model')
        await Until(lambda:(root/'model-port').exists(),'model fixture failed to start')
        port=(root/'model-port').read_text()
        config.write_text('model="mock-model"\nmodel_provider="probe"\ncheck_for_update_on_startup=false\napproval_policy="never"\nsandbox_mode="danger-full-access"\n[model_providers.probe]\nname="probe"\nbase_url="http://127.0.0.1:'+port+'/v1"\nwire_api="responses"\nrequires_openai_auth=false\n[mcp_servers.amesh]\ncommand='+json.dumps(amesh)+'\nargs=["mcp"]\n[mcp_servers.amesh.env]\nAMESH_BACKEND="codex"\nAMESH_BIND='+json.dumps(bind)+'\nAMESH_STATE='+json.dumps(str(root/'state.db'))+'\nCODEX_HOME='+json.dumps(str(codeHome))+'\n')
        with config.open('a') as stream:
            stream.write('\n[projects.'+json.dumps(str(cwd.resolve()))+']\ntrust_level="trusted"\n')
        hub=Spawn([amesh,'serve'],'hub')
        def HubReady():
            assert hub.poll() is None, 'isolated hub exited: '+str(hub.returncode)
            try: return json.load(http.open('http://'+bind+'/health',timeout=1)).get('ok') is True
            except urllib.error.URLError: return False
        await Until(HubReady,'isolated hub did not become ready')
        server=Spawn([ORIGINAL_CODEX,'app-server','--listen','unix://'],'app-server')
        await Until(lambda:(codeHome/'app-server-control/app-server-control.sock').exists(),'App Server socket missing')
        a=NewTui('a'); await a.ready()
        first=await Until(lambda:next((p for p in Peers() if p['session_id']),None),'cxr A remained unbound')
        markerA='AMESH_A_'+uuid.uuid4().hex[:12]; Notify(first['peer_id'],markerA)
        await Until(lambda:markerA in a.text(),'notify A absent from TUI')
        screenA=SaveScreen(a,'new-a')
        b=NewTui('b'); await b.ready()
        second=await Until(lambda:next((p for p in Peers() if p['session_id'] and p['peer_id']!=first['peer_id']),None),'cxr B remained unbound')
        assert first['path']==second['path'] and first['session_id']!=second['session_id']
        markerB='AMESH_B_'+uuid.uuid4().hex[:12]; Notify(second['peer_id'],markerB)
        await Until(lambda:markerB in b.text(),'notify B absent from TUI')
        screenB=SaveScreen(b,'new-b'); screenA=SaveScreen(a,'new-a')
        assert markerA not in screenB and markerB not in screenA
        report.update(first=first,second=second,new_cxr_visible=True,same_cwd_isolated=True,markers={'a':markerA,'b':markerB})
        ws=await Connect(); await Rpc(ws,'initialize',{'clientInfo':{'name':'identity_test','version':'0.1'},'capabilities':{'experimentalApi':True}}); await ws.send(json.dumps({'method':'initialized','params':{}}))
        beforeHot=McpPid()
        hot=NewTui('hot',resume=True); await hot.ready()
        markerH='AMESH_H_'+uuid.uuid4().hex[:12]; Notify(second['peer_id'],markerH)
        await Until(lambda:markerH in hot.text(),'hot cxcr notification absent')
        afterHot=McpPid()
        assert afterHot==beforeHot
        assert markerA not in SaveScreen(hot,'hot-resume')
        report['hot_resume']={'before_mcp_pids':beforeHot,'after_mcp_pids':afterHot,'thread_id':second['session_id']}
        await ws.close()
        for terminal in terminals: terminal.close()
        terminals.clear()
        os.killpg(server.pid,signal.SIGTERM)
        try: server.wait(timeout=5)
        except subprocess.TimeoutExpired: os.killpg(server.pid,signal.SIGKILL); server.wait(timeout=3)
        server=Spawn([ORIGINAL_CODEX,'app-server','--listen','unix://'],'app-server-cold')
        await Until(lambda:(codeHome/'app-server-control/app-server-control.sock').exists(),'cold socket missing')
        cold=NewTui('cold',resume=True); await cold.ready()
        coldPeer=await Until(lambda:next((p for p in Peers() if p['session_id']==second['session_id'] and p['status']=='online'),None),'cold cxcr identity changed')
        markerC='AMESH_C_'+uuid.uuid4().hex[:12]; Notify(coldPeer['peer_id'],markerC)
        await Until(lambda:markerC in cold.text(),'cold cxcr notification absent')
        assert markerA not in SaveScreen(cold,'cold-resume')
        assert McpPid() and not set(McpPid()) & set(beforeHot)
        report['cold_resume']={'new_mcp_pids':McpPid(),'thread_id':coldPeer['session_id']}
        report['markers'].update(hot=markerH,cold=markerC)
        report['ok']=True
    except BaseException as error:
        report['ok']=False; report['error']=repr(error)
        report['process_exits']={str(p.pid):p.poll() for p in processes}
        try:
            diagnostic=await Connect()
            await Rpc(diagnostic,'initialize',{'clientInfo':{'name':'identity_diagnostic','version':'0.1'},'capabilities':{'experimentalApi':True}})
            loaded=await Rpc(diagnostic,'thread/loaded/list',{})
            report['diagnostic']={'loaded':loaded,'peers':Peers(),'calls':[]}
            for thread in loaded['data']:
                try: result=await Rpc(diagnostic,'mcpServer/tool/call',{'threadId':thread,'server':'amesh','tool':'amesh_whoami','arguments':{}})
                except Exception as callError: result=repr(callError)
                report['diagnostic']['calls'].append({'thread':thread,'result':result})
            await diagnostic.close()
        except Exception as diagnosticError:
            report['diagnostic_error']=repr(diagnosticError)
        for index,terminal in enumerate(terminals):
            terminal.read(); (root/('failure-'+str(index)+'.screen.txt')).write_text(terminal.text())
        raise
    finally:
        (root/'report.json').write_text(json.dumps(report,indent=2)); print(json.dumps(report,indent=2),flush=True)
        for terminal in terminals: terminal.close()
        for process in reversed(processes):
            if process.poll() is None:
                try: os.killpg(process.pid,signal.SIGTERM)
                except ProcessLookupError: pass
        for process in processes:
            try: process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid,signal.SIGKILL); process.wait(timeout=3)

if __name__=='__main__':
    parser=argparse.ArgumentParser(); parser.add_argument('--amesh',default=str(pathlib.Path('build/debug/amesh').resolve())); parser.add_argument('--model-server')
    args=parser.parse_args()
    if args.model_server: ModelServer(args.model_server)
    else: asyncio.run(Run(args.amesh))
