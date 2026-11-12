#!/usr/bin/env python3
"""Check provenance-linked native fixture hashes and basic stored structure."""
import hashlib
import json
from pathlib import Path
import sqlite3
import struct

ROOT = Path(__file__).resolve().parents[1]
matrix = json.loads((ROOT/'compatibility/agents.json').read_text())
if set(matrix['agents']) != {'agy', 'claude', 'codex', 'dsh', 'opencode', 'pi'}:
    raise RuntimeError('Compatibility matrix must enumerate all six agents')
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
summary_sql, conversation_sql = (ROOT/matrix['agents']['agy']['fixture']).read_text().split(
    '-- CONVERSATION_DB: 93748d3b-ae91-447e-a184-c16dc21090c9.db\n', 1)
with sqlite3.connect(':memory:') as summary_db, sqlite3.connect(':memory:') as conversation_db:
    summary_db.executescript(summary_sql)
    conversation_db.executescript(conversation_sql)
    if summary_db.execute('PRAGMA integrity_check').fetchone()[0] != 'ok' or conversation_db.execute('PRAGMA integrity_check').fetchone()[0] != 'ok':
        raise RuntimeError('AGY native fixture violates database integrity')
    if summary_db.execute('SELECT count(*) FROM conversation_summaries').fetchone()[0] != 1 or conversation_db.execute('SELECT count(*) FROM steps').fetchone()[0] != 4:
        raise RuntimeError('AGY controlled fixture should contain one summary row and four steps')
dsh_bytes = (ROOT/matrix['agents']['dsh']['fixture']).read_bytes()
offset, frames = 0, 0
while offset < len(dsh_bytes):
    if struct.unpack_from('<I', dsh_bytes, offset)[0] != 0xFD2FB528:
        raise RuntimeError(f'DSH native fixture has invalid Zstandard frame magic at {offset}')
    offset += 4
    desc = dsh_bytes[offset]
    offset += 1
    fcs_flag, single_seg, checksum, dict_flag = desc >> 6, bool(desc & 32), bool(desc & 4), desc & 3
    dict_bytes = 4 if dict_flag == 3 else dict_flag
    fcs_bytes = int(single_seg) if fcs_flag == 0 else (1 << fcs_flag)
    offset += int(not single_seg) + dict_bytes + fcs_bytes
    while True:
        bh = dsh_bytes[offset] | (dsh_bytes[offset + 1] << 8) | (dsh_bytes[offset + 2] << 16)
        offset += 3
        last_block, block_type, block_size = bool(bh & 1), (bh >> 1) & 3, bh >> 3
        offset += 1 if block_type == 1 else block_size
        if last_block:
            break
    if checksum:
        offset += 4
    frames += 1
if offset != len(dsh_bytes) or frames != 9:
    raise RuntimeError(f'DSH native fixture expected 9 complete Zstandard frames, got {frames}')
print('Available native fixtures verified. This does not certify missing fixtures or native recovery.')
