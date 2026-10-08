//! Package-private libjxl provider for the retained CIX image frames.
//!
//! The provider loads one explicitly supplied absolute CIX bridge path. It does
//! not inspect environment variables, PATH, the system library cache, or a
//! Python wheel. The library handle is kept for the provider lifetime so its
//! resolved symbols cannot outlive the bridge DSO.

use crate::full_engine::{
    dynamic_library::{DynamicLibrary, DynamicLibraryError},
    volume_lifting::{ImageBackend, JpegxlImageInfo},
};
use crate::jxl_bridge::{
    JxlBridge, JxlBridgeError, JxlDecodeFn, JxlEncodeFn, JxlFreeBufferFn, JxlLibraryVersionFn,
    JxlPlanarDecodeFn, JxlPlanarEncodeFn, JxlPlanarProbeFn, JxlProbeFn, JxlRawBuffer,
};
use std::ffi::c_int;
use std::fmt;
use std::path::{Path, PathBuf};

const JXL_OUTPUT_OVERHEAD: usize = 64 << 20;
type LzmaCompressFn = unsafe extern "C" fn(*const u8, usize, usize, *mut JxlRawBuffer) -> c_int;
type LzmaDecompressFn =
    unsafe extern "C" fn(*const u8, usize, usize, usize, *mut JxlRawBuffer) -> c_int;

#[derive(Debug)]
pub enum JxlProviderError {
    NonAbsolutePath(PathBuf),
    MissingLibrary(PathBuf),
    InvalidLibraryPath,
    LoadFailed,
    MissingSymbol(&'static str),
    Version(String),
    UnsupportedLayout,
    ResourceLimit,
    Bridge(String),
}

impl fmt::Display for JxlProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonAbsolutePath(path) => write!(
                formatter,
                "JXL bridge path is not absolute: {}",
                path.display()
            ),
            Self::MissingLibrary(path) => write!(
                formatter,
                "packaged JXL bridge is unavailable: {}",
                path.display()
            ),
            Self::InvalidLibraryPath => write!(formatter, "JXL bridge path contains a NUL byte"),
            Self::LoadFailed => write!(
                formatter,
                "cannot load packaged JXL bridge or its staged dependencies"
            ),
            Self::MissingSymbol(symbol) => {
                write!(formatter, "packaged JXL bridge lacks symbol {symbol}")
            }
            Self::Version(error) => write!(
                formatter,
                "packaged JXL bridge version check failed: {error}"
            ),
            Self::UnsupportedLayout => write!(
                formatter,
                "JXL provider requires depth-one gray or depth-two-or-greater planar data"
            ),
            Self::ResourceLimit => {
                write!(formatter, "JXL provider request exceeds its resource limit")
            }
            Self::Bridge(error) => write!(formatter, "JXL bridge failed: {error}"),
        }
    }
}

impl std::error::Error for JxlProviderError {}

impl From<DynamicLibraryError> for JxlProviderError {
    fn from(error: DynamicLibraryError) -> Self {
        match error {
            DynamicLibraryError::InvalidPath => Self::InvalidLibraryPath,
            DynamicLibraryError::LoadFailed => Self::LoadFailed,
            DynamicLibraryError::MissingSymbol(name) => Self::MissingSymbol(name),
        }
    }
}

/// A loaded CIX-owned bridge plus the per-call output ceiling admitted by the
/// full engine. The private handle must be declared before the bridge symbols
/// and remains alive until after all provider use is complete.
pub struct JxlProvider {
    _library: DynamicLibrary,
    bridge: JxlBridge,
    lzma_compress: LzmaCompressFn,
    lzma_decompress: LzmaDecompressFn,
    max_jxl_output_bytes: usize,
}

