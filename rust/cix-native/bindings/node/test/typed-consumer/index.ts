import { Context, Decoder, Encoder, Stream, type Options, type StreamProgress } from "cix-native-node";
const options: Options = { profile: 2, workers: 1, outputLimit: 1 << 20, memoryLimit: 128 << 20 };
const context = new Context(options);
const input = Buffer.from("typed CIX consumer");
const encoded: Buffer = context.encode(input);
const decoded: Buffer = context.decode(encoded);
const encoder = new Encoder(options);
const progress: StreamProgress = encoder.process(input, 4096);
const decoder = new Decoder(options);
decoder.flush(4096); // Runtime rejects this exposed prototype method.
context.dispose(); encoder.dispose(); decoder.dispose(); void decoded; void progress;
// Declaration boundary checks: the native N-API layer accepts only Buffers and
// JavaScript number values, never generic typed arrays or bigint values.
// @ts-expect-error Uint8Array is not a Node Buffer accepted by napi_is_buffer.
context.encode(new Uint8Array([1]));
// @ts-expect-error bigint is not accepted by napi_get_value_double.
context.encode(1n);
// @ts-expect-error stream capacity is a JavaScript number, not bigint.
encoder.process(input, 4096n);
// @ts-expect-error native option parsing accepts number values, not bigint.
new Context({ outputLimit: 1n });

// @ts-expect-error Stream is a type-only shared method shape, not a runtime export.
new Stream();
