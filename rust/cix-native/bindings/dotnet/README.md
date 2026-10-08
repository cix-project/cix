# CIX .NET binding

Cix.Native is a dependency-free .NET 8 P/Invoke binding for the public CIX C
headers. ConfigureLibrary requires an absolute native library path and does not
fall back to platform search paths. Context and stream objects use SafeHandle.

Stream calls return exact consumed, produced, and state values. Managed
cancellation is checked before each native call; the ABI cannot interrupt an
in-flight native call. These streams cover native independent CIXG1 blocks,
not full-engine or retained-history streams.

The contract project has no NuGet dependency. With a .NET 8 SDK installed,
build `Cix.Native.Contract/Cix.Native.Contract.csproj`, then run the resulting
assembly with `CIX_NATIVE_LIBRARY` naming an absolute matching native library.
It checks buffers, fragmented streams, flush/finish, exact restoration, empty
input, reset, cancellation, and disposal. The Linux x86-64 contract was exercised
with SDK 8.0.425; other platforms still require their own native-library checks.
