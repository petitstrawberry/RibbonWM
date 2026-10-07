"""Record intermediate geometry of owned adjacent columns in the live service."""
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import threading
import time

ROOT = Path(__file__).resolve().parent.parent
BINARY = sys.argv[1]
HELPER = ROOT / 'native/build/test-window'
assert os.environ.get('IN_NIX_SHELL')
def cli(*args):
    value = json.loads(subprocess.check_output([BINARY, *map(str, args)], timeout=5))
    assert value.get('ok'), value
    return value
assert cli('status')['mode'] == 'live'
p = subprocess.Popen([str(HELPER), '--fixture', json.dumps(dict(regular=True, geometry_test=True, backdrops=[], lifetime=45, target=dict(x=300,y=150)))], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)
replies = queue.Queue()
samples = []
trace = None
trace_thread = None
def reader():
    for line in p.stdout:
        value = json.loads(line)
        if value.get('event') == 'frame-sample': samples.append(value)
        else: replies.put(value)
thread = threading.Thread(target=reader,daemon=True); thread.start()
def command(op):
    p.stdin.write(json.dumps(dict(op=op))+'\n');p.stdin.flush()
    return replies.get(timeout=5)
def wait(wid, width=None):
    until=time.monotonic()+8
    while time.monotonic()<until:
        status=cli('status')
        assert not status['native_overview'], 'Overview interrupted measurement'
        if wid in status['presented_windows']:
            plan=next(x for x in status['placements'] if x['window']==wid)
            if width is None or abs(plan['frame']['width']-width)<2: return status
        time.sleep(.01)
    raise AssertionError('Presentation did not settle')
try:
    ready=replies.get(timeout=5); assert ready['ready'];wid=ready['wid']
    command('present');wait(wid);cli('focus-window',wid)
    child=command('new')['created'];wait(child);cli('focus-window',wid)
    status=wait(wid);monitor=next(x['monitor'] for x in status['placements'] if x['window']==wid)
    placements=status['placements']
    a=next(x for x in placements if x['window']==wid); b=next(x for x in placements if x['window']==child)
    gap=status['state']['settings']['gap']
    assert abs(b['frame']['x']-a['frame']['x']-a['frame']['width']-gap)<2, 'Fixtures are not adjacent'
    trace=subprocess.Popen([str(HELPER),'--trace-pair',json.dumps([wid,child]),str(p.pid),'15'],stdout=subprocess.PIPE,text=True)
    trace_thread=threading.Thread(target=lambda: samples.extend(json.loads(line) for line in trace.stdout),daemon=True)
    trace_thread.start()
    steps=[]
    for width in [620,940,740,1020,520,800]:
        assert cli('status')['native_focused_window']==wid, 'Focus changed during measurement'
        begin=time.monotonic();cli('--monitor',monitor,'resize',width);wait(wid,width);time.sleep(.12)
        steps.append(dict(width=width,begin=begin,end=time.monotonic()))
    report=[]
    for step in steps:
        relevant=[s for s in samples if step['begin']<=s['time']<=step['end']]
        errors=[]
        for s in relevant:
            by_id={w['wid']:w for w in s['windows']}
            if wid not in by_id or child not in by_id: continue
            a,b=by_id[wid],by_id[child]
            assert a['errors']==[0,0] and b['errors']==[0,0]
            # Actual surface width and its clip both matter: clipping a wider
            # surface to a future width can hide content during acceptance.
            error=(-b['transform'][4])-(-a['transform'][4])-a['frame']['width']-gap
            errors.append(dict(time=s['time'],gap_error=error,width=a['frame']['width'],clip=a['clip_bounds']))
        assert len(errors)>5, 'Too few intermediate samples'
        bad=[v for v in errors if abs(v['gap_error'])>3]
        report.append(dict(width=step['width'],samples=len(errors),max_gap_error=max(abs(v['gap_error']) for v in errors),bad_samples=len(bad)))
    print(json.dumps(report,indent=2),flush=True)
    (ROOT/'docs/live-width-frames-trace.jsonl').write_text(''.join(json.dumps(s)+'\n' for s in samples))
finally:
    if trace is not None:
        trace.terminate();trace.wait(timeout=3);trace_thread.join(timeout=1);trace.stdout.close()
    if p.poll() is None:
        p.stdin.write('{"op":"quit"}\n');p.stdin.flush();p.wait(timeout=5)
    thread.join(timeout=1)
    p.stdin.close();p.stdout.close()
