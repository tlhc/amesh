"""Opt-in real Claude dispatch evaluation against isolated hubs; incurs model usage."""
import argparse
import concurrent.futures
import hashlib
import json
import os
import pathlib
import signal
import socket
import subprocess
import time
import urllib.request


def CheckReport(report):
    calls = report['calls']
    dispatch = [call for call in calls if call['name'].rsplit('__', 1)[-1] in
        ('amesh_ask', 'amesh_ask_many', 'amesh_notify_peer', 'amesh_job_create', 'amesh_job_update', 'amesh_ack')]
    names = [call['name'].rsplit('__', 1)[-1] for call in dispatch]
    case = report['case']
    expected = {'single-review': ['amesh_job_create'], 'multi-review': ['amesh_job_create'] * 2,
        'question': ['amesh_ask'], 'notify': ['amesh_notify_peer'], 'cross-circle': ['amesh_ask'],
        'self-ack': ['amesh_ack'], 'requeue': ['amesh_job_update']}
    assert names == expected[case], (case, names)
    assert report['exit'] == 0 and report['results'] and not report['results'][-1].get('is_error')
    assert not report.get('tool_errors'), report.get('tool_errors')
    if case in ('single-review', 'multi-review'):
        peers = ['reviewer-a'] if case == 'single-review' else ['reviewer-a', 'reviewer-b']
        assert sorted(job['assigned_peer'] for job in report['jobs']) == peers
        assert all(job['dispatch'] and job['state'] in ('queued', 'running') for job in report['jobs'])
    elif case == 'cross-circle':
        assert dispatch[0]['input']['cross_circle'] is True and not report['jobs']
    elif case == 'self-ack':
        assert len(report['jobs']) == 1 and report['jobs'][0]['state'] == 'done'
        assert report['jobs'][0]['result_summary'] == dispatch[0]['input']['message']
    elif case == 'requeue':
        assert dispatch[0]['input']['state'] == 'queued'
        assert len(report['jobs']) == 1 and report['jobs'][0]['assigned_peer'] == 'reviewer-a'
        assert report['jobs'][0]['prompt'] == dispatch[0]['input']['prompt']
    else:
        assert not report['jobs']