impl JxlProvider {
    /// Load exactly `library_path`; there is no name search or host fallback.
    /// The caller obtains this path from the installed CIX package layout.
    pub fn load_package_bridge(
        library_path: &Path,
        max_jxl_output_bytes: usize,
    ) -> Result<Self, JxlProviderError> {
        if !library_path.is_absolute() {
            return Err(JxlProviderError::NonAbsolutePath(
                library_path.to_path_buf(),
            ));
        }
        if max_jxl_output_bytes == 0 {
            return Err(JxlProviderError::ResourceLimit);
        }
        let metadata = std::fs::metadata(library_path)
            .map_err(|_| JxlProviderError::MissingLibrary(library_path.to_path_buf()))?;
        if !metadata.is_file() {
            return Err(JxlProviderError::MissingLibrary(library_path.to_path_buf()));
        }
        let library = DynamicLibrary::open(library_path)?;
        let encode = unsafe { library.symbol::<JxlEncodeFn>("cix_jxl_encode_gray_2d")? };
        let probe = unsafe { library.symbol::<JxlProbeFn>("cix_jxl_probe_gray_2d")? };
        let decode = unsafe { library.symbol::<JxlDecodeFn>("cix_jxl_decode_gray_2d")? };
        let planar_encode =
            unsafe { library.symbol::<JxlPlanarEncodeFn>("cix_jxl_encode_planar")? };
        let planar_probe = unsafe { library.symbol::<JxlPlanarProbeFn>("cix_jxl_probe_planar")? };
        let planar_decode =
            unsafe { library.symbol::<JxlPlanarDecodeFn>("cix_jxl_decode_planar")? };
        let free = unsafe { library.symbol::<JxlFreeBufferFn>("cix_jxl_free_buffer")? };
        let version = unsafe { library.symbol::<JxlLibraryVersionFn>("cix_jxl_library_version")? };
        let lzma_compress =
            unsafe { library.symbol::<LzmaCompressFn>("cix_lzma_compress_preset9")? };
        let lzma_decompress =
            unsafe { library.symbol::<LzmaDecompressFn>("cix_lzma_decompress_exact")? };
        let bridge = unsafe {
            JxlBridge::from_symbols_with_planar(
                encode,
                probe,
                decode,
                planar_encode,
                planar_probe,
                planar_decode,
                free,
                version,
            )
        };
        bridge
            .verify_pinned_library()
            .map_err(|error| JxlProviderError::Version(error.to_string()))?;
        Ok(Self {
            _library: library,
            bridge,
            lzma_compress,
            lzma_decompress,
            max_jxl_output_bytes,
        })
    }

    fn plane_bytes(height: usize, width: usize) -> Result<usize, JxlProviderError> {
        height
            .checked_mul(width)
            .and_then(|pixels| pixels.checked_mul(2))
            .ok_or(JxlProviderError::ResourceLimit)
    }

    fn output_limit(&self, canonical_input_bytes: usize) -> Result<usize, JxlProviderError> {
        canonical_input_bytes
            .checked_add(JXL_OUTPUT_OVERHEAD)
            .map(|limit| limit.min(self.max_jxl_output_bytes))
            .filter(|limit| *limit > 0)
            .ok_or(JxlProviderError::ResourceLimit)
    }

    /// Reserve the Rust Vec copy before the bridge admits its own exact XZ
    /// output. The C++ bridge receives the remaining allowance and separately
    /// charges its retained source plus output and liblzma workspace.
    fn lzma_bridge_memory(
        source_bytes: usize,
        expected: usize,
        memory: usize,
    ) -> Result<usize, JxlProviderError> {
        let retained = source_bytes
            .checked_add(expected)
            .and_then(|bytes| bytes.checked_add(expected))
            .ok_or(JxlProviderError::ResourceLimit)?;
        if retained > memory {
            return Err(JxlProviderError::ResourceLimit);
        }
        memory
            .checked_sub(expected)
            .ok_or(JxlProviderError::ResourceLimit)
    }

    fn decode_error(error: JxlBridgeError) -> String {
        match error {
            JxlBridgeError::ResourceLimit => JxlProviderError::ResourceLimit.to_string(),
            other => JxlProviderError::Bridge(other.to_string()).to_string(),
        }
    }

