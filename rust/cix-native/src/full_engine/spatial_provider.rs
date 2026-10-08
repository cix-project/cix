//! Explicit-package native JPEG-LS/JPEG 2000 provider.
//!
//! This module deliberately has no `ImageBackend` implementation yet: root
//! composes it into the full-engine route registry after all spatial providers
//! are present. It never searches a host library or invokes Python.

use crate::full_engine::dynamic_library::{DynamicLibrary, DynamicLibraryError};
use std::ffi::c_int;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum SpatialCodec {
    Jpeg2000 = 2,
    JpegLs = 3,
}

#[repr(C)]
struct RawBuffer {
    data: *mut u8,
    size: usize,
}
#[repr(C)]
struct RawInfo {
    width: u32,
    height: u32,
    bits_per_sample: u32,
}
type EncodeFn =
    unsafe extern "C" fn(u32, *const u8, usize, u32, u32, u32, usize, *mut RawBuffer) -> c_int;
type ProbeFn = unsafe extern "C" fn(u32, *const u8, usize, u32, u32, u32, *mut RawInfo) -> c_int;
type ProbeAnyFn = unsafe extern "C" fn(u32, *const u8, usize, usize, usize, *mut RawInfo) -> c_int;
type DecodeFn = unsafe extern "C" fn(u32, *const u8, usize, u32, u32, u32, *mut u8, usize) -> c_int;
type FreeFn = unsafe extern "C" fn(*mut RawBuffer);
type VersionFn = unsafe extern "C" fn() -> u32;
const BRIDGE_WORKING_ESTIMATE: usize = 64 << 20;

struct NativeBuffer<'a> {
    raw: &'a mut RawBuffer,
    free: FreeFn,
}
impl Drop for NativeBuffer<'_> {
    fn drop(&mut self) {
        unsafe { (self.free)(self.raw) }
    }
}

#[derive(Debug)]
pub enum SpatialProviderError {
    NonAbsolutePath(PathBuf),
    MissingLibrary(PathBuf),
    InvalidLibraryPath,
    LoadFailed,
    MissingSymbol(&'static str),
    Version,
    InvalidImage,
    ResourceLimit,
    BridgeStatus(c_int),
    InvalidBridgeBuffer,
}
impl std::fmt::Display for SpatialProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "spatial provider error: {self:?}")
    }
}
impl std::error::Error for SpatialProviderError {}

impl From<DynamicLibraryError> for SpatialProviderError {
    fn from(error: DynamicLibraryError) -> Self {
        match error {
            DynamicLibraryError::InvalidPath => Self::InvalidLibraryPath,
            DynamicLibraryError::LoadFailed => Self::LoadFailed,
            DynamicLibraryError::MissingSymbol(name) => Self::MissingSymbol(name),
        }
    }
}

pub struct SpatialProvider {
    _library: DynamicLibrary,
    encode: EncodeFn,
    probe: ProbeFn,
    probe_any: ProbeAnyFn,
    decode: DecodeFn,
    free: FreeFn,
    _charls_version: VersionFn,
    _openjpeg_version: VersionFn,
    max_output_bytes: usize,
}

/// Dimensions recovered from an embedded JPEG-LS/JPEG 2000 codestream. CIXI1
/// stores the region byte length but no shape, so callers must obtain this
/// bounded result before they allocate a canonical u16 output buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpatialImageInfo {
    pub width: usize,
    pub height: usize,
    pub bits_per_sample: u32,
}
impl SpatialProvider {
    /// CIX-owned maximum returned payload size configured for this bridge.
    /// Envelope builders reserve against this cap before calling native encode.
    pub fn max_output_bytes(&self) -> usize {
        self.max_output_bytes
    }

