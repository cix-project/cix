#!/usr/bin/env python3
"""Prepared native/portable CIXG1 differential; never downloads or builds."""
import argparse
import pathlib
import subprocess
import tempfile

parser = argparse.ArgumentParser()
parser.add_argument('--wasm', type=pathlib.Path, required=True)
parser.add_argument('--node', default='node')
parser.add_argument('--cix', type=pathlib.Path, required=True)
parser.add_argument('--cixcat', type=pathlib.Path, required=True)
parser.add_argument('--input', type=pathlib.Path, required=True)
args = parser.parse_args()
for path in (args.wasm, args.cix, args.cixcat, args.input):
    if not path.is_file(): parser.error(f'not a regular file: {path}')
helper = pathlib.Path(__file__).with_name('portable_file_roundtrip.mjs')
with tempfile.TemporaryDirectory(prefix='cix-portable-diff-') as temporary:
    work = pathlib.Path(temporary); archive = work / 'portable.cix'; native = work / 'native.out'; native_archive = work / 'native.cix'; portable = work / 'portable.out'
    subprocess.run([args.node, str(helper), 'encode', str(args.wasm), str(args.input), str(archive), str(args.input.stat().st_size * 4 + 128)], check=True)
    with native.open('wb') as output:
        subprocess.run([str(args.cixcat), str(archive)], check=True, stdout=output)
    if native.read_bytes() != args.input.read_bytes(): raise SystemExit('native CIXG1 decode differs')
    with native_archive.open('wb') as output:
        subprocess.run([str(args.cix), '--fast', '-c', '--format', 'cixg1', '--route', 'composition', '--backend', 'range', '--block-size', '65536B', str(args.input)], check=True, stdout=output)
    subprocess.run([args.node, str(helper), 'decode', str(args.wasm), str(native_archive), str(portable), str(args.input.stat().st_size)], check=True)
    if portable.read_bytes() != args.input.read_bytes(): raise SystemExit('portable subset decode differs')
print('portable encoder <-> forced-native CIXG1 subset: exact')
