"""Check centering and native level transitions using one owned fixture."""
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
BINARY = sys.argv[1]
assert os.environ.get('IN_NIX_SHELL')


def cli(*args):
    result = json.loads(subprocess.check_output([BINARY, *map(str, args)], timeout=5))
    assert result.get('ok'), result
    return result


assert cli('status')['mode'] == 'live'
fixture = subprocess.Popen([str(HELPER), '--fixture', json.dumps(dict(
    regular=True, geometry_test=True, backdrops=[], lifetime=45,
    target=dict(x=300, y=150)))], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
    text=True, bufsize=1)
replies = queue.Queue()


def read():
    for line in fixture.stdout:
        replies.put(json.loads(line))


threading.Thread(target=read, daemon=True).start()


def command(op):
    fixture.stdin.write(json.dumps(dict(op=op)) + '\n')
    fixture.stdin.flush()
    return replies.get(timeout=5)


def wait_tiled(wid):
    until = time.monotonic() + 8
    while time.monotonic() < until:
        status = cli('status')
        if wid in status['presented_windows']:
            return status
        time.sleep(.02)
    raise AssertionError('Fixture did not rejoin tiling')


def check_float(wid, expected_level, viewport, settings):
    status = cli('status')
    assert status['window_modes'][str(wid)]['floating']
    assert wid not in status['presented_windows']
    state = command('state')
    assert state['errors'] == [0, 0]
    assert state['level'] == expected_level, state
    frame = state['frame']
    left = settings['padding_left'] if settings['padding_left'] is not None else settings['horizontal_margin']
    right = settings['padding_right'] if settings['padding_right'] is not None else settings['horizontal_margin']
    top = settings['padding_top'] if settings['padding_top'] is not None else settings['vertical_margin']
    bottom = settings['padding_bottom'] if settings['padding_bottom'] is not None else settings['vertical_margin']
    x = viewport['x'] + left + (viewport['width'] - left - right - frame['width']) / 2
    y = viewport['y'] + top + (viewport['height'] - top - bottom - frame['height']) / 2
    assert abs(frame['x'] - x) <= 2 and abs(frame['y'] - y) <= 2, (frame, x, y)
    assert abs(-state['transform'][4] - frame['x']) <= 2
    assert abs(-state['transform'][5] - frame['y']) <= 2
    assert all(abs(state['appkit_frame'][key] - frame[key]) <= 2 for key in frame), state
    print(json.dumps(dict(centered=frame, level=state['level'], appkit_aligned=True)), flush=True)


try:
    ready = replies.get(timeout=5)
    assert ready['ready']
    wid = ready['wid']
    command('present')
    status = wait_tiled(wid)
    cli('focus-window', wid)
    monitor = next(p['monitor'] for p in status['placements'] if p['window'] == wid)
    viewport = status['state']['monitors'][monitor]['viewport']
    settings = status['state']['settings']
    original_level = command('state')['level']
    for _ in range(3):
        cli('float', 'on', '--window', wid)
        check_float(wid, max(original_level, 3), viewport, settings)
        cli('float', 'off', '--window', wid)
        wait_tiled(wid)
        assert command('state')['level'] == original_level
    cli('sticky', 'on', '--window', wid)
    assert command('state')['level'] == original_level
    cli('float', 'on', '--window', wid)
    check_float(wid, max(original_level, 3), viewport, settings)
    cli('float', 'off', '--window', wid)
    assert command('state')['level'] == original_level
    assert cli('status')['window_modes'][str(wid)]['sticky']
    cli('sticky', 'off', '--window', wid)
    wait_tiled(wid)
    print('PASS: float/unfloat, center, AppKit/native alignment, and sticky independence', flush=True)
finally:
    if fixture.poll() is None:
        fixture.stdin.write('{"op":"quit"}\n')
        fixture.stdin.flush()
        fixture.wait(timeout=5)
    fixture.stdin.close()
    fixture.stdout.close()
print('Owned fixture closed; existing service retained.', flush=True)
