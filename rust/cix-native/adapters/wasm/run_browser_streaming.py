#!/usr/bin/env python3
"""Serve the local portable adapter and obtain a structured Chromium contract result."""
import argparse
import html
import http.server
import json
import pathlib
import shutil
import socketserver
import subprocess
import tempfile
import threading

parser = argparse.ArgumentParser()
parser.add_argument('--wasm', type=pathlib.Path, required=True)
parser.add_argument('--chromium', type=pathlib.Path, required=True)
parser.add_argument('--timeout-ms', type=int, default=15000)
args = parser.parse_args()
if not args.wasm.is_file() or not args.chromium.is_file() or args.timeout_ms <= 0:
    parser.error('--wasm and --chromium must be regular files and timeout must be positive')
root = pathlib.Path(__file__).resolve().parent
with tempfile.TemporaryDirectory(prefix='cix-portable-browser-') as temporary:
    stage = pathlib.Path(temporary)
    for name in ('browser_streaming.html', 'browser_streaming.mjs', 'cix-portable.mjs'):
        shutil.copy2(root / name, stage / name)
    shutil.copy2(args.wasm, stage / 'cix_portable.wasm')
    handler = lambda *a, **k: http.server.SimpleHTTPRequestHandler(*a, directory=stage, **k)
    with socketserver.TCPServer(('127.0.0.1', 0), handler) as server:
        thread = threading.Thread(target=server.serve_forever, daemon=True); thread.start()
        url = f'http://127.0.0.1:{server.server_address[1]}/browser_streaming.html?wasm=/cix_portable.wasm'
        try:
            completed = subprocess.run([str(args.chromium), '--headless', '--disable-gpu',
                '--no-first-run', '--disable-background-networking',
                f'--user-data-dir={stage / "browser-profile"}',
                '--run-all-compositor-stages-before-draw',
                f'--virtual-time-budget={args.timeout_ms}', '--dump-dom', url],
                capture_output=True, text=True, timeout=(args.timeout_ms / 1000) + 10)
        finally:
            server.shutdown(); thread.join()
    marker = '<pre id="result">'
    start = completed.stdout.find(marker)
    end = completed.stdout.find('</pre>', start + len(marker)) if start >= 0 else -1
    if completed.returncode or start < 0 or end < 0:
        print(json.dumps({'status': 'FAIL', 'reason': 'chromium invocation or result marker failed', 'returncode': completed.returncode, 'stderr': completed.stderr[-2000:]})); raise SystemExit(1)
    try: result = json.loads(html.unescape(completed.stdout[start + len(marker):end]))
    except json.JSONDecodeError as error:
        print(json.dumps({'status': 'FAIL', 'reason': f'invalid browser JSON: {error}'})); raise SystemExit(1)
    print(json.dumps(result, sort_keys=True))
    if result.get('status') != 'PASS': raise SystemExit(1)
