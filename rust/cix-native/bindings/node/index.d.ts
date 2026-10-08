/** Type declarations for the installed native CIX Node-API add-on (N-API 8). */
export interface Options {
  /** Compression profile: 1, 2, or 3; supplied as a non-negative JavaScript safe-integer number. */
  profile?: number;
  /** Native worker count; a positive JavaScript safe-integer number. */
  workers?: number;
  /** Maximum emitted bytes; a non-negative JavaScript safe-integer number. */
  outputLimit?: number;
  /** Maximum native buffer budget; a non-negative JavaScript safe-integer number. */
  memoryLimit?: number;
}

/** Native stream state values returned by the installed CIXG1 ABI. */
export type StreamState = 1 | 2 | 3;
export interface StreamProgress {
  /** A newly allocated Node Buffer containing this call's emitted bytes. */
  data: Buffer;
  /** Number of input bytes consumed in this call. */
  consumed: number;
  /** Number of bytes in `data`. */
  produced: number;
  state: StreamState;
}

/** Complete-buffer native operations. Inputs must be Node Buffers, not generic Uint8Arrays or bigint lengths. */
export class Context {
  constructor(options?: Options);
  encode(input: Buffer): Buffer;
  decode(input: Buffer): Buffer;
  /** Idempotently releases the installed-SDK context. */
  dispose(): void;
}

/** Shared installed CIXG1 stream operations. All capacities are JavaScript safe-integer numbers. */
export interface Stream {
  process(input: Buffer, capacity: number): StreamProgress;
  finish(capacity: number): StreamProgress;
  reset(): void;
  /** Fences later add-on calls; it cannot interrupt a call already executing. */
  cancel(): void;
  /** Idempotently releases the installed-SDK stream. */
  dispose(): void;
}
export class Encoder implements Stream {
  constructor(options?: Options);
  process(input: Buffer, capacity: number): StreamProgress;
  finish(capacity: number): StreamProgress;
  reset(): void;
  cancel(): void;
  dispose(): void;
  flush(capacity: number): StreamProgress;
}
export class Decoder implements Stream {
  constructor(options?: Options);
  process(input: Buffer, capacity: number): StreamProgress;
  finish(capacity: number): StreamProgress;
  reset(): void;
  cancel(): void;
  dispose(): void;
  /** Present on the native prototype but rejects because decoder streams cannot flush. */
  flush(capacity: number): StreamProgress;
}