    pub fn load_package_bridge(
        path: &Path,
        max_output_bytes: usize,
    ) -> Result<Self, SpatialProviderError> {
        if !path.is_absolute() {
            return Err(SpatialProviderError::NonAbsolutePath(path.to_path_buf()));
        }
        if max_output_bytes == 0 {
            return Err(SpatialProviderError::ResourceLimit);
        }
        if !std::fs::metadata(path)
            .map_err(|_| SpatialProviderError::MissingLibrary(path.to_path_buf()))?
            .is_file()
        {
            return Err(SpatialProviderError::MissingLibrary(path.to_path_buf()));
        }
        let library = DynamicLibrary::open(path)?;
        let encode = unsafe { library.symbol::<EncodeFn>("cix_spatial_encode_u16_gray")? };
        let probe = unsafe { library.symbol::<ProbeFn>("cix_spatial_probe_gray")? };
        let probe_any = unsafe { library.symbol::<ProbeAnyFn>("cix_spatial_probe_any_gray")? };
        let decode = unsafe { library.symbol::<DecodeFn>("cix_spatial_decode_u16_gray")? };
        let free = unsafe { library.symbol::<FreeFn>("cix_spatial_free_buffer")? };
        let charls = unsafe { library.symbol::<VersionFn>("cix_spatial_charls_version")? };
        let openjpeg = unsafe { library.symbol::<VersionFn>("cix_spatial_openjpeg_version")? };
        // Pinned SDK receipts establish 2.4.2 and 2.5.0. A zero result means
        // the loaded DSO is not a usable CIX bridge.
        if unsafe { charls() } != 20402 || unsafe { openjpeg() } != 20500 {
            return Err(SpatialProviderError::Version);
        }
        Ok(Self {
            _library: library,
            encode,
            probe,
            probe_any,
            decode,
            free,
            _charls_version: charls,
            _openjpeg_version: openjpeg,
            max_output_bytes,
        })
    }
    fn bytes(width: usize, height: usize) -> Result<usize, SpatialProviderError> {
        if width == 0 || height == 0 {
            return Err(SpatialProviderError::InvalidImage);
        }
        width
            .checked_mul(height)
            .and_then(|n| n.checked_mul(2))
            .ok_or(SpatialProviderError::ResourceLimit)
    }
    fn encode_budget(
        input: usize,
        output_ceiling: usize,
        memory: usize,
    ) -> Result<(), SpatialProviderError> {
        input
            .checked_mul(16)
            .and_then(|n| n.checked_add(BRIDGE_WORKING_ESTIMATE))
            .and_then(|n| n.checked_add(output_ceiling))
            .and_then(|n| n.checked_add(output_ceiling))
            .filter(|need| *need <= memory)
            .map(|_| ())
            .ok_or(SpatialProviderError::ResourceLimit)
    }
    fn decode_budget(
        input: usize,
        output: usize,
        memory: usize,
    ) -> Result<(), SpatialProviderError> {
        input
            .checked_add(
                output
                    .checked_mul(16)
                    .ok_or(SpatialProviderError::ResourceLimit)?,
            )
            .and_then(|n| n.checked_add(BRIDGE_WORKING_ESTIMATE))
            .filter(|need| *need <= memory)
            .map(|_| ())
            .ok_or(SpatialProviderError::ResourceLimit)
    }
    pub fn encode(
        &self,
        codec: SpatialCodec,
        pixels: &[u8],
        width: usize,
        height: usize,
        bits: u32,
        memory: usize,
    ) -> Result<Vec<u8>, SpatialProviderError> {
        self.encode_with_output_limit(
            codec,
            pixels,
            width,
            height,
            bits,
            self.max_output_bytes,
            memory,
        )
    }

