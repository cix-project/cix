import { PortableCix } from './cix-portable.mjs';

const result = document.querySelector('#result');
const publish = (value) => { result.textContent = JSON.stringify(value); window.__CIX_PORTABLE_RESULT__ = value; };
const equal = (a, b) => a.length === b.length && !a.some((value, index) => value !== b[index]);
const chunks = (bytes, sizes, onCancel) => {
  let at = 0, index = 0;
  return new ReadableStream({
    pull(controller) {
      if (at === bytes.length) { controller.close(); return; }
      const size = sizes[index++ % sizes.length], end = Math.min(bytes.length, at + size);
      controller.enqueue(bytes.slice(at, end)); at = end;
    },
    cancel(reason) { if (onCancel) onCancel(reason); },
  });
};
const collect = async (stream) => {
  const reader = stream.getReader(), pieces = [];
  for (;;) { const step = await reader.read(); if (step.done) break; pieces.push(step.value); }
  const length = pieces.reduce((total, part) => total + part.length, 0), out = new Uint8Array(length); let at = 0;
  for (const part of pieces) { out.set(part, at); at += part.length; }
  return out;
};
const rejects = async (work, label) => {
  try { await work(); } catch { return; }
  throw new Error(`${label} accepted`);
};

try {
  const wasm = new URL(new URLSearchParams(location.search).get('wasm') || './cix_portable.wasm', location.href);
  const cix = await PortableCix.fromFetch(wasm);
  const input = new Uint8Array(65536 + 917);
  for (let index = 0; index < input.length; index++) input[index] = index % 101 < 79 ? index % 17 : ((index * 53) >>> 5);
  const archive = await collect(cix.encodeStream(chunks(input, [1, 7, 4093, 31]), { outputLimit: input.length * 4 + 128, outputChunkSize: 257 }));
  const restored = await collect(cix.decodeStream(chunks(archive, [2, 19, 73, 2048]), { outputLimit: input.length, archiveLimit: archive.length, outputChunkSize: 193 }));
  if (!equal(restored, input)) throw new Error('fragmented stream round-trip mismatch');
  const emptyArchive = await collect(cix.encodeStream(chunks(new Uint8Array(), [1]), { outputLimit: 54, outputChunkSize: 13 }));
  const empty = await collect(cix.decodeStream(chunks(emptyArchive, [1]), { outputLimit: 0, archiveLimit: emptyArchive.length, outputChunkSize: 11 }));
  if (empty.length !== 0) throw new Error('empty stream mismatch');
  await rejects(() => collect(cix.encodeStream(chunks(input, [5]), { outputLimit: 1 })), 'low encode cap');
  await rejects(() => collect(cix.decodeStream(chunks(archive.slice(0, -1), [9]), { outputLimit: input.length, archiveLimit: archive.length })), 'truncated archive');
  const tailed = new Uint8Array(archive.length + 1); tailed.set(archive); tailed[archive.length] = 0;
  await rejects(() => collect(cix.decodeStream(chunks(tailed, [33]), { outputLimit: input.length, archiveLimit: tailed.length })), 'tailed archive');
  const malformed = archive.slice(); malformed[0] ^= 0xff;
  await rejects(() => collect(cix.decodeStream(chunks(malformed, [33]), { outputLimit: input.length, archiveLimit: malformed.length })), 'malformed archive');
  await rejects(() => collect(cix.decodeStream(chunks(archive, [33]), { outputLimit: 1, archiveLimit: archive.length })), 'low decode cap');
  let cancelled = false;
  const never = new ReadableStream({ pull(controller) { controller.enqueue(new Uint8Array([1, 2, 3])); }, cancel() { cancelled = true; } });
  const reader = cix.encodeStream(never, { outputLimit: 1024, outputChunkSize: 17 }).getReader();
  await reader.read(); await reader.cancel('browser-contract-cancel');
  if (!cancelled) throw new Error('stream cancellation did not cancel the source');
  publish({ status: 'PASS', browser: true, fragmented: true, empty: true, malformed: true, caps: true, cancellation: true, inputBytes: input.length, archiveBytes: archive.length });
} catch (error) {
  publish({ status: 'FAIL', message: String(error?.message || error) });
}
