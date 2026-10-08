// Usage: node portable_streaming.mjs cix_portable.wasm
import { readFile } from 'node:fs/promises';
import { PortableCix } from './cix-portable.mjs';
const wasm = await readFile(process.argv[2]);
const cix = await PortableCix.instantiate(wasm);
const input = new Uint8Array(65536 + 913);
for (let i = 0; i < input.length; i++) input[i] = (i % 97 < 73) ? (i % 13) : ((i * 29) >>> 3);
const stream = (kind, bytes, outputLimit, archiveLimit = 0) => {
  const e = cix.e, source = cix.alloc(bytes.length), scratch = cix.alloc(4096), counts = cix.alloc(8);
  const handle = kind === 'encode' ? e.cix_portable_encoder_new(outputLimit) : e.cix_portable_decoder_new(outputLimit, archiveLimit);
  if (!handle) throw new Error(`${kind} constructor failed`);
  cix.bytes(source, bytes.length).set(bytes);
  let offset = 0; const chunks = [];
  const call = (finish) => {
    const status = kind === 'encode'
      ? e.cix_portable_encoder_process(handle, source + offset, bytes.length - offset, scratch, 4096, finish ? 1 : 0, counts, counts + 4)
      : e.cix_portable_decoder_process(handle, source + offset, bytes.length - offset, scratch, 4096, counts, counts + 4);
    if (status < 0) throw new Error(`${kind} process failed (${status})`);
    const view = new DataView(e.memory.buffer); const consumed = view.getUint32(counts, true), produced = view.getUint32(counts + 4, true);
    offset += consumed; chunks.push(cix.bytes(scratch, produced).slice()); return [status, consumed, produced];
  };
  try {
    while (offset < bytes.length) { const [status, consumed, produced] = call(false); if (!consumed && !produced) throw new Error(`${kind} stalled before input completed`); if (status === 2 && offset < bytes.length) throw new Error(`${kind} finished before input completed`); }
    for (;;) { const [status, consumed, produced] = call(kind === 'encode'); if (consumed) throw new Error(`${kind} consumed after input completion`); if (status === 2) break; if (!produced) throw new Error(`${kind} stalled at finish`); }
  } finally { if (kind === 'encode') e.cix_portable_encoder_free(handle); else e.cix_portable_decoder_free(handle); cix.free(source, bytes.length); cix.free(scratch, 4096); cix.free(counts, 8); }
  const size = chunks.reduce((n, part) => n + part.length, 0), result = new Uint8Array(size); let at = 0; for (const part of chunks) { result.set(part, at); at += part.length; } return result;
};
const archive = stream('encode', input, input.length * 4 + 128);
const restored = stream('decode', archive, input.length, archive.length);
if (restored.length !== input.length || restored.some((v, i) => v !== input[i])) throw new Error('streaming mismatch');
const rejects = (f, label) => { try { f(); } catch { return; } throw new Error(`${label} accepted`); };
rejects(() => cix.encode(input, 1), 'low output cap');
rejects(() => cix.decode(archive.slice(0, -1), input.length), 'truncated archive');
const tailed = new Uint8Array(archive.length + 1); tailed.set(archive); tailed[archive.length] = 0;
rejects(() => cix.decode(tailed, input.length), 'trailing archive');
const malformed = archive.slice(); malformed[0] ^= 0xff;
rejects(() => cix.decode(malformed, input.length), 'malformed archive');
const scratch = cix.alloc(64);
try {
  if (cix.e.cix_portable_encode(0, 1, scratch, 64) >= 0) throw new Error('invalid pointer accepted');
  const encoder = cix.e.cix_portable_encoder_new(1024);
  try { if (cix.e.cix_portable_encoder_process(encoder, 0, 0, scratch, 64, 0, 0, 0) >= 0) throw new Error('missing result pointers accepted'); }
  finally { cix.e.cix_portable_encoder_free(encoder); }
} finally { cix.free(scratch, 64); }
console.log(JSON.stringify({ input: input.length, archive: archive.length, exact: true, streaming: true }));