    fn take_buffer(&self, mut buffer: JxlRawBuffer) -> Result<Vec<u8>, String> {
        if buffer.data.is_null() || buffer.size == 0 || buffer.size > self.max_jxl_output_bytes {
            unsafe { self.bridge.free_bridge_buffer(&mut buffer) };
            return Err("CIX native metadata bridge returned an invalid buffer".into());
        }
        let result = unsafe { std::slice::from_raw_parts(buffer.data, buffer.size).to_vec() };
        unsafe { self.bridge.free_bridge_buffer(&mut buffer) };
        Ok(result)
    }
}

impl ImageBackend for JxlProvider {
    fn lzma_compress(&self, source: &[u8]) -> Result<Vec<u8>, String> {
        let mut output = JxlRawBuffer {
            data: std::ptr::null_mut(),
            size: 0,
        };
        let bridge_limit = self.max_jxl_output_bytes / 2;
        if bridge_limit == 0 {
            return Err(JxlProviderError::ResourceLimit.to_string());
        }
        let status = unsafe {
            (self.lzma_compress)(source.as_ptr(), source.len(), bridge_limit, &mut output)
        };
        if status != 0 {
            return Err(format!("CIX LZMA compression status {status}"));
        }
        self.take_buffer(output)
    }

    fn lzma_decompress(
        &self,
        source: &[u8],
        expected: usize,
        memory: usize,
    ) -> Result<Vec<u8>, String> {
        let bridge_memory = Self::lzma_bridge_memory(source.len(), expected, memory)
            .map_err(|error| error.to_string())?;
        let mut output = JxlRawBuffer {
            data: std::ptr::null_mut(),
            size: 0,
        };
        // Reserve the Rust-owned copy before asking the bridge to admit its
        // output buffer and liblzma workspace.
        let status = unsafe {
            (self.lzma_decompress)(
                source.as_ptr(),
                source.len(),
                expected,
                bridge_memory,
                &mut output,
            )
        };
        if status != 0 {
            return Err(format!("CIX LZMA decompression status {status}"));
        }
        if output.size != expected {
            unsafe { self.bridge.free_bridge_buffer(&mut output) };
            return Err("CIX LZMA output length mismatch".into());
        }
        if expected == 0 {
            unsafe { self.bridge.free_bridge_buffer(&mut output) };
            return Ok(Vec::new());
        }
        self.take_buffer(output)
    }

    fn jpegxl_encode(
        &self,
        pixels: &[u8],
        depth: usize,
        height: usize,
        width: usize,
        planar: bool,
        effort: u8,
    ) -> Result<Vec<u8>, String> {
        if depth == 0 || (depth == 1 && planar) || (depth > 1 && !planar) {
            return Err(JxlProviderError::UnsupportedLayout.to_string());
        }
        let expected = Self::plane_bytes(height, width)
            .and_then(|plane| {
                plane
                    .checked_mul(depth)
                    .ok_or(JxlProviderError::ResourceLimit)
            })
            .map_err(|error| error.to_string())?;
        if pixels.len() != expected {
            return Err("CIX JXL canonical plane byte count mismatch".into());
        }
        let width = u32::try_from(width).map_err(|_| "CIX JXL width exceeds u32")?;
        let height = u32::try_from(height).map_err(|_| "CIX JXL height exceeds u32")?;
        let limit = self
            .output_limit(pixels.len())
            .map_err(|error| error.to_string())?;
        if depth == 1 {
            self.bridge
                .encode_cix_u16_gray_2d(width, height, u32::from(effort), pixels, limit)
        } else {
            let depth = u32::try_from(depth).map_err(|_| "CIX JXL depth exceeds u32")?;
            self.bridge.encode_cix_u16_planar(
                depth,
                width,
                height,
                u32::from(effort),
                pixels,
                limit,
            )
        }
        .map_err(|error| JxlProviderError::Bridge(error.to_string()).to_string())
    }

