#!/usr/bin/env python3
"""Exercise real headless failure/restart boundaries in an isolated retained directory."""
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

from evidence import query


def main():
    root = Path(__file__).resolve().parents[1]
    (root / '.scratch').mkdir(exist_ok=True)
    work = Path(tempfile.mkdtemp(prefix='observability-', dir=root / '.scratch'))
    save = work / 'v.json'
    binary = root / 'target/debug/society'
    process = None
    output = (work / 'process.log').open('ab')
    url = ''

    def request(path, body=None, raw=None):
        data = raw if raw is not None else (None if body is None else json.dumps(body).encode())
        req = urllib.request.Request(url + path, data=data, headers={'content-type': 'application/json'})
        try:
            response = urllib.request.urlopen(req, timeout=10)
        except urllib.error.HTTPError as err:
            response = err
        with response:
            raw_body = response.read()
            try:
                body = json.loads(raw_body)
            except ValueError:
                body = {'text': raw_body.decode('utf-8', errors='replace')}
            return response.status, body, response.headers.get('x-society-operation-id')

    def start(attempts=3):
        nonlocal process, url
        if not binary.exists():
            raise SystemExit(f'missing {binary}; run: cargo build -p society --bin society')
        for attempt in range(attempts):
            # Probe then spawn: another process can claim the port in that gap,
            # so a child that dies on bind is retried with a fresh port instead
            # of failing the whole exercise.
            with socket.socket() as probe:
                probe.bind(('127.0.0.1', 0))
                port = probe.getsockname()[1]
            url = f'http://127.0.0.1:{port}'
            process = subprocess.Popen([str(binary), '--headless', '--paused', '--port', str(port)],
                                       cwd=root, env={**os.environ, 'SOCIETY_SAVE_PATH': str(save)},
                                       stdout=output, stderr=output)
            for _ in range(200):
                if process.poll() is not None:
                    break
                try:
                    return request('/health')[1]
                except (OSError, ValueError):
                    time.sleep(0.05)
            else:
                raise AssertionError('server did not become ready')
            if process.poll() is None:
                process.kill()
                process.wait(timeout=10)
            if attempt + 1 == attempts:
                raise AssertionError(f'process exited: {work / "process.log"}')
        raise AssertionError('unreachable')

    def stop(sig=signal.SIGINT):
        if process and process.poll() is None:
            process.send_signal(sig)
            process.wait(timeout=10)

    try:
        initial = start()
        world = initial['telemetry']['context']['world_id']
        session = initial['telemetry']['context']['session_id']
        assert request('/inject', raw=b'{broken')[0] == 400
        status, _, operation = request('/turn', {'turns': 3})
        assert status == 200
        records = query(save, journal=True, operation=operation)
        assert len(records['entries']) == 3
        assert len({e['context']['world_id'] for e in records['entries']}) == 1
        assert request('/verify')[1]['status'] == 'baseline_matches'
        # Concurrent retry-like submissions remain distinct operations; no claim of idempotency.
        with ThreadPoolExecutor(max_workers=2) as pool:
            submitted = list(pool.map(lambda _: request('/turn', {'turns': 1}), range(2)))
        assert len({r[2] for r in submitted}) == 2
        assert sorted(r[1]['turn'] for r in submitted) == [4, 5]
        # Disable the journal alone; gameplay/save still succeed.
        journal = Path(str(save) + '.journal.jsonl')
        parked = work / 'parked-journal'
        journal.rename(parked)
        journal.mkdir()
        status, body, missing_journal_op = request('/turn', {'turns': 1})
        assert status == 200 and body['saved']
        assert request('/health')[1]['telemetry']['write_failures'] >= 1
        assert request('/log')[1]['evidence']['read_errors'] > 0
        journal.rmdir()
        parked.rename(journal)
        # Fail only atomic save replacement; retain independent journal and state outcome.
        previous = save.read_bytes()
        save.unlink()
        save.mkdir()
        status, body, failed_save_op = request('/turn', {'turns': 1})
        assert status == 500 and body['committed'] == 1
        failed = query(save, operation=failed_save_op)['entries']
        outcome = next(e for e in failed if e['action'] == 'turn_completed')
        assert outcome['detail']['saved'] is False and outcome['detail']['journaled'] is True
        save.rmdir()
        save.write_bytes(previous)
        assert request('/turn', {'turns': 1})[0] == 200
        assert any(e['action'] == 'state_saved' and e['detail']['recovered'] for e in query(save)['entries'])
        # Retained evidence must survive a process death, including a torn audit tail.
        stop(signal.SIGKILL)
        audit = Path(str(save) + '.diagnostics.jsonl')
        with audit.open('ab') as stream:
            stream.write(b'{torn')
        resumed = start()
        assert resumed['turn'] == 8
        assert resumed['telemetry']['context']['world_id'] == world
        assert resumed['telemetry']['context']['session_id'] != session
        assert query(save, operation=failed_save_op)['entries']
        assert query(save)['corrupt_lines'] == 1
        assert request('/verify')[1]['status'] == 'baseline_diverged'  # missing journal turn 5
        # Same-seed resets keep two distinguishable histories.
        assert request('/reset', {'seed': 7})[0] == 200
        assert request('/turn', {'turns': 1})[0] == 200
        after_reset = request('/health')[1]
        assert after_reset['telemetry']['context']['world_id'] != world
        assert len([e for e in query(save, journal=True)['entries'] if e['turn'] == 0]) == 2
        # Private payloads and unknown paths must not leak into the audit sink.
        secret = 'SENTINEL_PRIVATE_PROSE_40198'
        assert request('/inject', {'kind': 'whisper', 'target': 'Ada', 'text': secret})[0] == 200
        assert request('/inject', {'kind': 'weather', 'target': secret, 'to': secret, 'place': secret})[0] == 200
        assert request('/inject', {'kind': secret})[0] == 400
        assert request('/halt', {'reason': secret})[0] == 200
        assert request('/turn', {'turns': 1})[0] == 409
        assert secret not in audit.read_text()
        evidence = query(save, operation=missing_journal_op)
        assert any(e['action'] == 'turn_completed' and not e['detail']['journaled'] for e in evidence['entries'])
        # The offline reader must classify rows the way the server does: a
        # boolean timestamp is not a u64 and a padded slot is not a segment.
        synthetic = work / 'reader'
        synthetic.mkdir()
        row = {'journal_version': 1, 'build': 'probe', 'save_version': 2, 'seed': 7,
               'turn': 0, 'day': 0, 'wall_time': True, 'context': {}, 'record': {}}
        (synthetic / 'v.json').write_text('{}')
        (synthetic / 'v.json.journal.jsonl').write_text(json.dumps(row) + '\n')
        (synthetic / 'v.json.journal.01.jsonl').write_text(
            json.dumps({**row, 'wall_time': 1794000000, 'turn': 1}) + '\n')
        classified = query(synthetic / 'v.json', journal=True)
        assert classified['entries'] == [], classified
        assert classified['corrupt_lines'] == 1, classified
        assert [Path(f).name for f in classified['files']] == ['v.json.journal.jsonl'], classified
        summary = {'ok': True, 'retained_directory': str(work), 'original_world_id': world,
                   'initial_operation': operation, 'save_failure_operation': failed_save_op,
                   'journal_failure_operation': missing_journal_op, 'checks': [
                       'malformed request', 'request/turn correlation', 'baseline replay', 'concurrent operations',
                       'journal failure isolation', 'save failure and recovery', 'SIGKILL/restart identity',
                       'torn tail recovery', 'missing-turn divergence', 'same-seed reset isolation', 'audit redaction',
                       'reader classification']}
        (work / 'result.json').write_text(json.dumps(summary, indent=2) + '\n')
        print(json.dumps(summary, indent=2))
    finally:
        stop()
        output.close()


if __name__ == '__main__':
    main()
