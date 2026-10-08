param(
    [ValidateSet("release", "release-lto")]
    [string]$Profile = "release",
    [string]$VcpkgRoot = $env:VCPKG_ROOT,
    [string]$VcpkgTriplet = "x64-windows"
)

$ErrorActionPreference = "Stop"

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "Rust Cargo is required. Install a native Rust toolchain first."
}
if (-not (Get-Command cl -ErrorAction SilentlyContinue) -and
    -not (Get-Command g++ -ErrorAction SilentlyContinue)) {
    throw "A C++17 compiler is required (MSVC Build Tools or MinGW-w64)."
}

if ($VcpkgRoot) {
    $installed = Join-Path $VcpkgRoot "installed\\$VcpkgTriplet"
    $include = Join-Path $installed "include"
    $library = Join-Path $installed "lib"
    if (-not (Test-Path $include) -or -not (Test-Path $library)) {
        throw "The requested vcpkg triplet is not installed: $installed"
    }
    $env:INCLUDE = "$include;$env:INCLUDE"
    $env:LIB = "$library;$env:LIB"
} else {
    Write-Warning "No VCPKG_ROOT supplied. Cargo may not find zlib, bzip2, liblzma, zstd, or Brotli."
}

Write-Host "Building CIX with profile $Profile"
& cargo build --locked --profile $Profile --lib --bin cix
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}

Write-Host "Native Windows qualification is required before distributing these binaries."