def RunCase(binary, primerBinary, root, case):
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
                  AMESH_TOKEN='isolated-behavior', AMESH_CIRCLE='eval',
                  AMESH_BACKEND='claude-code', AMESH_PEER_ID='eval-coordinator')
    http = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def Request(path, body=None):
        request = urllib.request.Request('http://' + bind + path,
            data=json.dumps(body).encode() if body is not None else None,
            headers={'Authorization': 'Bearer isolated-behavior', 'Content-Type': 'application/json'})
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
        for peer, circle in [('reviewer-a', 'eval'), ('reviewer-b', 'eval'), ('outside', 'other')]:
            Request('/peers', {'name': peer, 'peer_id': peer, 'backend': 'pi', 'circle': circle})
        hook = subprocess.run([primerBinary, 'hook', 'session', '--backend', 'claude-code'],
            input=json.dumps({'session_id': 'behavior', 'cwd': str(root)}), env=hubEnv,
            cwd=root, text=True, capture_output=True, check=True, timeout=15)
        primer = json.loads(hook.stdout)['hookSpecificOutput']['additionalContext']
        (root / 'primer.txt').write_text(primer)
        mcp = {'mcpServers': {'amesh': {'command': binary, 'args': ['mcp', '--peer-id', 'eval-coordinator'],
                                     'env': hubEnv}}}
        # Keep authentication in the parent environment; persist only isolated MCP settings.
        mcp['mcpServers']['amesh']['env'] = {k: v for k, v in hubEnv.items()
            if k.startswith('AMESH_') or k in ('HOME', 'CODEX_HOME')}
        frozen = root / 'frozen.diff'
        frozen.write_bytes(subprocess.check_output(['git', 'diff', '--', 'README.md', 'src/cli/mod.rs', 'tests/support/peer_handoff.rs'], cwd=pathlib.Path(__file__).resolve().parents[1]))
        frozenHash = hashlib.sha256(frozen.read_bytes()).hexdigest()
        prompt = case[1].replace('/tmp/frozen.diff', str(frozen)).replace('a' * 64, frozenHash) + '\n请实际完成派发，派发后结束，不等待接收方回复。'
        if case[0] in ('self-ack', 'requeue'):
            holder = 'eval-coordinator' if case[0] == 'self-ack' else 'reviewer-a'
            job = Request('/jobs', {'title': 'Review summary', 'prompt': 'Summarize the review.',
                'from_peer': 'eval-coordinator', 'assigned_peer': holder})
            for _ in range(100):
                job = Request('/jobs/' + job['job_id'])
                if job['ask_id']:
                    break
                time.sleep(.1)
            assert job['ask_id'], 'job did not dispatch'
            (root / 'before.json').write_text(json.dumps(job, indent=2))
            if case[0] == 'self-ack':
                prompt = ('你创建并指派给自己的汇总工作已经完成。实际结果：两份报告均确认无阻塞问题，'
                    '唯一限制是尚未执行性能测试。请向 hub 交付这个结果并结束。\n'
                    '<peer-message type="ask" from="eval-coordinator" to="eval-coordinator" correlation-id="'
                    + job['ask_id'] + '">job ' + job['job_id'] + ': Summarize the review.</peer-message>')
            else:
                prompt = ('你是协调者。工作 ' + job['job_id'] + ' 正由 reviewer-a 处理，状态 running，ask_id='
                    + job['ask_id'] + '。现在把验收范围改为：只审查 README 中的工具选择规则，'
                    '交付带行号和证据的报告；保留原接收方。请落实这项修改，完成操作后结束，无需等待结果。')
        (root / 'prompt.txt').write_text(prompt)
        command = ['claude', '-p', prompt, '--setting-sources', '', '--settings', '{"disableAllHooks":true}',
            '--disable-slash-commands', '--strict-mcp-config',
            '--mcp-config', json.dumps(mcp), '--tools', '', '--allowedTools', 'mcp__amesh__*',
            '--append-system-prompt', primer, '--output-format', 'stream-json', '--verbose']
        with (root / 'agent.jsonl').open('w') as out, (root / 'agent.stderr').open('w') as err:
            agent = subprocess.Popen(command, env=env, cwd=root, stdout=out, stderr=err,
                                     start_new_session=True)
            agent.wait(timeout=240)
        events = [json.loads(line) for line in (root / 'agent.jsonl').read_text().splitlines() if line.strip()]
        calls = [block for event in events if event.get('type') == 'assistant'
                 for block in event['message']['content'] if block.get('type') == 'tool_use']
        results = [event for event in events if event.get('type') == 'result']
        report = {'case': case[0], 'exit': agent.returncode, 'calls': calls,
                  'results': results, 'jobs': Request('/jobs'),
                  'initialized': [e for e in events if e.get('subtype') == 'init'],
                  'primer_sha256': hashlib.sha256(primer.encode()).hexdigest(),
                  'binary_sha256': hashlib.sha256(pathlib.Path(binary).read_bytes()).hexdigest()}
        report['tool_errors'] = [block for event in events if event.get('type') == 'user'
            for block in event.get('message', {}).get('content', [])
            if block.get('type') == 'tool_result' and block.get('is_error')]
        (root / 'result.json').write_text(json.dumps(report, indent=2))
        assert agent.returncode == 0 and results and not results[-1].get('is_error'), 'agent failed'
        assert calls and not report['tool_errors'], 'tools unavailable or failed'
        if root.name.startswith('new-'):
            CheckReport(report)
        print(root.name, [call['name'] for call in calls], flush=True)
        return report
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
    parser.add_argument('--old-binary', required=True)
    parser.add_argument('--output', required=True)
    parser.add_argument('--pilot', action='store_true')
    parser.add_argument('--lifecycle', action='store_true')
    args = parser.parse_args()
    cases = [
        ('single-review', '请让 reviewer-a 独立审查冻结输入 /tmp/frozen.diff（sha256: ' + 'a' * 64 + '）。只读，交付含严重度、文件行号和验证证据的报告；验收为每项结论有证据，无问题则明确说明。'),
        ('multi-review', '请让 reviewer-a 和 reviewer-b 各自独立审查同一份冻结输入 /tmp/frozen.diff（sha256: ' + 'a' * 64 + '），分别交付含严重度、文件行号和验证证据的报告；只读，每人的报告独立验收。'),
        ('question', '问 reviewer-a：你刚才报告里的第 2 项是阻塞上线还是建议优化？只需简短裁决。'),
        ('notify', '告诉 reviewer-a：冻结输入的说明补充在 /tmp/context.txt。只作通知，无需回复。'),
        ('cross-circle', '请让 other circle 的 outside 独立审查冻结输入 /tmp/frozen.diff，交付含文件行号和证据的只读报告；每个结论有证据才验收。'),
    ]
    runs = [('new', args.binary)] if args.pilot else [('old', args.old_binary), ('new', args.binary)]
    selected = cases[:1] if args.pilot else cases
    if args.lifecycle:
        runs = [('new', args.binary)]
        selected = [('self-ack', ''), ('requeue', '')]
    output = pathlib.Path(args.output).resolve()
    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
        futures = [pool.submit(RunCase, str(pathlib.Path(args.binary).resolve()),
                    str(pathlib.Path(primer).resolve()), output / (version + '-' + case[0]), case)
                   for version, primer in runs for case in selected]
        reports = [future.result() for future in futures]
    (output / 'results.json').write_text(json.dumps(reports, indent=2))
