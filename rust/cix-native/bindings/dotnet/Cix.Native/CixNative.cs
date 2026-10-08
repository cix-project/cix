using Microsoft.Win32.SafeHandles;

namespace Cix.Native;

public static class CixNative {
    public static void ConfigureLibrary(string absolutePath) => NativeApi.Configure(absolutePath);
    public static CixContext Create(CixOptionsV1 options) => CixContext.Create(options);
}
public sealed class CixContext : SafeHandleZeroOrMinusOneIsInvalid {
    private CixOptionsV1 options;
    private CixContext() : base(true) { }
    internal static CixContext Create(CixOptionsV1 options) {
        var status = NativeApi.cix_context_create(in options, out var raw);
        if (status != CixStatus.Ok) throw new InvalidOperationException($"CIX context create failed: {status}");
        var handle = new CixContext { options = options }; handle.SetHandle(raw); return handle;
    }
    protected override bool ReleaseHandle() { NativeApi.cix_context_destroy(handle); return true; }
    public unsafe byte[] Encode(ReadOnlySpan<byte> input, int outputCapacity) => Buffer(input, outputCapacity, true);
    public unsafe byte[] Decode(ReadOnlySpan<byte> input, int outputCapacity) => Buffer(input, outputCapacity, false);
    private unsafe byte[] Buffer(ReadOnlySpan<byte> input, int capacity, bool encode) {
        if (capacity < 0 || (ulong)capacity > options.OutputLimit || (ulong)input.Length > options.MemoryLimit
            || (ulong)capacity > options.MemoryLimit - (ulong)input.Length)
            throw new ArgumentOutOfRangeException(nameof(capacity), "The request exceeds configured native limits.");
        var output = GC.AllocateUninitializedArray<byte>(capacity);
        if (input.Overlaps(output)) throw new ArgumentException("Input and output must not overlap.");
        bool added = false; DangerousAddRef(ref added);
        fixed (byte* inputPtr = input)
        fixed (byte* outputPtr = output) {
            try {
                var native = DangerousGetHandle();
                CixStatus status = encode
                    ? NativeApi.cix_encode_buffer(native, inputPtr, (nuint)input.Length, outputPtr, (nuint)output.Length, out var needed)
                    : NativeApi.cix_decode_buffer(native, inputPtr, (nuint)input.Length, outputPtr, (nuint)output.Length, out needed);
                if (status != CixStatus.Ok) throw new InvalidOperationException($"CIX buffer operation failed: {status}; needed={needed}");
                if (needed > (nuint)output.Length || needed > int.MaxValue) throw new InvalidOperationException("Native returned an invalid output length.");
                return output[..(int)needed];
            } finally { if (added) DangerousRelease(); }
        }
    }
}
public abstract class CixStreamHandle : SafeHandleZeroOrMinusOneIsInvalid {
    protected CixStreamHandle() : base(true) { }
    protected abstract unsafe CixStatus ProcessNative(IntPtr native, byte* input, nuint inputLength, byte* output, nuint outputLength, out StreamResult result);
    protected abstract unsafe CixStatus FinishNative(IntPtr native, byte* output, nuint outputLength, out StreamResult result);
    protected abstract CixStatus ResetNative(IntPtr native);
    protected static void Validate(StreamResult result, int inputLength, int outputLength, bool finish = false) {
        if (result.Consumed > (nuint)inputLength || result.Produced > (nuint)outputLength
            || !Enum.IsDefined(result.State) || (finish && result.Consumed != 0))
            throw new InvalidOperationException("Native returned invalid stream progress.");
    }
    public unsafe (int Consumed, int Produced, CixStreamState State) Process(ReadOnlySpan<byte> input, Span<byte> output, CancellationToken cancellationToken = default) {
        cancellationToken.ThrowIfCancellationRequested();
        fixed (byte* inputPtr = input) fixed (byte* outputPtr = output) {
            StreamResult result = default;
            bool added = false; DangerousAddRef(ref added);
            CixStatus status;
            try { status = ProcessNative(DangerousGetHandle(), inputPtr, (nuint)input.Length, outputPtr, (nuint)output.Length, out result); }
            finally { if (added) DangerousRelease(); }
            if (status != CixStatus.Ok) throw new InvalidOperationException($"CIX stream process failed: {status}");
            Validate(result, input.Length, output.Length);
            return ((int)result.Consumed, (int)result.Produced, result.State);
        }
    }
    public unsafe (int Consumed, int Produced, CixStreamState State) Finish(Span<byte> output, CancellationToken cancellationToken = default) {
        cancellationToken.ThrowIfCancellationRequested();
        fixed (byte* outputPtr = output) {
            StreamResult result = default;
            bool added = false; DangerousAddRef(ref added);
            CixStatus status;
            try { status = FinishNative(DangerousGetHandle(), outputPtr, (nuint)output.Length, out result); }
            finally { if (added) DangerousRelease(); }
            if (status != CixStatus.Ok) throw new InvalidOperationException($"CIX stream finish failed: {status}");
            Validate(result, 0, output.Length, true);
            return ((int)result.Consumed, (int)result.Produced, result.State);
        }
    }
    public void Reset(CancellationToken cancellationToken = default) {
        cancellationToken.ThrowIfCancellationRequested();
        bool added = false; DangerousAddRef(ref added);
        CixStatus status;
        try { status = ResetNative(DangerousGetHandle()); }
        finally { if (added) DangerousRelease(); }
        if (status != CixStatus.Ok) throw new InvalidOperationException($"CIX stream reset failed: {status}");
    }
}
public sealed class CixStreamEncoder : CixStreamHandle {
    private CixStreamEncoder() { }
    public static CixStreamEncoder Create(CixOptionsV1 options) {
        var status = NativeApi.cix_stream_encoder_create(in options, out var raw);
        if (status != CixStatus.Ok) throw new InvalidOperationException($"CIX encoder create failed: {status}");
        var handle = new CixStreamEncoder(); handle.SetHandle(raw); return handle;
    }
    protected override bool ReleaseHandle() { NativeApi.cix_stream_encoder_destroy(handle); return true; }
    protected override unsafe CixStatus ProcessNative(IntPtr native, byte* input, nuint inputLength, byte* output, nuint outputLength, out StreamResult result)
        => NativeApi.cix_stream_encoder_process(native, input, inputLength, output, outputLength, out result);
    protected override unsafe CixStatus FinishNative(IntPtr native, byte* output, nuint outputLength, out StreamResult result)
        => NativeApi.cix_stream_encoder_finish(native, output, outputLength, out result);
    protected override CixStatus ResetNative(IntPtr native) => NativeApi.cix_stream_encoder_reset(native);
    public unsafe (int Consumed, int Produced, CixStreamState State) Flush(Span<byte> output, CancellationToken cancellationToken = default) {
        cancellationToken.ThrowIfCancellationRequested();
        fixed (byte* outputPtr = output) {
            StreamResult result = default;
            bool added = false; DangerousAddRef(ref added);
            CixStatus status;
            try { status = NativeApi.cix_stream_encoder_flush(DangerousGetHandle(), outputPtr, (nuint)output.Length, out result); }
            finally { if (added) DangerousRelease(); }
            if (status != CixStatus.Ok) throw new InvalidOperationException($"CIX stream flush failed: {status}");
            Validate(result, 0, output.Length, true);
            return ((int)result.Consumed, (int)result.Produced, result.State);
        }
    }
}
public sealed class CixStreamDecoder : CixStreamHandle {
    private CixStreamDecoder() { }
    public static CixStreamDecoder Create(CixOptionsV1 options) {
        var status = NativeApi.cix_stream_decoder_create(in options, out var raw);
        if (status != CixStatus.Ok) throw new InvalidOperationException($"CIX decoder create failed: {status}");
        var handle = new CixStreamDecoder(); handle.SetHandle(raw); return handle;
    }
    protected override bool ReleaseHandle() { NativeApi.cix_stream_decoder_destroy(handle); return true; }
    protected override unsafe CixStatus ProcessNative(IntPtr native, byte* input, nuint inputLength, byte* output, nuint outputLength, out StreamResult result)
        => NativeApi.cix_stream_decoder_process(native, input, inputLength, output, outputLength, out result);
    protected override unsafe CixStatus FinishNative(IntPtr native, byte* output, nuint outputLength, out StreamResult result)
        => NativeApi.cix_stream_decoder_finish(native, output, outputLength, out result);
    protected override CixStatus ResetNative(IntPtr native) => NativeApi.cix_stream_decoder_reset(native);
}
