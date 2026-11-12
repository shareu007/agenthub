#!/usr/bin/env python3
"""Check provenance-linked native fixture hashes and basic stored structure."""
import hashlib
import json
from pathlib import Path
import sqlite3

ROOT = Path(__file__).resolve().parents[1]
matrix = json.loads((ROOT/'compatibility/agents.json').read_text())
if set(matrix['agents']) != {'agy', 'claude', 'codex', 'opencode', 'pi'}:
    raise RuntimeError('Compatibility matrix must enumerate all five agents')
for agent, entry in matrix['agents'].items():
    if entry['fixture']:
        path = ROOT/entry['fixture']
        actual = hashlib.sha256(path.read_bytes()).hexdigest()
        if actual != entry['fixture_sha256']:
            raise RuntimeError(f'{agent}: fixture hash differs from recorded provenance')
        print(f'{agent}: native fixture hash verified ({entry["version"]})')
    else:
        print(f'{agent}: native fixture NOT AVAILABLE; status={entry["status"]}')
rows = [json.loads(line) for line in (ROOT/matrix['agents']['pi']['fixture']).read_text().splitlines()]
if rows[0]['version'] != 3 or len([r for r in rows if r['type'] == 'message']) != 2:
    raise RuntimeError('Pi native fixture lost its v3 header or two messages')
with sqlite3.connect(':memory:') as db:
    db.executescript((ROOT/matrix['agents']['opencode']['fixture']).read_text())
    if list(db.execute('PRAGMA foreign_key_check')) or db.execute('PRAGMA integrity_check').fetchone()[0] != 'ok':
        raise RuntimeError('OpenCode native fixture violates database integrity')
    if db.execute('SELECT count(*) FROM session').fetchone()[0] != 1 or db.execute('SELECT count(*) FROM message').fetchone()[0] != 1:
        raise RuntimeError('OpenCode controlled fixture should contain one session and one message')
print('Available native fixtures verified. This does not certify missing fixtures or native recovery.')
