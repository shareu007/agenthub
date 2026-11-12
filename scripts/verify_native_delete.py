#!/usr/bin/env python3
"""Optional OpenCode 1.19.5 native deletion smoke test on isolated fixture data.
Creates its own temporary native store; never calls a model or touches user stores.
Requires the pinned opencode executable on PATH. Not part of ordinary Rust CI.
"""
from pathlib import Path
import tempfile, subprocess, sqlite3, json, os, argparse
repo=Path(__file__).resolve().parents[1]
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--binary', type=Path, required=True)
args=parser.parse_args()
binary=args.binary.resolve()
with tempfile.TemporaryDirectory(prefix='ah-native-delete-') as tmp:
    root=Path(tmp)
    env=dict(os.environ, XDG_DATA_HOME=str(root/'data'), XDG_CONFIG_HOME=str(root/'config'),
        XDG_CACHE_HOME=str(root/'cache'), XDG_STATE_HOME=str(root/'state'))
    version=subprocess.run(['opencode','--version'],env=env,check=True,timeout=60,capture_output=True,text=True).stdout.strip()
    if version != '1.19.5':
        raise RuntimeError(f'Native deletion probe is pinned to OpenCode 1.19.5, found {version!r}')
    data=root/'data/opencode';data.mkdir(parents=True,exist_ok=True)
    # Let the native CLI initialize its complete migrations, then copy only the
    # controlled fixture session/message/part rows into that fresh native store.
    subprocess.run(['opencode','session','list','--pure'],env=env,check=True,timeout=60,capture_output=True)
    native=data/'opencode.db'
    with sqlite3.connect(':memory:') as fixture, sqlite3.connect(native) as db:
        fixture.executescript((repo/'tests/fixtures/opencode/1.19.5.sql').read_text())
        for table in ['project','session','message','part']:
            cols=[r[1] for r in fixture.execute(f'PRAGMA table_info({table})')]
            for row in fixture.execute(f'SELECT * FROM {table}'):
                db.execute(f'INSERT OR REPLACE INTO {table}({",".join(cols)}) VALUES({",".join("?" for _ in cols)})',row)
    cfg=root/'hub.toml'
    cfg.write_text('database = '+json.dumps(str(root/'index.db'))+'\n[sources]\n'+''.join(
        a+' = '+json.dumps(str(data if a=='opencode' else root/a))+'\n' for a in ['claude','codex','opencode','pi','agy']))
    def run(*args):
        return subprocess.run([str(binary),'--config',str(cfg),*args],env=env,check=True,timeout=60,capture_output=True,text=True)
    report=json.loads(run('sync','--agent','opencode','--json').stdout)
    key='opencode:ses_e326ec748ffePgLiZVEam9royd'
    assert report['opencode']['indexed']==1
    assert json.loads(run('delete',key,'--json').stdout)['dry_run']
    assert json.loads(run('delete',key,'--yes','--json').stdout)['deleted']==key
    with sqlite3.connect(native) as db:
        for table in ['session','message','part']:
            assert db.execute(f'SELECT count(*) FROM {table}').fetchone()[0]==0,table
        assert db.execute('PRAGMA integrity_check').fetchone()[0]=='ok'
    assert json.loads(run('list','--json','--include-missing').stdout)==[]
    print('OpenCode 1.19.5 official session delete: verified session/message/part removal, DB integrity, and index cleanup; no model call')
