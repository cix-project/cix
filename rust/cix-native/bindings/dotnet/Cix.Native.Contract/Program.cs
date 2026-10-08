using Cix.Native;
using System;
using System.Collections.Generic;
using System.Linq;
using System.Threading;

static void Add(List<byte> all, byte[] b, int n) {
    if (n < 0 || n > b.Length) throw new InvalidOperationException("invalid progress");
    all.AddRange(b.AsSpan(0, n).ToArray());
}
static byte[] Encode(CixOptionsV1 o, byte[] input) {
    using var h = CixStreamEncoder.Create(o); var all = new List<byte>(); var b = new byte[257]; var at = 0;
    for (var i = 0; i < 100_000 && at < input.Length; i++) { var p = h.Process(input.AsSpan(at, Math.Min(17, input.Length - at)), b); Add(all, b, p.Produced); at += p.Consumed; if (p.Consumed == 0 && p.Produced == 0) throw new InvalidOperationException("encoder stalled"); }
    if (at != input.Length) throw new InvalidOperationException("encoder input loop exhausted");
    for (var i = 0; i < 100_000; i++) { var p = h.Finish(b); Add(all, b, p.Produced); if (p.State == CixStreamState.Finished) return all.ToArray(); if (p.Produced == 0) throw new InvalidOperationException("encoder finish stalled"); }
    throw new InvalidOperationException("encoder finish loop exhausted");
}
static byte[] Decode(CixOptionsV1 o, byte[] archive) {
    using var h = CixStreamDecoder.Create(o); var all = new List<byte>(); var b = new byte[113]; var at = 0;
    for (var i = 0; i < 100_000 && at < archive.Length; i++) { var p = h.Process(archive.AsSpan(at, Math.Min(13, archive.Length - at)), b); Add(all, b, p.Produced); at += p.Consumed; if (p.Consumed == 0 && p.Produced == 0) throw new InvalidOperationException("decoder stalled"); }
    if (at != archive.Length) throw new InvalidOperationException("decoder input loop exhausted");
    for (var i = 0; i < 100_000; i++) { var p = h.Finish(b); Add(all, b, p.Produced); if (p.State == CixStreamState.Finished) return all.ToArray(); if (p.Produced == 0) throw new InvalidOperationException("decoder finish stalled"); }
    throw new InvalidOperationException("decoder finish loop exhausted");
}
var path = Environment.GetEnvironmentVariable("CIX_NATIVE_LIBRARY") ?? throw new InvalidOperationException("Set CIX_NATIVE_LIBRARY to an absolute native library path.");
CixNative.ConfigureLibrary(path);
var options = CixOptionsV1.Create(memoryLimit: 8UL << 20, outputLimit: 8UL << 20);
using var context = CixNative.Create(options);
var source = Enumerable.Range(0, 65_537).Select(i => (byte)(i * 31)).ToArray();
var archive = context.Encode(source, 1 << 20);
if (!source.AsSpan().SequenceEqual(context.Decode(archive, 1 << 20))) throw new InvalidOperationException("buffer round trip mismatch");
if (!source.AsSpan().SequenceEqual(Decode(options, Encode(options, source)))) throw new InvalidOperationException("fragmented stream round trip mismatch");
var emptyArchive = Encode(options, Array.Empty<byte>());
if (!Array.Empty<byte>().AsSpan().SequenceEqual(Decode(options, emptyArchive))) throw new InvalidOperationException("empty stream round trip mismatch");
using (var empty = CixStreamEncoder.Create(options)) {
    empty.Reset();
    try { empty.Reset(new CancellationToken(true)); throw new InvalidOperationException("cancellation not observed"); }
    catch (OperationCanceledException) { }
}
var disposed = CixStreamEncoder.Create(options);
disposed.Dispose();
try { _ = disposed.Process(ReadOnlySpan<byte>.Empty, new byte[1]); throw new InvalidOperationException("disposed handle accepted"); }
catch (ObjectDisposedException) { }
Console.WriteLine("CIX .NET native contract passed");