    fn jpegxl_probe(
        &self,
        payload: &[u8],
        expected_pixels: usize,
        memory: usize,
    ) -> Result<JpegxlImageInfo, String> {
        let canonical = expected_pixels
            .checked_mul(2)
            .ok_or_else(|| JxlProviderError::ResourceLimit.to_string())?;
        if canonical > memory {
            return Err(JxlProviderError::ResourceLimit.to_string());
        }
        let probed = self
            .bridge
            .probe_gray_2d(payload, expected_pixels)
            .map_err(|error| JxlProviderError::Bridge(error.to_string()).to_string())?;
        Ok(JpegxlImageInfo {
            depth: 1,
            height: probed.geometry.height as usize,
            width: probed.geometry.width as usize,
        })
    }

    fn jpegxl_decode(
        &self,
        payload: &[u8],
        expected: usize,
        depth: usize,
        height: usize,
        width: usize,
        memory: usize,
    ) -> Result<Vec<u8>, String> {
        if depth == 0 {
            return Err(JxlProviderError::UnsupportedLayout.to_string());
        }
        let canonical = Self::plane_bytes(height, width)
            .and_then(|plane| {
                plane
                    .checked_mul(depth)
                    .ok_or(JxlProviderError::ResourceLimit)
            })
            .map_err(|error| error.to_string())?;
        if expected != canonical {
            return Err(JxlProviderError::ResourceLimit.to_string());
        }
        // The bridge checks the retained payload plus its raw transport Vec
        // and canonical Vec before allocating. This is a CIX-owned-buffer
        // bound only; libjxl's internal working set remains outside it.
        let (actual_depth, actual_height, actual_width, decoded) = if depth == 1 {
            let (probed, decoded) = self
                .bridge
                .decode_cix_u16_gray_2d(payload, canonical / 2, memory)
                .map_err(Self::decode_error)?;
            (
                1,
                probed.geometry.height as usize,
                probed.geometry.width as usize,
                decoded,
            )
        } else {
            let expected_pixels = canonical / 2;
            let (probed, decoded) = self
                .bridge
                .decode_cix_u16_planar(payload, depth, expected_pixels, memory)
                .map_err(Self::decode_error)?;
            (
                probed.geometry.depth as usize,
                probed.geometry.height as usize,
                probed.geometry.width as usize,
                decoded,
            )
        };
        if actual_depth != depth || actual_width != width || actual_height != height {
            return Err("CIX JXL decoded geometry differs from paid CIX dimensions".into());
        }
        if decoded.len() != canonical {
            return Err("CIX JXL canonical output byte count mismatch".into());
        }
        Ok(decoded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::full_engine::volume_lifting::{
        decode_multiscale, decode_spatial, decode_volume, encode_multiscale, encode_spatial,
        encode_volume, DicomRegion,
    };

    fn packaged_provider() -> Option<JxlProvider> {
        let path = std::env::var_os("CIX_TEST_JXL_BRIDGE")?;
        Some(
            JxlProvider::load_package_bridge(Path::new(&path), 8 << 20)
                .expect("CIX_TEST_JXL_BRIDGE must name a loadable pinned bridge"),
        )
    }

    fn canonical(values: &[u16]) -> Vec<u8> {
        values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect()
    }

    #[test]
    fn packaged_bridge_roundtrips_canonical_frames() {
        let Some(provider) = packaged_provider() else {
            return;
        };

        let eight_bit = canonical(&[0, 1, 127, 255, 3, 42]);
        let eight_payload = provider
            .jpegxl_encode(&eight_bit, 1, 2, 3, false, 7)
            .expect("8-bit canonical encode");
        assert_eq!(
            provider
                .jpegxl_decode(&eight_payload, eight_bit.len(), 1, 2, 3, 1 << 20)
                .expect("8-bit canonical decode"),
            eight_bit
        );

        let sixteen_bit = canonical(&[0, 1, 255, 256, 0x1234, u16::MAX]);
        let sixteen_payload = provider
            .jpegxl_encode(&sixteen_bit, 1, 2, 3, false, 7)
            .expect("16-bit canonical encode");
        assert_eq!(
            provider
                .jpegxl_decode(&sixteen_payload, sixteen_bit.len(), 1, 2, 3, 1 << 20)
                .expect("16-bit canonical decode"),
            sixteen_bit
        );
        assert!(
            provider.jpegxl_probe(&eight_payload, 7, 1 << 20).is_err(),
            "paid pixel count must reject a valid but wrong shape"
        );
        let uint8_known_buffers = eight_payload
            .len()
            .checked_add(eight_bit.len() / 2)
            .and_then(|bytes| bytes.checked_add(eight_bit.len()))
            .unwrap();
        assert!(
            provider
                .jpegxl_decode(
                    &eight_payload,
                    eight_bit.len(),
                    1,
                    2,
                    3,
                    uint8_known_buffers - 1,
                )
                .unwrap_err()
                .contains("resource limit"),
            "UINT8 raw and canonical buffers must be admitted together"
        );

        let spatial_region = DicomRegion {
            offset: 3,
            length: eight_bit.len(),
            width: 3,
            height: 2,
            slice_height: 2,
        };
        let mut spatial_source = b"pre".to_vec();
        spatial_source.extend_from_slice(&eight_bit);
        spatial_source.extend_from_slice(b"post");
        let spatial =
            encode_spatial(&spatial_source, &spatial_region, &provider).expect("CIXI1 encode");
        assert_eq!(
            decode_spatial(&spatial, spatial_source.len(), 128 << 20, &provider)
                .expect("CIXI1 decode"),
            spatial_source
        );

        let volume_region = DicomRegion {
            offset: 2,
            length: 16,
            width: 2,
            height: 4,
            slice_height: 2,
        };
        let mut volume_source = b"v=".to_vec();
        volume_source.extend_from_slice(&canonical(&[0, 1, 2, 3, 256, 257, 258, 259]));
        volume_source.extend_from_slice(b";end");
        let volume = encode_volume(&volume_source, &volume_region, 1, 7, &provider)
            .expect("supported CIXV1 mode 1 encode");
        assert_eq!(
            decode_volume(&volume, volume_source.len(), 128 << 20, &provider)
                .expect("supported CIXV1 mode 1 decode"),
            volume_source
        );

        // Mode 3's [z, y, x] layout is transported as one gray plane followed
        // by optional channels, never as a flattened 2-D image.
        let planar_region = DicomRegion {
            offset: 2,
            length: 24,
            width: 2,
            height: 6,
            slice_height: 2,
        };
        let mut planar_source = b"p=".to_vec();
        planar_source.extend_from_slice(&canonical(&[
            0, 1, 2, 3, 256, 257, 258, 259, 1024, 1025, 1026, 1027,
        ]));
        planar_source.extend_from_slice(b";end");
        let planar = encode_volume(&planar_source, &planar_region, 3, 7, &provider)
            .expect("CIXV1 mode 3 planar encode");
        assert_eq!(
            decode_volume(&planar, planar_source.len(), 128 << 20, &provider)
                .expect("CIXV1 mode 3 planar decode"),
            planar_source
        );

        let multiscale = encode_multiscale(&planar_source, &planar_region, 1, 1, 0, 7, &provider)
            .expect("CIXI2 planar band encode");
        assert_eq!(
            decode_multiscale(&multiscale, planar_source.len(), 128 << 20, &provider)
                .expect("CIXI2 planar band decode"),
            planar_source
        );
    }

    #[test]
    fn lzma_budget_reserves_bridge_and_rust_output_buffers() {
        // Retained source (4) plus bridge output (8) plus Rust Vec copy (8).
        assert_eq!(JxlProvider::lzma_bridge_memory(4, 8, 20).unwrap(), 12);
        assert!(JxlProvider::lzma_bridge_memory(4, 8, 19).is_err());
        assert!(JxlProvider::lzma_bridge_memory(usize::MAX, 1, usize::MAX).is_err());
    }
}
