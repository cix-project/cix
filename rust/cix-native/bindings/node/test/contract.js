'use strict';
const assert = require('assert');
if (!process.env.CIX_NODE_ADDON) throw new Error('set CIX_NODE_ADDON to an explicit qualified .node path');
const cix = require(process.env.CIX_NODE_ADDON);
assert.throws(() => new cix.Context({ outputLimit: -1 }));
assert.throws(() => new cix.Context({ outputLimit: NaN }));
assert.throws(() => new cix.Context({ outputLimit: Infinity }));
assert.throws(() => new cix.Context({ outputLimit: Number.MAX_SAFE_INTEGER + 1 }));
const ctx = new cix.Context({ outputLimit: 1 << 20, memoryLimit: 128 << 20 });
const source = Buffer.from('node installed SDK contract '.repeat(64));
assert.deepStrictEqual(ctx.decode(ctx.encode(source)), source);
ctx.dispose(); ctx.dispose(); assert.throws(() => ctx.encode(source), /context is disposed/);
const encoder = new cix.Encoder({ outputLimit: 1 << 20, memoryLimit: 128 << 20 });
assert.throws(() => cix.Context.prototype.encode.call(encoder, source));
assert.throws(() => cix.Encoder.prototype.process.call(ctx, source, 4096));
assert.throws(() => cix.Context.prototype.decode.call({}, source));
encoder.cancel(); assert.throws(() => encoder.process(source, 4096)); encoder.reset();
assert.throws(() => encoder.process(source, 1 << 21)); encoder.dispose(); encoder.dispose();
assert.throws(() => encoder.process(source, 4096), /stream is disposed/);

function drain(stream, method) {
  const out = [];
  for (let i = 0; i < 64; i += 1) {
    const progress = stream[method](4096);
    assert.strictEqual(progress.produced, progress.data.length);
    out.push(progress.data);
    if (progress.state === 3) return Buffer.concat(out);
    assert(progress.produced > 0 || progress.state === 1, 'bounded terminal progress');
  }
  throw new Error(`${method} did not finish`);
}
function encodeFragments(input) {
  const stream = new cix.Encoder({ outputLimit: 1 << 20, memoryLimit: 128 << 20 });
  const out = [];
  for (let offset = 0; offset < input.length;) {
    const progress = stream.process(input.subarray(offset, offset + 7), 4096);
    assert(progress.consumed <= 7 && progress.produced === progress.data.length);
    out.push(progress.data); offset += progress.consumed;
  }
  out.push(drain(stream, 'finish')); stream.dispose(); return Buffer.concat(out);
}
function decodeFragments(input) {
  const stream = new cix.Decoder({ outputLimit: 1 << 20, memoryLimit: 128 << 20 });
  assert.throws(() => stream.flush(4096));
  const out = [];
  for (let offset = 0; offset < input.length;) {
    const progress = stream.process(input.subarray(offset, offset + 5), 4096);
    assert(progress.consumed <= 5 && progress.produced === progress.data.length);
    out.push(progress.data); offset += progress.consumed;
  }
  out.push(drain(stream, 'finish')); stream.reset(); stream.dispose(); return Buffer.concat(out);
}
assert.deepStrictEqual(decodeFragments(encodeFragments(source)), source);
assert.deepStrictEqual(decodeFragments(encodeFragments(Buffer.alloc(0))), Buffer.alloc(0));
