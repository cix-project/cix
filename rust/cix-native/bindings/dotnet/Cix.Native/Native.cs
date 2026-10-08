using System.Runtime.InteropServices;

namespace Cix.Native;

public enum CixStatus : int { Ok, InvalidArgument, InvalidOptions, OutputTooSmall, CodecError, Panic, ResourceLimit }
public enum CixProfile : uint { Fast = 1, Default = 2, Best = 3 }
public enum CixStreamState : uint { NeedsInput = 1, NeedsOutput = 2, Finished = 3 }

[StructLayout(LayoutKind.Sequential)]
public struct CixOptionsV1 {
    public uint AbiVersion, StructSize, Profile, Workers;
    public ulong OutputLimit, MemoryLimit;
    public static CixOptionsV1 Create(CixProfile profile = CixProfile.Default, uint workers = 1,
        ulong outputLimit = 128UL << 20, ulong memoryLimit = 128UL << 20) => new() {
            AbiVersion = 1, StructSize = (uint)Marshal.SizeOf<CixOptionsV1>(), Profile = (uint)profile,
            Workers = workers, OutputLimit = outputLimit, MemoryLimit = memoryLimit };
}
[StructLayout(LayoutKind.Sequential)]
public struct StreamResult { public nuint Consumed, Produced; public CixStreamState State; }

internal static class NativeApi {
    private const string Name = "cix_native";
    private static string? libraryPath;
    static NativeApi() => NativeLibrary.SetDllImportResolver(typeof(NativeApi).Assembly, Resolve);
    internal static void Configure(string path) {
        if (string.IsNullOrWhiteSpace(path) || !Path.IsPathFullyQualified(path))
            throw new ArgumentException("A fully-qualified native library path is required.", nameof(path));
        if (libraryPath is not null && !StringComparer.Ordinal.Equals(libraryPath, path))
            throw new InvalidOperationException("The native library path is already configured.");
        libraryPath = path;
    }
    private static IntPtr Resolve(string name, System.Reflection.Assembly _, DllImportSearchPath? __) {
        if (name != Name) return IntPtr.Zero;
        if (libraryPath is null) throw new DllNotFoundException("Call CixNative.ConfigureLibrary with an absolute CIX native library path first.");
        return NativeLibrary.Load(libraryPath);
    }
    [DllImport(Name, CallingConvention = CallingConvention.Cdecl)] internal static extern CixStatus cix_context_create(in CixOptionsV1 options, out IntPtr context);
    [DllImport(Name, CallingConvention = CallingConvention.Cdecl)] internal static extern void cix_context_destroy(IntPtr context);
    [DllImport(Name, CallingConvention = CallingConvention.Cdecl)] internal static extern unsafe CixStatus cix_encode_buffer(IntPtr context, byte* input, nuint inputLen, byte* output, nuint outputLen, out nuint needed);
    [DllImport(Name, CallingConvention = CallingConvention.Cdecl)] internal static extern unsafe CixStatus cix_decode_buffer(IntPtr context, byte* input, nuint inputLen, byte* output, nuint outputLen, out nuint needed);
    [DllImport(Name, CallingConvention = CallingConvention.Cdecl)] internal static extern CixStatus cix_stream_encoder_create(in CixOptionsV1 options, out IntPtr stream);
    [DllImport(Name, CallingConvention = CallingConvention.Cdecl)] internal static extern CixStatus cix_stream_decoder_create(in CixOptionsV1 options, out IntPtr stream);
    [DllImport(Name, CallingConvention = CallingConvention.Cdecl)] internal static extern void cix_stream_encoder_destroy(IntPtr stream);
    [DllImport(Name, CallingConvention = CallingConvention.Cdecl)] internal static extern void cix_stream_decoder_destroy(IntPtr stream);
    [DllImport(Name, CallingConvention = CallingConvention.Cdecl)] internal static extern unsafe CixStatus cix_stream_encoder_process(IntPtr stream, byte* input, nuint inputLen, byte* output, nuint outputLen, out StreamResult result);
    [DllImport(Name, CallingConvention = CallingConvention.Cdecl)] internal static extern unsafe CixStatus cix_stream_decoder_process(IntPtr stream, byte* input, nuint inputLen, byte* output, nuint outputLen, out StreamResult result);
    [DllImport(Name, CallingConvention = CallingConvention.Cdecl)] internal static extern unsafe CixStatus cix_stream_encoder_flush(IntPtr stream, byte* output, nuint outputLen, out StreamResult result);
    [DllImport(Name, CallingConvention = CallingConvention.Cdecl)] internal static extern unsafe CixStatus cix_stream_encoder_finish(IntPtr stream, byte* output, nuint outputLen, out StreamResult result);
    [DllImport(Name, CallingConvention = CallingConvention.Cdecl)] internal static extern unsafe CixStatus cix_stream_decoder_finish(IntPtr stream, byte* output, nuint outputLen, out StreamResult result);
    [DllImport(Name, CallingConvention = CallingConvention.Cdecl)] internal static extern CixStatus cix_stream_encoder_reset(IntPtr stream);
    [DllImport(Name, CallingConvention = CallingConvention.Cdecl)] internal static extern CixStatus cix_stream_decoder_reset(IntPtr stream);
}
