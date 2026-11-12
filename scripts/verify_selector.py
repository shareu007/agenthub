#!/usr/bin/env python3
"""Exercise both CLI names in real PTYs, using an isolated native-process stub.

Verifies keyboard selection/launch, terminal restoration, cancellation, regrouping,
sorting, page changes, and redirected output. No real agent or model is invoked.
"""
import argparse
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import sqlite3
import struct
import subprocess
import sys
import tempfile
import termios
import time


def terminal(command, environment, actions, size=(26, 110)):
    master, slave = pty.openpty()
    original = termios.tcgetattr(slave)
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', *size, 0, 0))

    def child():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)

    process = subprocess.Popen(command, stdin=slave, stdout=slave, stderr=slave,
                               env=environment, preexec_fn=child)
    output = bytearray()
    stage = 0
    stage_start = 0
    try:
        deadline = time.monotonic() + 12
        while time.monotonic() < deadline:
            if select.select([master], [], [], 0.05)[0]:
                try:
                    output.extend(os.read(master, 65536))
                except OSError:
                    break
            if stage < len(actions) and actions[stage][0] in output[stage_start:]:
                stage_start = len(output)
                os.write(master, actions[stage][1])
                stage += 1
            if process.poll() is not None:
                # Drain bytes already written before asserting teardown order.
                while select.select([master], [], [], 0)[0]:
                    output.extend(os.read(master, 65536))
                break
        else:
            raise RuntimeError('Selector did not complete; output tail: ' + bytes(output[-2000:]).decode(errors='replace'))
        assert stage == len(actions), f'Only {stage}/{len(actions)} keyboard actions executed; output={bytes(output[-2000:])!r}'
        assert termios.tcgetattr(slave) == original, 'Terminal attributes were not restored'
        return process.wait(timeout=2), bytes(output)
    finally:
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
        os.close(master)
        os.close(slave)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--bin-dir', required=True, type=Path)
    args = parser.parse_args()
    binaries = [str((args.bin_dir/name).resolve()) for name in ['agenthub', 'ah']]
    with tempfile.TemporaryDirectory(prefix='agenthub-selector-') as tmp:
        root = Path(tmp)
        source = root/'pi'
        source.mkdir()
        project = root/'project'
        project.mkdir()
        for native_id, day in [('a', 1), ('b', 3), ('c', 2)]:
            rows = [{'type': 'session', 'version': 3, 'id': native_id, 'cwd': str(project),
                     'timestamp': f'2026-01-0{day}T00:00:00Z'},
                    {'type': 'message', 'id': 'm-'+native_id, 'parentId': None, 'message': {'role': 'user', 'content': f'fixture {native_id} 渲染'}}]
            (source/f'{native_id}.jsonl').write_text(''.join(json.dumps(r)+'\n' for r in rows))
        config = root/'config.toml'
        config.write_text('database = '+json.dumps(str(root/'index.db'))+'\n[sources]\n'+''.join(
            name+' = '+json.dumps(str(source if name == 'pi' else root/name))+'\n'
            for name in ['claude', 'codex', 'pi', 'opencode', 'agy']))
        bindir = root/'bin'
        bindir.mkdir()
        result_file = root/'launch.json'
        stub = bindir/'pi'
        stub.write_text('#!'+sys.executable+'\nimport os,sys,json,termios\n'
                        'flags=termios.tcgetattr(0)[3]\n'
                        'json.dump({"args":sys.argv[1:],"cwd":os.getcwd(),'
                        '"canonical":bool(flags & termios.ICANON),"echo":bool(flags & termios.ECHO)},'
                        'open(os.environ["AGENTHUB_TEST_RESULT"],"w"))\n'
                        'print("AH_NATIVE_OPENED",flush=True)\nsys.exit(17)\n')
        stub.chmod(0o755)
        environment = dict(os.environ, TERM='xterm-256color',
                           PATH=str(bindir)+os.pathsep+os.environ['PATH'], AGENTHUB_TEST_RESULT=str(result_file))
        subprocess.run([binaries[0], '--config', str(config), 'sync', '--json'], check=True, capture_output=True)
        for binary in binaries:
            base = [binary, '--config', str(config), '--color', 'always']
            code, output = terminal(base, environment, [(b'Enter: resume', b'\x1b[B\r')])
            assert code == 17, (code, output)
            launched = json.loads(result_file.read_text())
            assert launched['args'] == ['--session', str(source/'c.jsonl')], launched
            assert launched['cwd'] == str(project) and launched['canonical'] and launched['echo'], launched
            assert output.index(b'\x1b[?1049l') < output.index(b'AH_NATIVE_OPENED')
            result_file.unlink()
            for quit_key in [b'q', b'\x03', b'\x1b']:
                code, _ = terminal(base, environment, [(b'Enter: resume', quit_key)])
                assert code == 0 and not result_file.exists()
            code, _ = terminal(base+['list', '--limit', '2'], environment, [
                (b'Enter: resume', b'g'), (b'Group: Project', b's'),
                (b'Sort: Created', b'r'), (b'reversed', b'n'),
                (b'offset 2', b'p'), (b'offset 0', b'q')])
            assert code == 0
            code, output = terminal(base+['list', '--no-interactive'], environment, [])
            assert code == 0 and b'?1049h' not in output and b'pi:b' in output
            code, output = terminal(base+['--json'], environment, [])
            assert code == 0 and len(json.loads(output)) == 3 and b'?1049h' not in output
            # Failure must stay in selector, not execute a cached/missing source.
            (source/'b.jsonl').rename(source/'b.hidden')
            code, _ = terminal(base, environment, [(b'Enter: resume', b'\r'),
                                                    (b'Cannot resume:', b'q')])
            assert code == 0 and not result_file.exists()
            (source/'b.hidden').rename(source/'b.jsonl')
            print(Path(binary).name+': selection, exec, terminal restore, quit, grouping, sorting, paging, JSON and source failure passed')
        # Synthetic index rows deliberately fill the first 50 results with AGY.
        # Other Agent tabs and body search must still reach all matching sessions.
        with sqlite3.connect(root/'index.db') as db:
            sid = db.execute('SELECT id FROM sources LIMIT 1').fetchone()[0]
            for agent, count in [('agy', 60), ('claude', 1), ('codex', 1), ('opencode', 1)]:
                for i in range(count):
                    key = f'{agent}:seed{i}'
                    db.execute('INSERT INTO sessions VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)',
                               (key, f'seed{i}', agent, sid, 'Hidden '+agent.title(), str(project), 1, 2,
                                str(root/f'{agent}-synthetic-source'), None, '[]', 1, 1))
                    if agent == 'claude':
                        db.execute('INSERT INTO documents(session_key,ordinal,body) VALUES(?,0,?)',
                                   (key, 'BODY_ONLY_RENDER_TOKEN_1983 渲染猫 LONG_SEARCH_'+'x'*72+'_END'))
        for binary in binaries:
            base = [binary, '--config', str(config), '--color', 'always']
            code, output = terminal(base, environment, [(b'Enter: resume', b'2'),
                                                        (b'agent=codex', b'q')])
            assert code == 0 and b'Hidden Codex' in output
            for label in [b'1 Claude:1', b'2 Codex:1', b'3 OpenCode:1', b'4 Pi:3', b'5 AGY:60']:
                assert label in output, (label, output)
            # Important headings and the selected row receive explicit styles.
            assert b'\x1b[1;93m' in output and b'\x1b[1;30;46m' in output
            code, output = terminal(base, environment, [(b'Enter: resume', b'/'),
                (b'Ctrl-U clear', b'BODY_ONLY_RENDER_TOKEN_1983'),
                (b'1 matches', b'\r'), (b'Search: /BODY_ONLY_RENDER_TOKEN_1983', b'q')])
            assert code == 0 and b'Hidden Claude' in output
            # Enter while editing must apply a query, not open a native program.
            assert not result_file.exists()
            long_query = b'LONG_SEARCH_'+b'x'*72+b'_END'
            code, _ = terminal(base, environment, [(b'Enter: resume', b'/'),
                (b'Ctrl-U clear', long_query), (b'/'+long_query, b'\r'),
                (b'1 matches', b'q')])
            assert code == 0

            code, _ = terminal(base, environment, [(b'Enter: resume', b'/'),
                (b'Ctrl-U clear', '渲染猫'.encode()), (b'1 matches', b'\r'),
                ('Search: /渲染猫'.encode(), b'\x1b'), (b'66 matches', b'q')])
            assert code == 0
            # Cancel an edited search, and ensure its previous result set returns.
            code, _ = terminal(base, environment, [(b'Enter: resume', b'/'),
                (b'Ctrl-U clear', b'no-match-token'), (b'0 matches', b'\x1b'),
                (b'66 matches', b'q')])
            assert code == 0
            # Modal handoff preview/cancel must never create a native file.
            before = set(source.rglob('*.jsonl'))
            code, _ = terminal(base, environment, [(b'Enter: resume', b'4'),
                (b'agent=pi', b't'), (b'SESSION TRANSFER', b'4'),
                (b'confirm transfer', b'\x1b'), (b'Enter: resume', b'q')])
            assert code == 0 and set(source.rglob('*.jsonl')) == before
            print(Path(binary).name+': all five tabs, >50-row counts, global body/Unicode search, search cancellation and transfer preview passed')
        original_sources = {p: p.read_bytes() for p in source.glob('*.jsonl')}
        code, output = terminal([binaries[0], '--config', str(config), '--color', 'always'], environment,
            [(b'Enter: resume', b'4'), (b'agent=pi', b't'),
             (b'SESSION TRANSFER', b'2'), (b'confirm transfer', b'y'),
             (b'Created codex:', b'q')])
        assert code == 0 and len(list((root/'codex').rglob('*.jsonl'))) == 1
        assert all(p.read_bytes() == content for p, content in original_sources.items())
        code, output = terminal([binaries[0], '--config', str(config), '--color', 'never'], environment,
                                [(b'Enter: resume', b'q')])
        assert code == 0 and b'\x1b[1;93m' not in output and b'\x1b[1;30;46m' not in output
        print('Confirmed terminal transfer creates a new Codex session, preserves sources, and no-color is honored')
        db.execute('UPDATE sessions SET title=? WHERE key=?', ('Huge '+chr(0x301)*10000, 'agy:seed0'))
        db.commit()
        code, output = terminal([binaries[0], '--config', str(config), '--color', 'always'], environment,
                                [(b'Enter: resume', b'q')])
        assert code == 0 and len(output) < 20000, 'Combining-character title expanded beyond terminal preview bounds'
        print('Pathological zero-width title remains bounded in terminal output')
        db.close()
        # Also check an empty list on a tiny terminal can be dismissed.
        (root/'index.db').unlink()
        code, _ = terminal([binaries[0], '--config', str(config)], environment, [(b'Enter: resume', b'q')], size=(10, 100))
        assert code == 0
    print('PTY selector checks passed (native process stub, not native-agent recovery).')


if __name__ == '__main__':
    main()
