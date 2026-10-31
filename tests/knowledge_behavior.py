"""Opt-in real Claude knowledge retrieval probes against an isolated hub."""
import argparse
import hashlib
import json
import os
import pathlib
import re
import signal
import socket
import sqlite3
import subprocess
import time
import urllib.request


def RunCase(binary, root, case):
    root.mkdir(parents=True, exist_ok=False)
    home = root / 'home'
    home.mkdir()
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(('AMESH_', 'CODEX_', 'CLAUDE_CODE_'))}
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        bind = '127.0.0.1:' + str(listener.getsockname()[1])
    hubEnv = dict(env, HOME=str(home), CODEX_HOME=str(home / '.codex'),
                  AMESH_BIND=bind, AMESH_STATE=str(root / 'state.db'),
                  AMESH_TOKEN='isolated-knowledge', AMESH_CIRCLE='eval',
                  AMESH_BACKEND='claude-code', AMESH_PEER_ID='eval-coordinator')
    http = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def Request(path, body=None):
        request = urllib.request.Request('http://' + bind + path,
            data=json.dumps(body).encode() if body is not None else None,
            headers={'Authorization': 'Bearer isolated-knowledge', 'Content-Type': 'application/json'})
        return json.load(http.open(request, timeout=3))

    with (root / 'hub.log').open('w') as log:
        hub = subprocess.Popen([binary, 'serve'], env=hubEnv, cwd=root,
                               stdout=log, stderr=log, start_new_session=True)
    agent = None
    try:
        for _ in range(100):
            assert hub.poll() is None, 'hub exited'
            try:
                if Request('/health')['ok']:
                    break
            except OSError:
                time.sleep(.05)
        else:
            raise AssertionError('hub startup timeout')
        Request('/peers', {'name': 'reviewer', 'peer_id': 'reviewer', 'backend': 'pi',
                          'circle': 'eval', 'session_id': 'reviewer-session'})
        hook = subprocess.run([binary, 'hook', 'session', '--backend', 'claude-code'],
            input=json.dumps({'session_id': 'behavior', 'cwd': str(root)}), env=hubEnv,
            cwd=root, text=True, capture_output=True, check=True, timeout=15)
        primer = json.loads(hook.stdout)['hookSpecificOutput']['additionalContext']
        (root / 'primer.txt').write_text(primer)
        secret = 'ORCHID_GATE_72'
        if case in ('hit', 'expired'):
            entry = Request('/knowledge/publish', {'from_peer': 'reviewer',
                'title': 'KX741 recovery procedure',
                'body': 'For KX741, set recovery_marker=' + secret + ' and retry once.',
                'applies_to': 'KX741 in the controlled integration fixture, revision fixture-1',
                'evidence': 'Controlled fixture probe fixture-1: marker accepted and retry succeeded.'})['knowledge']
            if case == 'expired':
                with sqlite3.connect(root / 'state.db') as db:
                    db.execute('UPDATE knowledge SET created_at=?, expires_at=? WHERE id=?',
                               (int(time.time()) - 200, int(time.time()) - 100, entry['id']))
        prompt = ('本次是受控测试环境 fixture-1，执行器报 KX741。请给出有依据的处置方案，'
                  '需要协作时可问 reviewer，发出问题后结束，不等待回复。')
        if case == 'expired':
            prompt += '上次处理时留下的记录 ID 是 ' + entry['id'] + '。'
        if case == 'publish':
            prompt = ('这项受控 fixture-1 排障任务已完成：错误 KX741 通过设置 recovery_marker='
                      + secret + ' 后重试一次消失。验证证据：fixture-1-run.log 中首次退出码 74，'
                      '设置后退出码 0；适用范围仅 fixture-1。这是用户提供的受控测试结果，'
                      '日志文件未挂载到本会话；保存时注明证据来源。请完成协作收尾。')
        if case == 'skip':
            prompt = '请把“测试完成”翻译成英文。'
        prompt += '本会话仅提供 amesh 工具，无需执行本地检查或修复命令。'
        mcp = {'mcpServers': {'amesh': {'command': binary,
            'args': ['mcp', '--peer-id', 'eval-coordinator'],
            'env': {k: v for k, v in hubEnv.items()
                    if k.startswith('AMESH_') or k in ('HOME', 'CODEX_HOME')}}}}
        command = ['claude', '-p', prompt, '--setting-sources', '',
            '--settings', '{"disableAllHooks":true}', '--disable-slash-commands',
            '--no-session-persistence', '--strict-mcp-config', '--mcp-config', json.dumps(mcp),
            '--tools', '', '--allowedTools', 'mcp__amesh__*', '--append-system-prompt', primer,
            '--output-format', 'stream-json', '--verbose']
        (root / 'prompt.txt').write_text(prompt)
        with (root / 'agent.jsonl').open('w') as out, (root / 'agent.stderr').open('w') as err:
            agent = subprocess.Popen(command, env=env, cwd=root, stdout=out, stderr=err,
                                     start_new_session=True)
            agent.wait(timeout=240)
        events = [json.loads(line) for line in (root / 'agent.jsonl').read_text().splitlines() if line.strip()]
        calls = [block for event in events if event.get('type') == 'assistant'
                 for block in event['message']['content'] if block.get('type') == 'tool_use']
        results = [event for event in events if event.get('type') == 'result']
        errors = [block for event in events if event.get('type') == 'user'
            for block in event.get('message', {}).get('content', [])
            if block.get('type') == 'tool_result' and block.get('is_error')]
        names = [call['name'].rsplit('__', 1)[-1] for call in calls]
        final = results[-1].get('result', '') if results else ''
        report = {'case': case, 'exit': agent.returncode, 'calls': calls, 'results': results,
                  'tool_errors': errors, 'primer_sha256': hashlib.sha256(primer.encode()).hexdigest(),
                  'binary_sha256': hashlib.sha256(pathlib.Path(binary).read_bytes()).hexdigest()}
        (root / 'result.json').write_text(json.dumps(report, indent=2))
        assert agent.returncode == 0 and results and not results[-1].get('is_error'), 'agent failed'
        assert not errors, errors
        assert all(call['name'].startswith('mcp__amesh__') for call in calls), names
        if case == 'skip':
            assert 'amesh_knowledge_publish' not in names, names
            assert final.strip(), 'missing translation'
            print(case, names, flush=True)
            return
        if case == 'publish':
            assert names.count('amesh_knowledge_publish') == 1, names
            entries = Request('/knowledge/search', {'from_peer': 'reviewer', 'query': 'KX741'})['items']
            assert len(entries) == 1, entries
            record = Request('/knowledge/get', {'from_peer': 'reviewer', 'id': entries[0]['id']})['knowledge']
            assert secret in record['body'] and 'fixture-1' in record['applies_to'], record
            assert 'fixture-1-run.log' in record['evidence'], record
            assert re.search(r'用户|\buser\b', record['evidence'], re.I), record
            print(case, names, flush=True)
            return
        assert 'amesh_knowledge_search' in names, names
        assert 'amesh_knowledge_publish' not in names, 'diagnosis has not been independently executed'
        if case == 'hit':
            assert 'amesh_knowledge_get' in names and secret in final, (names, final)
            assert names.index('amesh_knowledge_search') < names.index('amesh_knowledge_get'), names
            assert not set(names) & {'amesh_ask', 'amesh_ask_many', 'amesh_broadcast', 'amesh_notify_peer'}, names
        else:
            assert set(names) & {'amesh_ask', 'amesh_ask_many'}, names
            if case == 'miss':
                asks = [call for call in calls if call['name'].rsplit('__', 1)[-1] in ('amesh_ask', 'amesh_ask_many')]
                question = json.dumps(asks[0]['input'], ensure_ascii=False)
                assert 'KX741' in question, question
                assert re.search(r'检查|查看|收集|定位|对照|核对|检索|复现|确认|check|inspect|collect|compare|trace|verify|reproduc', question, re.I), question
                assert names.index('amesh_knowledge_search') < names.index(asks[0]['name'].rsplit('__', 1)[-1]), names
            if case == 'expired':
                assert 'amesh_knowledge_get' in names, names
                assert '过期' in final or 'expired' in final.lower(), final
        print(case, names, flush=True)
    finally:
        for process in (agent, hub):
            if process is not None and process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True)
    parser.add_argument('--output', required=True)
    parser.add_argument('--case', choices=['hit', 'miss', 'expired', 'publish', 'skip', 'all'], default='all')
    args = parser.parse_args()
    for case in ['hit', 'miss', 'expired', 'publish', 'skip'] if args.case == 'all' else [args.case]:
        RunCase(str(pathlib.Path(args.binary).resolve()), pathlib.Path(args.output).resolve() / case, case)
