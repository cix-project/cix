// Browser/Node wrapper for the allocation-explicit cix-portable ABI.
export class PortableCix {
  constructor(instance) { this.instance = instance; this.e = instance.exports; }
  static async instantiate(bytes, imports = {}) {
    const { instance } = await WebAssembly.instantiate(bytes, imports);
    return new PortableCix(instance);
  }
  static async fromFetch(url, imports = {}) {
    const response = await fetch(url);
    if (!response.ok) throw new Error(`WASM fetch failed (${response.status})`);
    try {
      const { instance } = await WebAssembly.instantiateStreaming(response, imports);
      return new PortableCix(instance);
    } catch (first) {
      const retry = await fetch(url);
      if (!retry.ok) throw new Error(`WASM fetch retry failed (${retry.status})`);
      try { return await PortableCix.instantiate(await retry.arrayBuffer(), imports); }
      catch { throw first; }
    }
  }
  bytes(ptr, length) { return new Uint8Array(this.e.memory.buffer, ptr, length); }
  alloc(length) { const ptr = this.e.cix_portable_alloc(length); if (!ptr && length) throw new Error('WASM allocation failed'); return ptr; }
  free(ptr, length) { this.e.cix_portable_free(ptr, length); }
  oneShot(name, input, outputCap) {
    const source = this.alloc(input.length); const target = this.alloc(outputCap);
    try {
      this.bytes(source, input.length).set(input);
      const used = this.e[name](source, input.length, target, outputCap);
      if (used < 0) throw new Error(`${name} failed (${used})`);
      return this.bytes(target, used).slice();
    } finally { this.free(source, input.length); this.free(target, outputCap); }
  }
  encode(input, outputCap) { return this.oneShot('cix_portable_encode', input, outputCap); }
  decode(input, outputCap) { return this.oneShot('cix_portable_decode', input, outputCap); }
  encodeStream(source, { outputLimit, outputChunkSize = 16384 } = {}) {
    if (!Number.isSafeInteger(outputLimit) || outputLimit < 54) throw new Error('encode stream needs a finite CIXG1 output limit');
    return this.#stream('encode', source, outputLimit, outputChunkSize);
  }
  decodeStream(source, { outputLimit, archiveLimit, outputChunkSize = 16384 } = {}) {
    if (!Number.isSafeInteger(outputLimit) || outputLimit < 0 || !Number.isSafeInteger(archiveLimit) || archiveLimit < 54) throw new Error('decode stream needs finite output and archive limits');
    return this.#stream('decode', source, outputLimit, outputChunkSize, archiveLimit);
  }
  #stream(kind, source, outputLimit, outputChunkSize, archiveLimit = 0) {
    if (!(source instanceof ReadableStream)) throw new TypeError('portable stream source must be a ReadableStream');
    if (!Number.isSafeInteger(outputChunkSize) || outputChunkSize <= 0) throw new Error('portable stream chunk size must be positive');
    const e = this.e, reader = source.getReader(), scratch = this.alloc(outputChunkSize), counts = this.alloc(8);
    const handle = kind === 'encode' ? e.cix_portable_encoder_new(outputLimit) : e.cix_portable_decoder_new(outputLimit, archiveLimit);
    if (!handle) { this.free(scratch, outputChunkSize); this.free(counts, 8); throw new Error(`${kind} stream constructor failed`); }
    let current = null, sourcePtr = 0, sourceAt = 0, done = false, disposed = false;
    const dispose = () => {
      if (disposed) return; disposed = true;
      if (sourcePtr) this.free(sourcePtr, current.length);
      if (kind === 'encode') e.cix_portable_encoder_free(handle); else e.cix_portable_decoder_free(handle);
      this.free(scratch, outputChunkSize); this.free(counts, 8);
    };
    const next = async () => {
      for (;;) {
        if (!current && !done) {
          const item = await reader.read();
          if (item.done) done = true;
          else {
            if (!(item.value instanceof Uint8Array)) throw new TypeError('portable stream chunks must be Uint8Array');
            current = item.value; sourceAt = 0; sourcePtr = this.alloc(current.length); this.bytes(sourcePtr, current.length).set(current);
          }
        }
        const remaining = current ? current.length - sourceAt : 0;
        const status = kind === 'encode'
          ? e.cix_portable_encoder_process(handle, sourcePtr + sourceAt, remaining, scratch, outputChunkSize, done && !current ? 1 : 0, counts, counts + 4)
          : e.cix_portable_decoder_process(handle, sourcePtr + sourceAt, remaining, scratch, outputChunkSize, counts, counts + 4);
        if (status < 0) throw new Error(`${kind} stream failed (${status})`);
        const view = new DataView(e.memory.buffer), consumed = view.getUint32(counts, true), produced = view.getUint32(counts + 4, true);
        if (consumed > remaining || produced > outputChunkSize) throw new Error(`${kind} stream returned invalid progress`);
        sourceAt += consumed;
        if (current && sourceAt === current.length) { this.free(sourcePtr, current.length); current = null; sourcePtr = 0; sourceAt = 0; }
        if (produced) return { chunk: this.bytes(scratch, produced).slice(), status };
        if (status === 2) return { done: true };
        if (consumed) continue;
        if (done && kind === 'decode') throw new Error('truncated portable CIXG1 archive');
        if (done && kind === 'encode') continue;
        throw new Error(`${kind} stream made no progress`);
      }
    };
    return new ReadableStream({
      async pull(controller) {
        try { const result = await next(); if (result.done) { dispose(); controller.close(); } else { controller.enqueue(result.chunk); } }
        catch (error) { dispose(); controller.error(error); }
      },
      async cancel(reason) { try { await reader.cancel(reason); } finally { dispose(); } },
    });
  }
}
