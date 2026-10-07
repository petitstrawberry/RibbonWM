"""Observe a user's Mission Control selection of two owned blue fixtures.

Run inside nix develop against the existing live service. No synthesized input,
service replacement, or queries into other applications' AX trees.
"""
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import threading
import time

ROOT = Path(__file__).resolve().parent.parent
HELPER = ROOT / 'native/build/test-window'
BINARY = sys.argv[1] if len(sys.argv) > 1 else ROOT / 'target/debug/ribbonwm'


def cli(*args):
    return json.loads(subprocess.check_output([str(BINARY), *map(str, args)], timeout=3))


def main():
    assert os.environ.get('IN_NIX_SHELL')
    assert json.loads(subprocess.check_output([str(HELPER), '--session-state']))['session_active']
    assert cli('status')['mode'] == 'live'
    process = subprocess.Popen([str(HELPER), '--fixture', json.dumps(dict(
        regular=True, geometry_test=True, backdrops=[], lifetime=180,
        target=dict(x=300, y=150)))], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        text=True, bufsize=1)
    replies = queue.Queue()
    def read():
        for line in process.stdout:
            replies.put(json.loads(line))
    threading.Thread(target=read, daemon=True).start()
    def command(op, expected):
        process.stdin.write(json.dumps(dict(op=op))+'\n')
        process.stdin.flush()
        deadline = time.monotonic()+4
        while time.monotonic() < deadline:
            value = replies.get(timeout=4)
            if expected(value):
                return value
        raise AssertionError('Fixture response missing')
    def wait(predicate):
        deadline = time.monotonic()+8
        while time.monotonic() < deadline:
            state = cli('status')
            if predicate(state):
                return state
            time.sleep(.02)
        raise AssertionError('Fixture not presented')
    try:
        ready = replies.get(timeout=4)
        assert ready['ready']
        wid = ready['wid']
        command('present', lambda r:r.get('presented'))
        wait(lambda s:wid in s['presented_windows'])
        child = command('new', lambda r:'created' in r)['created']
        state = wait(lambda s:child in s['presented_windows'])
        monitor = next(p['monitor'] for p in state['placements'] if p['window']==wid)
        for target in [wid, child]:
            cli('focus-window', target)
            wait(lambda s:s['native_focused_window']==target)
            cli('--monitor', monitor, 'resize', 1000)
            wait(lambda s:target in s['presented_windows'] and any(
                p['window']==target and abs(p['frame']['width']-1000)<2 for p in s['placements']))
        time.sleep(1.5)
        initial = command('state', lambda r:r.get('event')=='state')
        baseline = cli('status')['native_size_requests']
        print('READY: select the blue RibbonWM QA geometry window in Mission Control, then repeat.', flush=True)
        until = time.monotonic()+90
        previous = False
        ended = None
        cycles = []
        with (ROOT/'docs/overview-selection-trace.jsonl').open('w') as log:
            while time.monotonic() < until and len(cycles)<3:
                state = cli('status')
                if not state['session_active']:
                    raise AssertionError('Session became inactive')
                active = state['native_overview']
                now = time.monotonic()
                if previous and not active:
                    ended = now
                if active:
                    ended = None
                own = command('state', lambda r:r.get('event')=='state')
                plan = next((p for p in state['placements'] if p['window']==wid), None)
                row = dict(time=now, overview=active, phase=state['control_phase'], selected=state['native_focused_window']==wid,
                           requests=state['native_size_requests']-baseline, own=own, plan=plan)
                log.write(json.dumps(row)+'\n')
                if ended is not None and row['selected'] and plan and plan['clip']==plan['frame']:
                    t = own['transform']; f = plan['frame']; clip = own['clip_bounds']
                    if (abs(-t[4]-f['x'])<2 and abs(-t[5]-f['y'])<2
                        and abs(clip[2]-f['width'])<2 and abs(clip[3]-f['height'])<2):
                        cycles.append(round((now-ended)*1000, 1))
                        print(json.dumps(dict(cycle=len(cycles), observed_exit_to_selected_ms=cycles[-1],
                                              resize_requests=row['requests'])), flush=True)
                        # Record resize activity; do not terminate a user's diagnostic session.
                        assert own['frame']['width']==initial['frame']['width']
                        assert own['frame']['height']==initial['frame']['height']
                        ended = None
                previous = active
                time.sleep(.008)
        # Keep a usable trace even if selection was another owned fixture.
        print(json.dumps(dict(completed_cycles=len(cycles), observed_exit_to_selected_ms=cycles)), flush=True)
    finally:
        if process.poll() is None:
            process.stdin.write('{"op":"quit"}\n'); process.stdin.flush()
            process.wait(timeout=5)
        process.stdin.close(); process.stdout.close()
    print('Owned fixtures closed; existing service retained.', flush=True)


if __name__ == '__main__':
    main()
