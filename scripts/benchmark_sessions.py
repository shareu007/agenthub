#!/usr/bin/env python3
"""Manual synthetic benchmark. Uses temporary stores; calls no native agent/model.
Linux /proc RSS is sampled after exec; other platforms report RSS as unavailable.
"""
import argparse
import os,pathlib,json,subprocess,tempfile,time,statistics,fcntl,pty,termios,struct,select,signal
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--binary',type=pathlib.Path,default=pathlib.Path(__file__).resolve().parents[1]/'target/release/ah')
parser.add_argument('--output',type=pathlib.Path)
args=parser.parse_args()
BIN=args.binary.resolve()
results=[]
with tempfile.TemporaryDirectory(prefix='ah-review-bench-') as tmp:
 root=pathlib.Path(tmp);src=root/'pi';src.mkdir();project=root/'project';project.mkdir()
 cfg=root/'config.toml';cfg.write_text('database = '+json.dumps(str(root/'index.db'))+'\n[sources]\n'+''.join(a+' = '+json.dumps(str(src if a=='pi' else root/a))+'\n' for a in ['claude','codex','opencode','pi','agy']))
 def make(path,sid,count,text_size):
  with path.open('w') as f:
   f.write(json.dumps({'type':'session','version':3,'id':sid,'cwd':str(project),'timestamp':'2026-01-01T00:00:00Z'})+'\n')
   for i in range(count):
    content=('renderer 渲染 '+('x'*text_size)) if i!=count-1 else 'unique-final-marker 渲染'
    f.write(json.dumps({'type':'message','id':f'm{i}','parentId':f'm{i-1}' if i else None,'timestamp':'2026-01-02T00:00:00Z','message':{'role':'user' if i%2==0 else 'assistant','content':content}},ensure_ascii=False)+'\n')
 for i in range(1000):make(src/f'{i}.jsonl',str(i),100,120)
 make(src/'large.jsonl','large',20000,1000)
 def run(label,*args):
  with tempfile.TemporaryFile() as out,tempfile.TemporaryFile() as err:
   start=time.perf_counter();p=subprocess.Popen([str(BIN),'--config',str(cfg),*args],stdout=out,stderr=err);rss=0
   while p.poll() is None:
    try:
     if pathlib.Path(f'/proc/{p.pid}/exe').resolve()==BIN.resolve():
      for line in pathlib.Path(f'/proc/{p.pid}/status').read_text().splitlines():
       if line.startswith('VmHWM:'):rss=max(rss,int(line.split()[1]))
    except (OSError,ProcessLookupError):pass
    time.sleep(.002)
   elapsed=(time.perf_counter()-start)*1000
   if p.returncode:err.seek(0);raise RuntimeError(err.read().decode())
   row={'command':label,'ms':round(elapsed,2),'peak_rss_mib':round(rss/1024,2) if rss >= 1024 else None};results.append(row)
 run('initial sync: 1001 sessions / 120000 messages','sync','--include-unavailable','--agent','pi','--json')
 run('unchanged sync','sync','--include-unavailable','--agent','pi','--json')
 run('list 50','list','--json')
 run('list largest 50','list','--sort','size','--group-by','none','--json')
 run('large branch transfer preview all 20000 messages','transfer','pi:large','--to','codex','--dry-run','--json')
 run('large branch transfer preview last 10','transfer','pi:large','--to','codex','--last','10','--dry-run','--json')
 run('write new Codex file last 1000','transfer','pi:large','--to','codex','--last','1000','--json')
 # Measure actual terminal filter response, including both list/count queries and rendering.
 def search(query):
  master,slave=pty.openpty();fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',26,110,0,0))
  def pre():os.setsid();fcntl.ioctl(0,termios.TIOCSCTTY,0)
  p=subprocess.Popen([str(BIN),'--config',str(cfg),'--color','never'],stdin=slave,stdout=slave,stderr=slave,preexec_fn=pre,env=dict(os.environ,TERM='xterm-256color'))
  data=bytearray();stage=0;deadline=time.monotonic()+25;start=None;elapsed=None
  try:
   while time.monotonic()<deadline:
    if select.select([master],[],[],.05)[0]:data.extend(os.read(master,65536))
    if stage==0 and b'Enter: resume' in data:
     data.clear();os.write(master,b'/');stage=1
    if stage==1 and b'Ctrl-U clear' in data:
     data.clear();start=time.perf_counter();os.write(master,query.encode());stage=2
    if stage==2 and ('Search: /'+query).encode() in data:
     elapsed=(time.perf_counter()-start)*1000;os.write(master,b'\x03');stage=3
    if p.poll() is not None:break
   if elapsed is None:raise RuntimeError('PTY search timeout')
   results.append({'command':'interactive filter '+query,'ms':round(elapsed,2)})
  finally:
   if p.poll() is None:os.killpg(p.pid,signal.SIGKILL)
   p.wait();os.close(master);os.close(slave)
 for q in ['renderer','unique-final-marker','渲染']:search(q)
 report={'sessions':1001,'messages':120000,'source_bytes':sum(p.stat().st_size for p in src.glob('*.jsonl')),'index_bytes':sum(p.stat().st_size for p in root.glob('index.db*')),'results':results,'notes':'Single-run local synthetic workload, warm caches; /proc RSS sampled after exec every 2 ms (very short commands may have no meaningful sample); includes native-file transfer but no model calls.'}

 if args.output:args.output.write_text(json.dumps(report,indent=2,ensure_ascii=False)+'\n')
 print(json.dumps(report,indent=2,ensure_ascii=False))
