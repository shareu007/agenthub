#!/usr/bin/env python3
"""Opt-in native recovery checks against disposable, non-sensitive sessions.

This launches real installed agents. It never sends a model prompt. Interactive
agents may update their own store; do not use a production session. No transcript
or environment values are saved. Missing prerequisites are BLOCKED, never PASS.
"""
import argparse
import hashlib
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import pty
import re
import selectors
import select
import shutil
import signal
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]


def native_version(program, expected, environment):
    if not expected:
        raise RuntimeError("No verified version pin; establish official distribution and version first")
    binary = shutil.which(program)
    if not binary:
        raise RuntimeError(f"Native executable not installed: {program}")
    result = subprocess.run([binary, '--version'], capture_output=True, text=True, timeout=15, env=environment)
    version = re.search(r'(?<![\w.])\d+\.\d+\.\d+(?:-[\w.-]+)?', result.stdout)
    if result.returncode or not version or version[0] != expected:
        raise RuntimeError(f"Installed {program} version does not match {expected}")
    return binary


def terminal_probe(command, marker, timeout):
    pid, fd = pty.fork()
    if pid == 0:
        os.execvpe(command[0], command, dict(os.environ, TERM='xterm-256color'))
    raw = bytearray()
    matched = False
    try:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if not select.select([fd], [], [], min(0.2, max(0, deadline-time.monotonic())))[0]:
                continue
            try:
                block = os.read(fd, 65536)
            except OSError:
                break
            if not block:
                break
            raw.extend(block)
            if len(raw) > 4 * 1024 * 1024:
                raise RuntimeError('Native output exceeded 4 MiB; marker not established')
            text = re.sub(rb'\x1b\[[0-?]*[ -/]*[@-~]', b'', raw)
            if marker.encode() in text:
                matched = True
                break
    finally:
        # Stop only the process group created by pty.fork, including native children.
        try:
            os.killpg(pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        for _ in range(20):
            done, _ = os.waitpid(pid, os.WNOHANG)
            if done:
                break
            time.sleep(0.05)
        else:
            try:
                os.killpg(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            os.waitpid(pid, 0)
        os.close(fd)
    if not matched:
        raise RuntimeError('Native UI did not display the expected transcript marker; login/trust/format/startup may block it')
    return {'probe': 'agenthub-resume-exec-terminal', 'transcript_marker_seen': True,
            'output_sha256': hashlib.sha256(raw).hexdigest(), 'stopped_after_load': True}


def codex_loader(binary, plan, native_id, marker, timeout):
    environment = dict(os.environ, **dict(plan['env']))
    process = subprocess.Popen([binary, 'app-server'], env=environment, cwd=plan['cwd'],
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    selector = selectors.DefaultSelector()
    selector.register(process.stdout, selectors.EVENT_READ)
    buffer = bytearray()

    def send(value):
        process.stdin.write((json.dumps(value)+'\n').encode())
        process.stdin.flush()

    def response(identifier):
        deadline = time.monotonic()+timeout
        total = 0
        while time.monotonic() < deadline:
            while b'\n' in buffer:
                line, _, remaining = buffer.partition(b'\n')
                buffer[:] = remaining
                item = json.loads(line)
                if item.get('id') == identifier:
                    return item
            if selector.select(0.1):
                block = os.read(process.stdout.fileno(), 65536)
                if not block:
                    raise RuntimeError('Codex loader exited before confirming the transcript')
                buffer.extend(block)
                total += len(block)
                if total > 4*1024*1024:
                    raise RuntimeError('Codex loader exceeded the evidence output limit')
        raise RuntimeError('Codex loader timed out without confirming the transcript')

    try:
        send({'id': 0, 'method': 'initialize', 'params': {'clientInfo': {'name': 'agenthub-native-check', 'version': '0.1.0'}}})
        if 'result' not in response(0):
            raise RuntimeError('Codex app-server initialization failed')
        send({'method': 'initialized'})
        send({'id': 1, 'method': 'thread/resume', 'params': {'threadId': native_id}})
        result = response(1).get('result', {})
        if result.get('thread', {}).get('id') != native_id or marker not in json.dumps(result, ensure_ascii=False):
            raise RuntimeError('Codex loader did not confirm the same session ID and transcript marker')
        return {'probe': 'native-codex-app-server-thread-resume', 'original_id_confirmed': True,
                'transcript_marker_seen': True, 'model_called': False}
    finally:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        selector.close()


def check(args):
    agent, separator, native_id = args.session.partition(':')
    if not separator or not native_id:
        raise RuntimeError('Use the full agent:native-id session key')
    matrix = json.loads((ROOT/'compatibility/agents.json').read_text())
    entry = matrix['agents'][agent]
    if not entry['version']:
        raise RuntimeError('No verified version pin; establish official distribution and version first')
    command = [str(Path(args.agenthub).resolve()), '--config', str(Path(args.config).resolve())]
    plan_result = subprocess.run(command + ['resume', args.session, '--json'], capture_output=True, text=True, timeout=15)
    if plan_result.returncode:
        raise RuntimeError('AgentHub could not produce a resume plan; check source availability and supported root layout')
    plan = json.loads(plan_result.stdout)
    if plan['program'] != agent:
        raise RuntimeError('Resume plan program differs from selected agent')
    environment = dict(os.environ, **dict(plan['env']))
    binary = native_version(agent, entry['version'], environment)
    if args.probe == 'terminal':
        return dict(terminal_probe(command + ['resume', args.session, '--exec'], args.expect, args.timeout), version=entry['version'])
    if args.probe == 'codex-loader':
        if agent != 'codex':
            raise RuntimeError('codex-loader probe requires a Codex session')
        return dict(codex_loader(binary, plan, native_id, args.expect, args.timeout), version=entry['version'])
    if agent != 'pi':
        raise RuntimeError('RPC probe is currently verified only for Pi; use terminal probe for other agents')
    result = subprocess.run([binary, *plan['args'], '--mode', 'rpc', '--no-extensions', '--no-skills', '--no-prompt-templates'],
                            input='{"id":"state","type":"get_state"}\n{"id":"messages","type":"get_messages"}\n',
                            capture_output=True, text=True, env=environment, cwd=plan['cwd'], timeout=args.timeout)
    replies = {r['id']: r for line in result.stdout.splitlines() if (r := json.loads(line)).get('type') == 'response'}
    state = replies.get('state', {})
    messages = replies.get('messages', {})
    if (result.returncode or not state.get('success') or not messages.get('success')
            or state['data'].get('isStreaming') is not False
            or state['data'].get('isCompacting') is not False
            or state['data'].get('sessionId') != native_id
            or args.expect not in json.dumps(messages['data'], ensure_ascii=False)):
        raise RuntimeError('Native Pi did not confirm the original identity and transcript marker')
    return {'version': entry['version'], 'probe': 'native-pi-rpc-from-agenthub-plan', 'original_id_confirmed': True,
            'transcript_marker_seen': True, 'message_count': state['data']['messageCount'], 'model_called': False}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--agenthub', required=True)
    parser.add_argument('--config', required=True)
    parser.add_argument('--session', required=True)
    parser.add_argument('--expect', required=True, help='Unique non-sensitive marker already in the test transcript')
    parser.add_argument('--probe', choices=['terminal', 'pi-rpc', 'codex-loader'], default='terminal')
    parser.add_argument('--timeout', type=float, default=20)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    record = {'checked_at': datetime.now(timezone.utc).isoformat(), 'session': args.session, 'scope': 'native load only; no model continuation or tool execution'}
    try:
        record.update(check(args), status='passed')
    except (RuntimeError, OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        record.update(status='blocked', reason=str(error))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(record, ensure_ascii=False, indent=2)+'\n')
    print(json.dumps(record, ensure_ascii=False))
    return 0 if record['status'] == 'passed' else 2


if __name__ == '__main__':
    sys.exit(main())
