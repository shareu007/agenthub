#!/usr/bin/env python3
"""Record the real CLI with disposable sample sessions; requires Pillow and pyte.
No real agent is launched, and no user data or configuration is read.
"""
import argparse
from datetime import datetime, timezone
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
import tempfile
import termios
import time

from PIL import Image, ImageDraw, ImageFont
import pyte

ROOT = Path(__file__).resolve().parents[1]
COLS, ROWS = 110, 24
FONT_PATH = '/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf'
COLORS = dict(black='#161b22', red='#ff7b72', green='#7ee787', brown='#d29922',
              blue='#79c0ff', magenta='#d2a8ff', cyan='#76e3ea', white='#e6edf3',
              brightblack='#8b949e', brightred='#ffa198', brightgreen='#aff5b4',
              brightbrown='#f8e3a1', brightblue='#a5d6ff', brightmagenta='#e2c5ff',
              brightcyan='#a5f3fc', brightwhite='#ffffff', default='#d1d9e0')


def draw(screen, font, bold):
    cw, ch, margin, chrome = 10, 22, 20, 44
    image = Image.new('RGB', (COLS*cw+margin*2, ROWS*ch+margin+chrome), '#0d1117')
    pen = ImageDraw.Draw(image)
    pen.rectangle((0, 0, image.width, chrome-1), fill='#161b22')
    for i, color in enumerate(('#ff5f57', '#febc2e', '#28c840')):
        pen.ellipse((18+i*20, 16, 28+i*20, 26), fill=color)
    pen.text((image.width//2-135, 10), 'AgentHub · sample sessions', font=font, fill='#8b949e')
    for y in range(ROWS):
        for x in range(COLS):
            cell = screen.buffer[y][x]
            fg = COLORS.get(cell.fg, '#'+cell.fg if len(cell.fg) == 6 else '#d1d9e0')
            bg = '#0d1117' if cell.bg == 'default' else COLORS.get(cell.bg, '#'+cell.bg if len(cell.bg) == 6 else '#0d1117')
            if cell.reverse:
                fg, bg = bg, fg
            left, top = margin+x*cw, chrome+y*ch
            if bg != '#0d1117':
                pen.rectangle((left, top, left+cw-1, top+ch-1), fill=bg)
            if cell.data.strip():
                pen.text((left, top+1), cell.data, font=bold if cell.bold else font, fill=fg)
            if cell.underscore:
                pen.line((left, top+ch-3, left+cw-1, top+ch-3), fill=fg)
    if not screen.cursor.hidden:
        x, y = screen.cursor.x, screen.cursor.y
        pen.line((margin+x*cw, chrome+(y+1)*ch-2, margin+(x+1)*cw-1, chrome+(y+1)*ch-2), fill='#d1d9e0', width=2)
    return image


def samples(root, binary):
    projects = {p: root/p for p in ('agenthub', 'web-app', 'mesa')}
    for p in projects.values():
        p.mkdir()
    agents = ['claude', 'codex', 'opencode', 'pi', 'agy']
    sources = {a: root/'sessions'/a for a in agents}
    for p in sources.values():
        p.mkdir(parents=True)
    items = [
        ('claude', 'Fix authentication redirect', 'web-app', 12),
        ('codex', 'Add session transfer preview', 'agenthub', 11),
        ('pi', 'Speed up project search', 'web-app', 9),
        ('claude', 'Add tests for session pagination', 'agenthub', 8),
        ('codex', 'Investigate renderer latency', 'mesa', 7),
        ('pi', 'Improve keyboard navigation', 'agenthub', 6),
        ('claude', 'Review API error messages', 'web-app', 5),
        ('codex', 'Simplify release installation', 'agenthub', 4),
        ('pi', 'Document session search', 'agenthub', 3),
    ]
    for i, (agent, title, project, hour) in enumerate(items):
        stamp = f'2026-11-12T{hour:02}:00:00Z'
        sid = f'demo-{agent}-{i+1:02}'
        cwd = str(projects[project])
        message = title + '. ' + 'Sample conversation for the AgentHub demo. '*(10+i*12)
        if agent == 'claude':
            rows = [dict(type='user', sessionId=sid, cwd=cwd, timestamp=stamp,
                         message=dict(role='user', content=message))]
            path = sources[agent]/f'{sid}.jsonl'
        elif agent == 'codex':
            rows = [dict(type='session_meta', timestamp=stamp, payload=dict(id=sid, cwd=cwd)),
                    dict(type='response_item', timestamp=stamp, payload=dict(type='message', role='user',
                         content=[dict(type='input_text', text=message)]))]
            path = sources[agent]/'sessions'/f'{sid}.jsonl'
        else:
            rows = [dict(type='session', version=3, id=sid, cwd=cwd, timestamp=stamp),
                    dict(type='message', id=f'm{i}', parentId=None, timestamp=stamp,
                         message=dict(role='user', content=message))]
            path = sources[agent]/f'{sid}.jsonl'
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(''.join(json.dumps(r)+'\n' for r in rows))
    with sqlite3.connect(sources['opencode']/'opencode.db') as db:
        db.executescript((ROOT/'tests/fixtures/opencode/1.19.5.sql').read_text())
        db.execute('UPDATE session SET title=?,directory=?,time_updated=?',
                   ('Trace session import failures', str(projects['agenthub']),
                    int(datetime(2026, 11, 12, 10, tzinfo=timezone.utc).timestamp()*1000)))
    summary, conversation = (ROOT/'tests/fixtures/agy/1.5.6.sql').read_text().split(
        '-- CONVERSATION_DB: 93748d3b-ae91-447e-a184-c16dc21090c9.db\n', 1)
    (sources['agy']/'conversations').mkdir()
    with sqlite3.connect(sources['agy']/'conversation_summaries.db') as db:
        db.executescript(summary)
        db.execute('UPDATE conversation_summaries SET title=?,workspace_uris=?,last_modified_time=?',
                   ('Improve GPU buffer synchronization', json.dumps([projects['mesa'].as_uri()]), '2026-11-12T10:35:00Z'))
    with sqlite3.connect(sources['agy']/'conversations/93748d3b-ae91-447e-a184-c16dc21090c9.db') as db:
        db.executescript(conversation)
    config = root/'config.toml'
    config.write_text('database = '+json.dumps(str(root/'index.db'))+'\n[sources]\n'+
                      ''.join(f'{a} = {json.dumps(str(p))}\n' for a, p in sources.items()))
    subprocess.run([str(binary), '--config', str(config), '--db', str(root/'index.db'), 'sync', '--include-unavailable'],
                   check=True, capture_output=True)
    for i, (agent, title, _, _) in enumerate(items):
        subprocess.run([str(binary), '--config', str(config), '--db', str(root/'index.db'), 'rename', f'{agent}:demo-{agent}-{i+1:02}', title],
                       check=True, capture_output=True)
    # Install-detection stubs only: recording never launches a client or model.
    bindir = root/'bin'
    bindir.mkdir()
    for agent in agents:
        p = bindir/agent
        p.write_text('#!/bin/sh\nexit 1\n')
        p.chmod(0o755)
    return config, bindir


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--font', type=Path, default=Path(FONT_PATH))
    parser.add_argument('--output', type=Path, default=ROOT/'docs/assets/demo.gif')
    args = parser.parse_args()
    binary = args.binary.resolve()
    font = ImageFont.truetype(str(args.font), 16)
    bold_path = args.font.with_name(args.font.stem+'-Bold'+args.font.suffix)
    bold = ImageFont.truetype(str(bold_path if bold_path.exists() else args.font), 16)
    screen = pyte.Screen(COLS, ROWS)
    stream = pyte.Stream(screen)
    frames, durations = [], []
    with tempfile.TemporaryDirectory(prefix='agenthub-demo-') as tmp:
        config, bindir = samples(Path(tmp), binary)
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', ROWS, COLS, 0, 0))
        def child():
            os.setsid()
            fcntl.ioctl(0, termios.TIOCSCTTY, 0)
        env = dict(os.environ, PATH=str(bindir)+os.pathsep+os.environ['PATH'], TERM='xterm-256color')
        env.pop('NO_COLOR', None)
        process = subprocess.Popen([str(binary), '--config', str(config), '--db', str(Path(tmp)/'index.db'), '--color', 'always'],
                                   stdin=slave, stdout=slave, stderr=slave, env=env, preexec_fn=child)
        def read_frame(keys, marker, duration):
            if keys:
                os.write(master, keys)
            data = bytearray()
            deadline = time.monotonic()+5
            while time.monotonic() < deadline:
                if select.select([master], [], [], .05)[0]:
                    data.extend(os.read(master, 65536))
                elif marker in data:
                    break
            if marker not in data:
                raise RuntimeError('Demo screen did not reach expected state')
            stream.feed(data.decode('utf-8'))
            frames.append(draw(screen, font, bold))
            durations.append(duration)
        try:
            read_frame(b'', b'Enter: resume', 1800)
            read_frame(b'jj', b'Enter: resume', 900)
            read_frame(b'3', b'agent=codex', 1500)
            read_frame(b'1/', b'Ctrl-U clear', 500)
            read_frame(b'\x1b[200~session\x1b[201~', b'Search: /session', 1900)
            read_frame(b'\x1b', b'1-6 agent', 1300)
            os.write(master, b'q')
            process.wait(timeout=3)
        finally:
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
            os.close(master)
            os.close(slave)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    frames[0].save(args.output, save_all=True, append_images=frames[1:], duration=durations,
                   loop=0, optimize=True, disposal=2)
    frames[0].save(args.output.with_suffix('.png'))
    print(f'{args.output}: {len(frames)} frames, {args.output.stat().st_size} bytes')


if __name__ == '__main__':
    main()