    /// Encode with a tighter per-call cap than the package-wide cap. Envelope
    /// builders use this to admit an archive whose frame cap is smaller than
    /// the installed bridge's general output ceiling.
    // The public bridge ABI carries dimensions, precision, and both caps separately.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_with_output_limit(
        &self,
        codec: SpatialCodec,
        pixels: &[u8],
        width: usize,
        height: usize,
        bits: u32,
        output_cap: usize,
        memory: usize,
    ) -> Result<Vec<u8>, SpatialProviderError> {
        let expected = Self::bytes(width, height)?;
        if pixels.len() != expected
            || !(1..=16).contains(&bits)
            || output_cap == 0
            || output_cap > self.max_output_bytes
        {
            return Err(SpatialProviderError::InvalidImage);
        }
        let maximum = pixels
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| u16::from_le_bytes([p[0], p[1]]))
            .max()
            .unwrap_or(0);
        if bits < 16 && u32::from(maximum) >= (1_u32 << bits) {
            return Err(SpatialProviderError::InvalidImage);
        }
        Self::encode_budget(pixels.len(), output_cap, memory)?;
        let width = u32::try_from(width).map_err(|_| SpatialProviderError::InvalidImage)?;
        let height = u32::try_from(height).map_err(|_| SpatialProviderError::InvalidImage)?;
        let mut out = RawBuffer {
            data: std::ptr::null_mut(),
            size: 0,
        };
        let status = unsafe {
            (self.encode)(
                codec as u32,
                pixels.as_ptr(),
                pixels.len(),
                width,
                height,
                bits,
                output_cap,
                &mut out,
            )
        };
        if status != 0 {
            if !out.data.is_null() {
                unsafe { (self.free)(&mut out) }
            }
            return Err(SpatialProviderError::BridgeStatus(status));
        }
        if out.data.is_null()
            || out.size == 0
            || out.size > output_cap
            || out.size > isize::MAX as usize
        {
            unsafe { (self.free)(&mut out) };
            return Err(SpatialProviderError::InvalidBridgeBuffer);
        }
        let guard = NativeBuffer {
            raw: &mut out,
            free: self.free,
        };
        let mut result = Vec::new();
        result
            .try_reserve_exact(guard.raw.size)
            .map_err(|_| SpatialProviderError::ResourceLimit)?;
        if result.capacity() > output_cap {
            return Err(SpatialProviderError::ResourceLimit);
        }
        unsafe {
            result.extend_from_slice(std::slice::from_raw_parts(guard.raw.data, guard.raw.size))
        };
        Ok(result)
    }
    pub fn decode(
        &self,
        codec: SpatialCodec,
        payload: &[u8],
        width: usize,
        height: usize,
        bits: u32,
        memory: usize,
    ) -> Result<Vec<u8>, SpatialProviderError> {
        let expected = Self::bytes(width, height)?;
        if payload.is_empty() || !(1..=16).contains(&bits) {
            return Err(SpatialProviderError::ResourceLimit);
        }
        Self::decode_budget(payload.len(), expected, memory)?;
        let width = u32::try_from(width).map_err(|_| SpatialProviderError::InvalidImage)?;
        let height = u32::try_from(height).map_err(|_| SpatialProviderError::InvalidImage)?;
        let mut info = RawInfo {
            width: 0,
            height: 0,
            bits_per_sample: 0,
        };
        let status = unsafe {
            (self.probe)(
                codec as u32,
                payload.as_ptr(),
                payload.len(),
                width,
                height,
                bits,
                &mut info,
            )
        };
        if status != 0 {
            return Err(SpatialProviderError::BridgeStatus(status));
        }
        if info.width != width || info.height != height || info.bits_per_sample != bits {
            return Err(SpatialProviderError::InvalidImage);
        }
        let mut output = Vec::new();
        output
            .try_reserve_exact(expected)
            .map_err(|_| SpatialProviderError::ResourceLimit)?;
        output.resize(expected, 0);
        let status = unsafe {
            (self.decode)(
                codec as u32,
                payload.as_ptr(),
                payload.len(),
                width,
                height,
                bits,
                output.as_mut_ptr(),
                output.len(),
            )
        };
        if status != 0 {
            return Err(SpatialProviderError::BridgeStatus(status));
        };
        Ok(output)
    }

    /// Recover the one-plane, unsigned shape of an embedded historical
    /// codestream. `max_output_bytes` is the paid CIXI1 region byte count;
    /// probe limits are enforced by the C bridge before Rust allocates output.
    pub fn probe_embedded(
        &self,
        codec: SpatialCodec,
        payload: &[u8],
        max_output_bytes: usize,
    ) -> Result<SpatialImageInfo, SpatialProviderError> {
        if payload.is_empty() || max_output_bytes < 2 || !max_output_bytes.is_multiple_of(2) {
            return Err(SpatialProviderError::InvalidImage);
        }
        let mut info = RawInfo {
            width: 0,
            height: 0,
            bits_per_sample: 0,
        };
        let status = unsafe {
            (self.probe_any)(
                codec as u32,
                payload.as_ptr(),
                payload.len(),
                max_output_bytes / 2,
                max_output_bytes,
                &mut info,
            )
        };
        if status != 0 {
            return Err(SpatialProviderError::BridgeStatus(status));
        }
        let width = usize::try_from(info.width).map_err(|_| SpatialProviderError::InvalidImage)?;
        let height =
            usize::try_from(info.height).map_err(|_| SpatialProviderError::InvalidImage)?;
        let exact = Self::bytes(width, height)?;
        if exact > max_output_bytes || info.bits_per_sample == 0 || info.bits_per_sample > 16 {
            return Err(SpatialProviderError::InvalidImage);
        }
        Ok(SpatialImageInfo {
            width,
            height,
            bits_per_sample: info.bits_per_sample,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conservative_admission_counts_all_cix_owned_encode_copies() {
        assert!(SpatialProvider::encode_budget(4, 8, BRIDGE_WORKING_ESTIMATE + 79).is_err());
        assert!(SpatialProvider::encode_budget(4, 8, BRIDGE_WORKING_ESTIMATE + 80).is_ok());
    }

    #[test]
    fn decode_admission_requires_working_estimate_before_output_allocation() {
        assert!(SpatialProvider::decode_budget(4, 8, BRIDGE_WORKING_ESTIMATE + 131).is_err());
        assert!(SpatialProvider::decode_budget(4, 8, BRIDGE_WORKING_ESTIMATE + 132).is_ok());
        assert!(SpatialProvider::bytes(0, 1).is_err());
    }

    #[test]
    fn packaged_bridge_roundtrips_tiny_lossless_spatial_codecs() {
        let Some(path) = std::env::var_os("CIX_TEST_SPATIAL_BRIDGE") else {
            return;
        };
        let provider = SpatialProvider::load_package_bridge(Path::new(&path), 8 << 20)
            .expect("CIX_TEST_SPATIAL_BRIDGE must name the staged spatial bridge");
        for (bits, values) in [
            (8, [0_u16, 1, 127, 255]),
            (12, [0, 1, 0x7ff, 0xfff]),
            (16, [0, 1, 255, 0x1234]),
        ] {
            let pixels: Vec<u8> = values.into_iter().flat_map(u16::to_le_bytes).collect();
            for codec in [SpatialCodec::JpegLs, SpatialCodec::Jpeg2000] {
                let payload = provider
                    .encode(codec, &pixels, 2, 2, bits, 128 << 20)
                    .unwrap_or_else(|error| {
                        panic!("native spatial encode {codec:?} bits={bits}: {error}")
                    });
                assert_eq!(
                    provider
                        .probe_embedded(codec, &payload, pixels.len())
                        .unwrap_or_else(|error| panic!(
                            "native spatial embedded probe {codec:?} bits={bits}: {error}"
                        )),
                    SpatialImageInfo {
                        width: 2,
                        height: 2,
                        bits_per_sample: bits,
                    }
                );
                assert!(provider
                    .probe_embedded(codec, &payload, pixels.len() - 2)
                    .is_err());
                assert_eq!(
                    provider
                        .decode(codec, &payload, 2, 2, bits, 128 << 20)
                        .unwrap_or_else(|error| panic!(
                            "native spatial decode {codec:?} bits={bits}: {error}"
                        )),
                    pixels
                );
                assert!(provider
                    .decode(codec, &payload, 3, 2, bits, 128 << 20)
                    .is_err());
                assert!(provider
                    .decode(codec, &payload[..payload.len() - 1], 2, 2, bits, 128 << 20)
                    .is_err());
                let mut trailing = payload.clone();
                trailing.push(0);
                assert!(provider
                    .decode(codec, &trailing, 2, 2, bits, 128 << 20)
                    .is_err());
                if codec == SpatialCodec::Jpeg2000 {
                    let mut forged_eoc = payload.clone();
                    forged_eoc.extend_from_slice(&[0, 0, 0xff, 0xd9]);
                    assert!(provider
                        .decode(codec, &forged_eoc, 2, 2, bits, 128 << 20)
                        .is_err());
                }
            }
        }

        // Fixed 64x64 patterns exercise packet data beyond the tiny-image
        // path. Keep these deterministic so native failures are reproducible.
        let samples = 64 * 64;
        let patterns: Vec<(&str, u32, Vec<u16>)> = vec![
            ("constant", 12, vec![0x0a55; samples]),
            ("ones", 8, vec![1; samples]),
            (
                "wave",
                12,
                (0..samples)
                    .map(|index| ((index * 37 + (index / 64) * 113) & 0x0fff) as u16)
                    .collect(),
            ),
            (
                "pseudo-random",
                16,
                std::iter::successors(Some(0x9e37_79b9_u32), |state| {
                    Some(state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223))
                })
                .take(samples)
                .map(|state| (state >> 16) as u16)
                .collect(),
            ),
        ];
        for (name, bits, values) in patterns {
            let pixels: Vec<u8> = values.into_iter().flat_map(u16::to_le_bytes).collect();
            for codec in [SpatialCodec::JpegLs, SpatialCodec::Jpeg2000] {
                let payload = provider
                    .encode(codec, &pixels, 64, 64, bits, 128 << 20)
                    .unwrap_or_else(|error| {
                        panic!("native spatial encode {codec:?} {name} bits={bits}: {error}")
                    });
                assert_eq!(
                    provider
                        .decode(codec, &payload, 64, 64, bits, 128 << 20)
                        .unwrap_or_else(|error| panic!(
                            "native spatial decode {codec:?} {name} bits={bits}: {error}"
                        )),
                    pixels,
                    "native spatial round trip {codec:?} {name} bits={bits}"
                );
            }
        }
    }
}
