//! Typed binding for the CIX-owned libjxl grayscale bridge.
//!
//! The platform loader supplies symbols from the package's sibling
//! `cix_jxl_bridge` library. This module intentionally does not `dlopen`, set
//! environment variables, or select a host libjxl. Registration and package
//! loading remain the responsibility of the full engine.

use std::fmt;
use std::os::raw::c_int;

pub const LIBJXL_0_12_0_VERSION: u32 = 12_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum JxlSampleType {
    Uint8 = 1,
    Uint16 = 2,
}

impl JxlSampleType {
    pub fn bytes_per_sample(self) -> usize {
        match self {
            Self::Uint8 => 1,
            Self::Uint16 => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum JxlBridgeStatus {
    Ok = 0,
    InvalidArgument = 1,
    Unsupported = 2,
    OutputLimit = 3,
    EncodeFailure = 4,
    DecodeFailure = 5,
    AllocationFailure = 6,
    Exception = 7,
}

impl JxlBridgeStatus {
    fn from_raw(value: c_int) -> Option<Self> {
        match value {
            0 => Some(Self::Ok),
            1 => Some(Self::InvalidArgument),
            2 => Some(Self::Unsupported),
            3 => Some(Self::OutputLimit),
            4 => Some(Self::EncodeFailure),
            5 => Some(Self::DecodeFailure),
            6 => Some(Self::AllocationFailure),
            7 => Some(Self::Exception),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JxlGrayGeometry {
    pub width: u32,
    pub height: u32,
    pub bits_per_sample: u32,
    pub sample_type: JxlSampleType,
}

impl JxlGrayGeometry {
    pub fn byte_len(self) -> Result<usize, JxlBridgeError> {
        if self.width == 0 || self.height == 0 || self.bits_per_sample == 0 {
            return Err(JxlBridgeError::InvalidImage(
                "zero image dimension or depth",
            ));
        }
        if self.bits_per_sample > 16
            || (self.sample_type == JxlSampleType::Uint8 && self.bits_per_sample > 8)
            || (self.sample_type == JxlSampleType::Uint16 && self.bits_per_sample <= 8)
        {
            return Err(JxlBridgeError::InvalidImage(
                "incompatible sample type and depth",
            ));
        }
        usize::try_from(self.width)
            .ok()
            .and_then(|width| {
                usize::try_from(self.height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .and_then(|pixels| pixels.checked_mul(self.sample_type.bytes_per_sample()))
            .ok_or(JxlBridgeError::InvalidImage(
                "image byte count overflows usize",
            ))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct JxlGrayImage<'a> {
    pub geometry: JxlGrayGeometry,
    pub effort: u32,
    pub bytes: &'a [u8],
}

impl<'a> JxlGrayImage<'a> {
    pub fn validate(self) -> Result<(), JxlBridgeError> {
        if !(1..=10).contains(&self.effort) {
            return Err(JxlBridgeError::InvalidImage("effort must be in 1..=10"));
        }
        if self.bytes.len() != self.geometry.byte_len()? {
            return Err(JxlBridgeError::InvalidImage(
                "pixel bytes do not match geometry",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JxlBridgeError {
    InvalidImage(&'static str),
    InvalidOutputBuffer,
    ResourceLimit,
    ProbeUnavailable,
    WrongLibraryVersion(u32),
    Status(Option<JxlBridgeStatus>),
}

impl fmt::Display for JxlBridgeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidImage(reason) => write!(formatter, "invalid JXL gray image: {reason}"),
            Self::InvalidOutputBuffer => write!(formatter, "JXL bridge returned an invalid buffer"),
            Self::ResourceLimit => {
                write!(formatter, "JXL bridge exceeds the CIX-owned buffer budget")
            }
            Self::ProbeUnavailable => {
                write!(formatter, "JXL bridge did not provide basic-info probing")
            }
            Self::WrongLibraryVersion(version) => write!(
                formatter,
                "CIX JXL bridge requires libjxl 0.12.0 (12000), loaded {version}"
            ),
            Self::Status(status) => write!(formatter, "CIX JXL bridge returned {status:?}"),
        }
    }
}

impl std::error::Error for JxlBridgeError {}

/// Memory retained by this Rust bridge while restoring a canonical u16 image.
/// It includes the caller's JXL input slice because its owner retains it until
/// the decode returns. It deliberately does not claim to bound libjxl's own
/// internal state.
fn cix_decode_buffer_budget(
    input_bytes: usize,
    raw_bytes: usize,
    canonical_bytes: usize,
    sample_type: JxlSampleType,
) -> Result<usize, JxlBridgeError> {
    let buffers = match sample_type {
        // The raw UINT8 output remains allocated while it is expanded to the
        // canonical u16 Vec.
        JxlSampleType::Uint8 => raw_bytes.checked_add(canonical_bytes),
        // The raw UINT16 Vec is returned directly as canonical storage.
        JxlSampleType::Uint16 => {
            if raw_bytes != canonical_bytes {
                return Err(JxlBridgeError::InvalidOutputBuffer);
            }
            Some(raw_bytes)
        }
    };
    input_bytes
        .checked_add(buffers.ok_or(JxlBridgeError::ResourceLimit)?)
        .ok_or(JxlBridgeError::ResourceLimit)
}

/// Raw ABI descriptor passed to `cix_jxl_encode_gray_2d`.
///
/// The full-engine loader needs this public type to resolve the C symbol with
/// its real signature. Safe callers use [`JxlGrayImage`] instead.
#[repr(C)]
pub struct JxlRawGrayImage {
    pixels: *const u8,
    pixels_size: usize,
    width: u32,
    height: u32,
    bits_per_sample: u32,
    sample_type: u32,
    effort: u32,
}

/// Raw ABI buffer returned by `cix_jxl_encode_gray_2d`.
///
/// It is released only through the matching `JxlFreeBufferFn` from the same
/// loaded bridge.
#[repr(C)]
pub struct JxlRawBuffer {
    pub data: *mut u8,
    pub size: usize,
}

/// Raw ABI basic-info response from `cix_jxl_probe_gray_2d`.
#[repr(C)]
pub struct JxlRawGrayInfo {
    width: u32,
    height: u32,
    bits_per_sample: u32,
    sample_type: u32,
}

#[repr(C)]
pub struct JxlRawPlanarImage {
    pixels: *const u8,
    pixels_size: usize,
    depth: u32,
    width: u32,
    height: u32,
    bits_per_sample: u32,
    sample_type: u32,
    effort: u32,
}

#[repr(C)]
pub struct JxlRawPlanarInfo {
    depth: u32,
    width: u32,
    height: u32,
    bits_per_sample: u32,
    sample_type: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JxlPlanarGeometry {
    pub depth: u32,
    pub width: u32,
    pub height: u32,
    pub bits_per_sample: u32,
    pub sample_type: JxlSampleType,
}

impl JxlPlanarGeometry {
    pub fn byte_len(self) -> Result<usize, JxlBridgeError> {
        if self.depth < 2 {
            return Err(JxlBridgeError::InvalidImage(
                "planar depth must be at least two",
            ));
        }
        let plane = JxlGrayGeometry {
            width: self.width,
            height: self.height,
            bits_per_sample: self.bits_per_sample,
            sample_type: self.sample_type,
        }
        .byte_len()?;
        usize::try_from(self.depth)
            .ok()
            .and_then(|depth| plane.checked_mul(depth))
            .ok_or(JxlBridgeError::InvalidImage(
                "planar image byte count overflows usize",
            ))
    }
    pub fn pixel_count(self) -> Result<usize, JxlBridgeError> {
        usize::try_from(self.depth)
            .ok()
            .and_then(|depth| {
                usize::try_from(self.width).ok().and_then(|width| {
                    usize::try_from(self.height)
                        .ok()
                        .and_then(|height| depth.checked_mul(width)?.checked_mul(height))
                })
            })
            .ok_or(JxlBridgeError::InvalidImage(
                "planar pixel count overflows usize",
            ))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JxlProbedPlanarImage {
    pub geometry: JxlPlanarGeometry,
}

/// Bounded geometry established before caller pixel-buffer allocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JxlProbedGrayImage {
    pub geometry: JxlGrayGeometry,
}

/// Resolved `cix_jxl_encode_gray_2d` symbol type.
pub type JxlEncodeFn =
    unsafe extern "C" fn(*const JxlRawGrayImage, usize, *mut JxlRawBuffer) -> c_int;
/// Resolved `cix_jxl_probe_gray_2d` symbol type.
pub type JxlProbeFn = unsafe extern "C" fn(*const u8, usize, usize, *mut JxlRawGrayInfo) -> c_int;
/// Resolved `cix_jxl_decode_gray_2d` symbol type.
pub type JxlDecodeFn =
    unsafe extern "C" fn(*const u8, usize, u32, u32, u32, u32, *mut u8, usize) -> c_int;
pub type JxlPlanarEncodeFn =
    unsafe extern "C" fn(*const JxlRawPlanarImage, usize, *mut JxlRawBuffer) -> c_int;
pub type JxlPlanarProbeFn =
    unsafe extern "C" fn(*const u8, usize, u32, usize, *mut JxlRawPlanarInfo) -> c_int;
pub type JxlPlanarDecodeFn =
    unsafe extern "C" fn(*const u8, usize, u32, u32, u32, u32, u32, *mut u8, usize) -> c_int;
/// Resolved `cix_jxl_free_buffer` symbol type.
pub type JxlFreeBufferFn = unsafe extern "C" fn(*mut JxlRawBuffer);
/// Resolved `cix_jxl_library_version` symbol type.
pub type JxlLibraryVersionFn = unsafe extern "C" fn() -> u32;

/// Functions resolved by the full engine from the CIX-packaged bridge.
#[derive(Clone, Copy)]
pub struct JxlBridge {
    encode: JxlEncodeFn,
    probe: Option<JxlProbeFn>,
    decode: JxlDecodeFn,
    planar_encode: Option<JxlPlanarEncodeFn>,
    planar_probe: Option<JxlPlanarProbeFn>,
    planar_decode: Option<JxlPlanarDecodeFn>,
    free_buffer: JxlFreeBufferFn,
    library_version: JxlLibraryVersionFn,
}

impl JxlBridge {
    /// Release an allocation returned by any CIX bridge entry point.
    ///
    /// # Safety
    ///
    /// `buffer` must describe a live allocation returned by this same bridge
    /// and must not already have been released. It must not be accessed by any
    /// other thread while this call releases its storage.
    pub unsafe fn free_bridge_buffer(self, buffer: &mut JxlRawBuffer) {
        unsafe { (self.free_buffer)(buffer) };
    }
    /// # Safety
    ///
    /// All supplied pointers must come from the matching CIX-owned
    /// `cix_jxl_bridge` build. The engine must not bind an arbitrary host DSO.
    pub const unsafe fn from_symbols(
        encode: JxlEncodeFn,
        decode: JxlDecodeFn,
        free_buffer: JxlFreeBufferFn,
        library_version: JxlLibraryVersionFn,
    ) -> Self {
        Self {
            encode,
            probe: None,
            decode,
            planar_encode: None,
            planar_probe: None,
            planar_decode: None,
            free_buffer,
            library_version,
        }
    }

    /// # Safety
    ///
    /// As [`Self::from_symbols`], with the matching CIX-owned bounded
    /// `cix_jxl_probe_gray_2d` symbol from the same bridge DSO.
    pub const unsafe fn from_symbols_with_probe(
        encode: JxlEncodeFn,
        probe: JxlProbeFn,
        decode: JxlDecodeFn,
        free_buffer: JxlFreeBufferFn,
        library_version: JxlLibraryVersionFn,
    ) -> Self {
        Self {
            encode,
            probe: Some(probe),
            decode,
            planar_encode: None,
            planar_probe: None,
            planar_decode: None,
            free_buffer,
            library_version,
        }
    }

    /// # Safety
    ///
    /// Every function must originate in the same CIX-owned package bridge.
    #[allow(
        clippy::too_many_arguments,
        reason = "the bridge ABI resolves eight independently typed symbols from one DSO"
    )]
    pub const unsafe fn from_symbols_with_planar(
        encode: JxlEncodeFn,
        probe: JxlProbeFn,
        decode: JxlDecodeFn,
        planar_encode: JxlPlanarEncodeFn,
        planar_probe: JxlPlanarProbeFn,
        planar_decode: JxlPlanarDecodeFn,
        free_buffer: JxlFreeBufferFn,
        library_version: JxlLibraryVersionFn,
    ) -> Self {
        Self {
            encode,
            probe: Some(probe),
            decode,
            planar_encode: Some(planar_encode),
            planar_probe: Some(planar_probe),
            planar_decode: Some(planar_decode),
            free_buffer,
            library_version,
        }
    }

    pub fn verify_pinned_library(self) -> Result<(), JxlBridgeError> {
        let version = unsafe { (self.library_version)() };
        if version == LIBJXL_0_12_0_VERSION {
            Ok(())
        } else {
            Err(JxlBridgeError::WrongLibraryVersion(version))
        }
    }

    /// Read and validate basic image information without supplying a pixel
    /// output buffer. `expected_pixels` is paid CIX framing information, not
    /// untrusted JXL metadata.
    pub fn probe_gray_2d(
        self,
        input: &[u8],
        expected_pixels: usize,
    ) -> Result<JxlProbedGrayImage, JxlBridgeError> {
        if input.is_empty() || expected_pixels == 0 {
            return Err(JxlBridgeError::InvalidOutputBuffer);
        }
        self.verify_pinned_library()?;
        let probe = self.probe.ok_or(JxlBridgeError::ProbeUnavailable)?;
        let mut raw = JxlRawGrayInfo {
            width: 0,
            height: 0,
            bits_per_sample: 0,
            sample_type: 0,
        };
        let status = unsafe { (probe)(input.as_ptr(), input.len(), expected_pixels, &mut raw) };
        if JxlBridgeStatus::from_raw(status) != Some(JxlBridgeStatus::Ok) {
            return Err(JxlBridgeError::Status(JxlBridgeStatus::from_raw(status)));
        }
        let sample_type = match raw.sample_type {
            1 => JxlSampleType::Uint8,
            2 => JxlSampleType::Uint16,
            _ => return Err(JxlBridgeError::InvalidOutputBuffer),
        };
        let geometry = JxlGrayGeometry {
            width: raw.width,
            height: raw.height,
            bits_per_sample: raw.bits_per_sample,
            sample_type,
        };
        geometry.byte_len()?;
        let pixel_count = usize::try_from(geometry.width)
            .ok()
            .and_then(|width| {
                usize::try_from(geometry.height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .ok_or(JxlBridgeError::InvalidOutputBuffer)?;
        if pixel_count != expected_pixels {
            return Err(JxlBridgeError::InvalidOutputBuffer);
        }
        Ok(JxlProbedGrayImage { geometry })
    }

    /// Encode a CIX canonical little-endian u16 grayscale plane. The frozen
    /// image wrappers choose UINT8 when its observed maximum fits eight bits;
    /// otherwise they retain UINT16 and record the exact significant depth.
    pub fn encode_cix_u16_gray_2d(
        self,
        width: u32,
        height: u32,
        effort: u32,
        canonical_le_u16: &[u8],
        max_output_bytes: usize,
    ) -> Result<Vec<u8>, JxlBridgeError> {
        let pixels = usize::try_from(width)
            .ok()
            .and_then(|w| usize::try_from(height).ok().and_then(|h| w.checked_mul(h)))
            .ok_or(JxlBridgeError::InvalidImage(
                "image pixel count overflows usize",
            ))?;
        let expected = pixels.checked_mul(2).ok_or(JxlBridgeError::InvalidImage(
            "image byte count overflows usize",
        ))?;
        if canonical_le_u16.len() != expected {
            return Err(JxlBridgeError::InvalidImage(
                "canonical u16 bytes do not match geometry",
            ));
        }
        let maximum = canonical_le_u16
            .as_chunks::<2>()
            .0
            .iter()
            .map(|sample| u16::from_le_bytes(*sample))
            .max()
            .unwrap_or(0);
        let bits_per_sample = (u16::BITS - maximum.leading_zeros()).max(1);
        if bits_per_sample <= 8 {
            let compact: Vec<u8> = canonical_le_u16
                .as_chunks::<2>()
                .0
                .iter()
                .map(|sample| sample[0])
                .collect();
            self.encode_gray_2d(
                JxlGrayImage {
                    geometry: JxlGrayGeometry {
                        width,
                        height,
                        bits_per_sample,
                        sample_type: JxlSampleType::Uint8,
                    },
                    effort,
                    bytes: &compact,
                },
                max_output_bytes,
            )
        } else {
            self.encode_gray_2d(
                JxlGrayImage {
                    geometry: JxlGrayGeometry {
                        width,
                        height,
                        bits_per_sample,
                        sample_type: JxlSampleType::Uint16,
                    },
                    effort,
                    bytes: canonical_le_u16,
                },
                max_output_bytes,
            )
        }
    }

    pub fn encode_gray_2d(
        self,
        image: JxlGrayImage<'_>,
        max_output_bytes: usize,
    ) -> Result<Vec<u8>, JxlBridgeError> {
        image.validate()?;
        if max_output_bytes == 0 {
            return Err(JxlBridgeError::InvalidOutputBuffer);
        }
        self.verify_pinned_library()?;
        let request = JxlRawGrayImage {
            pixels: image.bytes.as_ptr(),
            pixels_size: image.bytes.len(),
            width: image.geometry.width,
            height: image.geometry.height,
            bits_per_sample: image.geometry.bits_per_sample,
            sample_type: image.geometry.sample_type as u32,
            effort: image.effort,
        };
        let mut output = JxlRawBuffer {
            data: std::ptr::null_mut(),
            size: 0,
        };
        let status = unsafe { (self.encode)(&request, max_output_bytes, &mut output) };
        let parsed = JxlBridgeStatus::from_raw(status);
        if parsed != Some(JxlBridgeStatus::Ok) {
            if !output.data.is_null() {
                unsafe { (self.free_buffer)(&mut output) };
            }
            return Err(JxlBridgeError::Status(parsed));
        }
        if output.data.is_null()
            || output.size == 0
            || output.size > max_output_bytes
            || output.size > isize::MAX as usize
        {
            unsafe { (self.free_buffer)(&mut output) };
            return Err(JxlBridgeError::InvalidOutputBuffer);
        }
        let encoded = unsafe { std::slice::from_raw_parts(output.data, output.size).to_vec() };
        unsafe { (self.free_buffer)(&mut output) };
        Ok(encoded)
    }

    /// Decode a bounded gray 2-D payload to CIX canonical little-endian u16
    /// samples. It probes and checks the paid pixel count before allocating the
    /// raw JXL output buffer, then expands UINT8 samples exactly as the frozen
    /// Python route's `astype("<u2")` conversion does. `memory` bounds the
    /// retained input plus Rust-owned raw/canonical output buffers only.
    pub fn decode_cix_u16_gray_2d(
        self,
        input: &[u8],
        expected_pixels: usize,
        memory: usize,
    ) -> Result<(JxlProbedGrayImage, Vec<u8>), JxlBridgeError> {
        let probed = self.probe_gray_2d(input, expected_pixels)?;
        let raw_bytes = probed.geometry.byte_len()?;
        let canonical_size = expected_pixels
            .checked_mul(2)
            .ok_or(JxlBridgeError::ResourceLimit)?;
        if cix_decode_buffer_budget(
            input.len(),
            raw_bytes,
            canonical_size,
            probed.geometry.sample_type,
        )? > memory
        {
            return Err(JxlBridgeError::ResourceLimit);
        }
        let mut raw = vec![0_u8; raw_bytes];
        self.decode_gray_2d(input, probed.geometry, &mut raw)?;
        let canonical = match probed.geometry.sample_type {
            JxlSampleType::Uint8 => raw.into_iter().flat_map(|sample| [sample, 0]).collect(),
            JxlSampleType::Uint16 => raw,
        };
        if canonical.len() != canonical_size {
            return Err(JxlBridgeError::InvalidOutputBuffer);
        }
        Ok((probed, canonical))
    }

    pub fn decode_gray_2d(
        self,
        input: &[u8],
        geometry: JxlGrayGeometry,
        output: &mut [u8],
    ) -> Result<(), JxlBridgeError> {
        let expected = geometry.byte_len()?;
        if input.is_empty() || output.len() != expected {
            return Err(JxlBridgeError::InvalidOutputBuffer);
        }
        self.verify_pinned_library()?;
        let status = unsafe {
            (self.decode)(
                input.as_ptr(),
                input.len(),
                geometry.width,
                geometry.height,
                geometry.bits_per_sample,
                geometry.sample_type as u32,
                output.as_mut_ptr(),
                output.len(),
            )
        };
        match JxlBridgeStatus::from_raw(status) {
            Some(JxlBridgeStatus::Ok) => Ok(()),
            other => Err(JxlBridgeError::Status(other)),
        }
    }

    fn planar_fns(
        self,
    ) -> Result<(JxlPlanarEncodeFn, JxlPlanarProbeFn, JxlPlanarDecodeFn), JxlBridgeError> {
        match (self.planar_encode, self.planar_probe, self.planar_decode) {
            (Some(encode), Some(probe), Some(decode)) => Ok((encode, probe, decode)),
            _ => Err(JxlBridgeError::ProbeUnavailable),
        }
    }

    pub fn probe_planar(
        self,
        input: &[u8],
        expected_depth: usize,
        expected_pixels: usize,
    ) -> Result<JxlProbedPlanarImage, JxlBridgeError> {
        if input.is_empty() || expected_depth < 2 || expected_pixels == 0 {
            return Err(JxlBridgeError::InvalidOutputBuffer);
        }
        let depth =
            u32::try_from(expected_depth).map_err(|_| JxlBridgeError::InvalidOutputBuffer)?;
        self.verify_pinned_library()?;
        let (_, probe, _) = self.planar_fns()?;
        let mut raw = JxlRawPlanarInfo {
            depth: 0,
            width: 0,
            height: 0,
            bits_per_sample: 0,
            sample_type: 0,
        };
        let status = unsafe {
            probe(
                input.as_ptr(),
                input.len(),
                depth,
                expected_pixels,
                &mut raw,
            )
        };
        if JxlBridgeStatus::from_raw(status) != Some(JxlBridgeStatus::Ok) {
            return Err(JxlBridgeError::Status(JxlBridgeStatus::from_raw(status)));
        }
        let sample_type = match raw.sample_type {
            1 => JxlSampleType::Uint8,
            2 => JxlSampleType::Uint16,
            _ => return Err(JxlBridgeError::InvalidOutputBuffer),
        };
        let geometry = JxlPlanarGeometry {
            depth: raw.depth,
            width: raw.width,
            height: raw.height,
            bits_per_sample: raw.bits_per_sample,
            sample_type,
        };
        if geometry.depth != depth || geometry.pixel_count()? != expected_pixels {
            return Err(JxlBridgeError::InvalidOutputBuffer);
        }
        geometry.byte_len()?;
        Ok(JxlProbedPlanarImage { geometry })
    }

    /// Encode CIX canonical little-endian u16 Z/H/W samples using the frozen
    /// gray-plus-optional-channel layout. One significant depth is used for
    /// every plane, as imagecodecs does for a planar uint array.
    pub fn encode_cix_u16_planar(
        self,
        depth: u32,
        width: u32,
        height: u32,
        effort: u32,
        canonical_le_u16: &[u8],
        max_output_bytes: usize,
    ) -> Result<Vec<u8>, JxlBridgeError> {
        let pixels = usize::try_from(depth)
            .ok()
            .and_then(|d| {
                usize::try_from(width).ok().and_then(|w| {
                    usize::try_from(height)
                        .ok()
                        .and_then(|h| d.checked_mul(w)?.checked_mul(h))
                })
            })
            .ok_or(JxlBridgeError::InvalidImage(
                "planar pixel count overflows usize",
            ))?;
        if canonical_le_u16.len()
            != pixels.checked_mul(2).ok_or(JxlBridgeError::InvalidImage(
                "planar byte count overflows usize",
            ))?
            || !(1..=10).contains(&effort)
            || max_output_bytes == 0
        {
            return Err(JxlBridgeError::InvalidImage(
                "canonical planar bytes or effort are invalid",
            ));
        }
        let maximum = canonical_le_u16
            .as_chunks::<2>()
            .0
            .iter()
            .map(|sample| u16::from_le_bytes(*sample))
            .max()
            .unwrap_or(0);
        let bits = (u16::BITS - maximum.leading_zeros()).max(1);
        let (sample_type, transport): (JxlSampleType, Vec<u8>) = if bits <= 8 {
            (
                JxlSampleType::Uint8,
                canonical_le_u16
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|sample| sample[0])
                    .collect(),
            )
        } else {
            (JxlSampleType::Uint16, canonical_le_u16.to_vec())
        };
        let geometry = JxlPlanarGeometry {
            depth,
            width,
            height,
            bits_per_sample: bits,
            sample_type,
        };
        if transport.len() != geometry.byte_len()? {
            return Err(JxlBridgeError::InvalidImage(
                "planar transport size mismatch",
            ));
        }
        self.verify_pinned_library()?;
        let (encode, _, _) = self.planar_fns()?;
        let request = JxlRawPlanarImage {
            pixels: transport.as_ptr(),
            pixels_size: transport.len(),
            depth,
            width,
            height,
            bits_per_sample: bits,
            sample_type: sample_type as u32,
            effort,
        };
        let mut output = JxlRawBuffer {
            data: std::ptr::null_mut(),
            size: 0,
        };
        let status = unsafe { encode(&request, max_output_bytes, &mut output) };
        if JxlBridgeStatus::from_raw(status) != Some(JxlBridgeStatus::Ok) {
            if !output.data.is_null() {
                unsafe { (self.free_buffer)(&mut output) };
            }
            return Err(JxlBridgeError::Status(JxlBridgeStatus::from_raw(status)));
        }
        if output.data.is_null()
            || output.size == 0
            || output.size > max_output_bytes
            || output.size > isize::MAX as usize
        {
            unsafe { (self.free_buffer)(&mut output) };
            return Err(JxlBridgeError::InvalidOutputBuffer);
        }
        let encoded = unsafe { std::slice::from_raw_parts(output.data, output.size).to_vec() };
        unsafe { (self.free_buffer)(&mut output) };
        Ok(encoded)
    }

    /// Decode planar transport with the same retained-input and CIX-owned
    /// raw/canonical buffer accounting as [`Self::decode_cix_u16_gray_2d`].
    pub fn decode_cix_u16_planar(
        self,
        input: &[u8],
        expected_depth: usize,
        expected_pixels: usize,
        memory: usize,
    ) -> Result<(JxlProbedPlanarImage, Vec<u8>), JxlBridgeError> {
        let probed = self.probe_planar(input, expected_depth, expected_pixels)?;
        let raw_bytes = probed.geometry.byte_len()?;
        let canonical_size = expected_pixels
            .checked_mul(2)
            .ok_or(JxlBridgeError::ResourceLimit)?;
        if cix_decode_buffer_budget(
            input.len(),
            raw_bytes,
            canonical_size,
            probed.geometry.sample_type,
        )? > memory
        {
            return Err(JxlBridgeError::ResourceLimit);
        }
        let mut raw = vec![0_u8; raw_bytes];
        let (_, _, decode) = self.planar_fns()?;
        let status = unsafe {
            decode(
                input.as_ptr(),
                input.len(),
                probed.geometry.depth,
                probed.geometry.width,
                probed.geometry.height,
                probed.geometry.bits_per_sample,
                probed.geometry.sample_type as u32,
                raw.as_mut_ptr(),
                raw.len(),
            )
        };
        if JxlBridgeStatus::from_raw(status) != Some(JxlBridgeStatus::Ok) {
            return Err(JxlBridgeError::Status(JxlBridgeStatus::from_raw(status)));
        }
        let canonical = match probed.geometry.sample_type {
            JxlSampleType::Uint8 => raw.into_iter().flat_map(|sample| [sample, 0]).collect(),
            JxlSampleType::Uint16 => raw,
        };
        if canonical.len() != canonical_size {
            return Err(JxlBridgeError::InvalidOutputBuffer);
        }
        Ok((probed, canonical))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());
    static ENCODE_CALLS: AtomicUsize = AtomicUsize::new(0);
    static DECODE_CALLS: AtomicUsize = AtomicUsize::new(0);
    static FREE_CALLS: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn good_version() -> u32 {
        LIBJXL_0_12_0_VERSION
    }

    unsafe extern "C" fn wrong_version() -> u32 {
        LIBJXL_0_12_0_VERSION - 1
    }

    unsafe extern "C" fn mock_encode(
        image: *const JxlRawGrayImage,
        max_output_bytes: usize,
        output: *mut JxlRawBuffer,
    ) -> c_int {
        ENCODE_CALLS.fetch_add(1, Ordering::SeqCst);
        if image.is_null() || output.is_null() || max_output_bytes < 4 {
            return JxlBridgeStatus::InvalidArgument as c_int;
        }
        let encoded = Box::new([1_u8, 2, 3, 4]);
        (*output).data = Box::into_raw(encoded).cast::<u8>();
        (*output).size = 4;
        JxlBridgeStatus::Ok as c_int
    }

    unsafe extern "C" fn mock_decode(
        input: *const u8,
        input_size: usize,
        width: u32,
        height: u32,
        bits: u32,
        sample_type: u32,
        output: *mut u8,
        output_size: usize,
    ) -> c_int {
        DECODE_CALLS.fetch_add(1, Ordering::SeqCst);
        if input.is_null()
            || output.is_null()
            || input_size != 4
            || width != 2
            || height != 2
            || bits != 8
            || sample_type != JxlSampleType::Uint8 as u32
            || output_size != 4
        {
            return JxlBridgeStatus::InvalidArgument as c_int;
        }
        std::ptr::copy_nonoverlapping(input, output, output_size);
        JxlBridgeStatus::Ok as c_int
    }

    unsafe extern "C" fn mock_free(buffer: *mut JxlRawBuffer) {
        FREE_CALLS.fetch_add(1, Ordering::SeqCst);
        if !buffer.is_null() && !(*buffer).data.is_null() {
            drop(Box::from_raw((*buffer).data.cast::<[u8; 4]>()));
            (*buffer).data = std::ptr::null_mut();
            (*buffer).size = 0;
        }
    }

    fn bridge(version: JxlLibraryVersionFn) -> JxlBridge {
        unsafe { JxlBridge::from_symbols(mock_encode, mock_decode, mock_free, version) }
    }

    fn geometry() -> JxlGrayGeometry {
        JxlGrayGeometry {
            width: 2,
            height: 2,
            bits_per_sample: 8,
            sample_type: JxlSampleType::Uint8,
        }
    }

    fn reset_calls() {
        ENCODE_CALLS.store(0, Ordering::SeqCst);
        DECODE_CALLS.store(0, Ordering::SeqCst);
        FREE_CALLS.store(0, Ordering::SeqCst);
    }

    #[test]
    fn canonical_decode_budget_counts_input_and_simultaneous_uint8_expansion() {
        // Four samples: 4-byte raw UINT8 output remains live while its 8-byte
        // canonical u16 output is collected, alongside the retained input.
        assert_eq!(
            cix_decode_buffer_budget(5, 4, 8, JxlSampleType::Uint8).unwrap(),
            17
        );
        // UINT16 transport is returned as the canonical Vec without a copy.
        assert_eq!(
            cix_decode_buffer_budget(5, 8, 8, JxlSampleType::Uint16).unwrap(),
            13
        );
        assert!(cix_decode_buffer_budget(usize::MAX, 1, 2, JxlSampleType::Uint8).is_err());
    }

    #[test]
    fn wrong_version_rejects_before_codec_invocation() {
        let _serial = TEST_LOCK.lock().unwrap();
        reset_calls();
        let bridge = bridge(wrong_version);
        let image = JxlGrayImage {
            geometry: geometry(),
            effort: 7,
            bytes: &[1, 2, 3, 4],
        };
        assert_eq!(
            bridge.encode_gray_2d(image, 4),
            Err(JxlBridgeError::WrongLibraryVersion(11_999))
        );
        let mut output = [0_u8; 4];
        assert_eq!(
            bridge.decode_gray_2d(&[1, 2, 3, 4], geometry(), &mut output),
            Err(JxlBridgeError::WrongLibraryVersion(11_999))
        );
        assert_eq!(ENCODE_CALLS.load(Ordering::SeqCst), 0);
        assert_eq!(DECODE_CALLS.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn invalid_geometry_or_size_never_reaches_codec() {
        let _serial = TEST_LOCK.lock().unwrap();
        reset_calls();
        let bridge = bridge(good_version);
        let invalid = JxlGrayImage {
            geometry: JxlGrayGeometry {
                width: 2,
                height: 2,
                bits_per_sample: 8,
                sample_type: JxlSampleType::Uint16,
            },
            effort: 7,
            bytes: &[1, 2, 3, 4],
        };
        assert!(matches!(
            bridge.encode_gray_2d(invalid, 4),
            Err(JxlBridgeError::InvalidImage(_))
        ));
        let mut too_small = [0_u8; 3];
        assert_eq!(
            bridge.decode_gray_2d(&[1, 2, 3, 4], geometry(), &mut too_small),
            Err(JxlBridgeError::InvalidOutputBuffer)
        );
        assert_eq!(ENCODE_CALLS.load(Ordering::SeqCst), 0);
        assert_eq!(DECODE_CALLS.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn bounded_mock_round_trip_releases_bridge_buffer() {
        let _serial = TEST_LOCK.lock().unwrap();
        reset_calls();
        let bridge = bridge(good_version);
        let original = [1_u8, 2, 3, 4];
        let encoded = bridge
            .encode_gray_2d(
                JxlGrayImage {
                    geometry: geometry(),
                    effort: 7,
                    bytes: &original,
                },
                4,
            )
            .unwrap();
        assert_eq!(encoded, original);
        assert_eq!(FREE_CALLS.load(Ordering::SeqCst), 1);
        let mut restored = [0_u8; 4];
        bridge
            .decode_gray_2d(&encoded, geometry(), &mut restored)
            .unwrap();
        assert_eq!(restored, original);
        assert_eq!(ENCODE_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(DECODE_CALLS.load(Ordering::SeqCst), 1);
    }
}
