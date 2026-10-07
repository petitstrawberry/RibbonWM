"""Record manual dragging of one owned fixture without restarting the WM."""
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import threading
import time
ROOT=Path(__file__).resolve().parent.parent
HELPER=ROOT/'native/build/test-window'
BINARY=sys.argv[1]
def cli(*args):
    return json.loads(subprocess.check_output([BINARY,*map(str,args)],timeout=3))
assert os.environ.get('IN_NIX_SHELL')
assert cli('status')['mode']=='live'
anchored=os.environ.get('RIBBONWM_QA_ANCHOR')=='1'
prefix='manual-drag-anchored' if anchored else 'manual-drag'
p=subprocess.Popen([str(HELPER),'--fixture',json.dumps(dict(regular=True,geometry_test=True,anchor_idle=anchored,backdrops=[],lifetime=150,target=dict(x=300,y=150)))],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True,bufsize=1)
replies=queue.Queue()
def read():
    for line in p.stdout: replies.put(json.loads(line))
threading.Thread(target=read,daemon=True).start()
trace=None
try:
    ready=replies.get(timeout=4); assert ready['ready'];wid=ready['wid']
    p.stdin.write('{"op":"present"}\n');p.stdin.flush()
    until=time.monotonic()+8
    while time.monotonic()<until:
        s=cli('status')
        if wid in s['presented_windows']:break
        time.sleep(.02)
    else:raise AssertionError('Fixture not presented')
    cli('focus-window',wid)
    time.sleep(.3)
    with (ROOT/f'docs/{prefix}-native-trace.jsonl').open('w') as log:
        trace=subprocess.Popen([str(HELPER),'--trace-window',str(wid),str(p.pid),'90'],stdout=log,stderr=subprocess.PIPE,text=True)
        print('READY: drag the title bar of the blue RibbonWM QA geometry window several times. Recording for 90 seconds.',flush=True)
        with (ROOT/f'docs/{prefix}-daemon-trace.jsonl').open('w') as states:
            while trace.poll() is None:
                s=cli('status')
                states.write(json.dumps(dict(time=time.monotonic(),mouse_hold=s['mouse_hold'],overview=s['native_overview'],
                    requests=s['native_size_requests'],pending=s['native_size_pending'],
                    plans=[x for x in s['placements'] if x['window']==wid]))+'\n')
                states.flush();time.sleep(.02)
        assert trace.returncode==0,trace.stderr.read()
finally:
    if trace is not None and trace.poll() is None:trace.terminate();trace.wait(timeout=3)
    if p.poll() is None:
        p.stdin.write('{"op":"quit"}\n');p.stdin.flush();p.wait(timeout=5)
    p.stdin.close();p.stdout.close()
print('Owned fixture closed; original service retained.',flush=True)
