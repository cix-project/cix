//! Owned Zstd dictionary handles; payloads remain ordinary Zstd frames.
use sha2::{Digest, Sha256};
use std::ffi::{c_int, c_uint, c_void};
#[link(name = "zstd")]
unsafe extern "C" {
    fn ZDICT_trainFromBuffer(
        d: *mut c_void,
        cap: usize,
        s: *const c_void,
        sizes: *const usize,
        n: c_uint,
    ) -> usize;
    fn ZDICT_isError(n: usize) -> u32;
    fn ZDICT_getDictID(d: *const c_void, n: usize) -> u32;
    fn ZSTD_createCDict(d: *const c_void, n: usize, l: c_int) -> *mut CD;
    fn ZSTD_freeCDict(x: *mut CD) -> usize;
    fn ZSTD_createDDict(d: *const c_void, n: usize) -> *mut DD;
    fn ZSTD_freeDDict(x: *mut DD) -> usize;
    fn ZSTD_createCCtx() -> *mut CC;
    fn ZSTD_freeCCtx(x: *mut CC) -> usize;
    fn ZSTD_createDCtx() -> *mut DC;
    fn ZSTD_freeDCtx(x: *mut DC) -> usize;
    fn ZSTD_compress_usingCDict(
        c: *mut CC,
        o: *mut c_void,
        oc: usize,
        i: *const c_void,
        ic: usize,
        d: *const CD,
    ) -> usize;
    fn ZSTD_decompress_usingDDict(
        c: *mut DC,
        o: *mut c_void,
        oc: usize,
        i: *const c_void,
        ic: usize,
        d: *const DD,
    ) -> usize;
    fn ZSTD_compressBound(n: usize) -> usize;
    fn ZSTD_isError(n: usize) -> u32;
    fn ZSTD_getDictID_fromFrame(p: *const c_void, n: usize) -> u32;
    fn ZSTD_findFrameCompressedSize(p: *const c_void, n: usize) -> usize;
    fn ZSTD_estimateCCtxSize(level: c_int) -> usize;
    fn ZSTD_estimateCDictSize(dict: usize, level: c_int) -> usize;
    fn ZSTD_estimateDCtxSize() -> usize;
    fn ZSTD_estimateDDictSize(dict: usize, load_method: c_int) -> usize;
}
#[repr(C)]
struct CD {
    _opaque: [u8; 0],
}
#[repr(C)]
struct DD {
    _opaque: [u8; 0],
}
#[repr(C)]
struct CC {
    _opaque: [u8; 0],
}
#[repr(C)]
struct DC {
    _opaque: [u8; 0],
}
struct CGuard(*mut CC);
impl Drop for CGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                ZSTD_freeCCtx(self.0);
            }
        }
    }
}
struct DGuard(*mut DC);
impl Drop for DGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                ZSTD_freeDCtx(self.0);
            }
        }
    }
}
struct CDGuard(*mut CD);
impl Drop for CDGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                ZSTD_freeCDict(self.0);
            }
        }
    }
}
struct DDGuard(*mut DD);
impl Drop for DDGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                ZSTD_freeDDict(self.0);
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DictionaryIdentity {
    pub dict_id: u32,
    pub sha256: [u8; 32],
}
pub struct ZstdDictionary {
    bytes: Vec<u8>,
    identity: DictionaryIdentity,
}
impl ZstdDictionary {
    pub fn train(samples: &[&[u8]], cap: usize, memory: usize) -> Result<Self, String> {
        let total = samples
            .iter()
            .try_fold(0usize, |a, x| a.checked_add(x.len()))
            .ok_or("sample overflow")?;
        if cap < 256
            || samples.len() > c_uint::MAX as usize
            || total < 128
            || total
                .checked_mul(7)
                .and_then(|n| n.checked_add(cap))
                .and_then(|n| {
                    n.checked_add(samples.len().checked_mul(std::mem::size_of::<usize>())?)
                })
                .is_none_or(|n| n > memory)
        {
            return Err("dictionary training resource limit".into());
        };
        let mut flat = Vec::new();
        flat.try_reserve_exact(total)
            .map_err(|_| "sample allocation failed")?;
        let mut sizes = Vec::new();
        sizes
            .try_reserve_exact(samples.len())
            .map_err(|_| "sample-size allocation failed")?;
        for s in samples {
            flat.extend_from_slice(s);
            sizes.push(s.len())
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(cap)
            .map_err(|_| "dictionary allocation failed")?;
        bytes.resize(cap, 0);
        let n = unsafe {
            ZDICT_trainFromBuffer(
                bytes.as_mut_ptr().cast(),
                cap,
                flat.as_ptr().cast(),
                sizes.as_ptr(),
                c_uint::try_from(sizes.len()).map_err(|_| "too many training samples")?,
            )
        };
        if unsafe { ZDICT_isError(n) } != 0 {
            return Err("Zstd dictionary training failed".into());
        };
        bytes.truncate(n);
        Self::from_bytes(bytes)
    }
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, String> {
        let id = unsafe { ZDICT_getDictID(bytes.as_ptr().cast(), bytes.len()) };
        if bytes.is_empty() {
            return Err("empty dictionary".into());
        }
        let identity = DictionaryIdentity {
            dict_id: id,
            sha256: Sha256::digest(&bytes).into(),
        };
        Ok(Self { bytes, identity })
    }
    pub fn identity(&self) -> &DictionaryIdentity {
        &self.identity
    }

    /// Exact serialized dictionary bytes for persistence or transfer.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Bytes retained by the owned dictionary allocation, including spare
    /// capacity left by dictionary training.
    pub fn retained_bytes(&self) -> usize {
        self.bytes.capacity()
    }

    pub fn encode(
        &self,
        input: &[u8],
        level: i32,
        limit: usize,
        memory: usize,
    ) -> Result<Vec<u8>, String> {
        if !(-131072..=22).contains(&level) {
            return Err("invalid Zstd level".into());
        }
        let cap = unsafe { ZSTD_compressBound(input.len()) }.min(limit);
        let cctx = unsafe { ZSTD_estimateCCtxSize(level) };
        let cdict = unsafe { ZSTD_estimateCDictSize(self.bytes.len(), level) };
        if unsafe { ZSTD_isError(cctx) } != 0 || unsafe { ZSTD_isError(cdict) } != 0 {
            return Err("Zstd encode memory estimate unavailable".into());
        }
        if self
            .retained_bytes()
            .checked_add(input.len())
            .and_then(|n| n.checked_add(cap))
            .and_then(|n| n.checked_add(cctx))
            .and_then(|n| n.checked_add(cdict))
            .is_none_or(|n| n > memory)
        {
            return Err("dictionary encode resource limit".into());
        }
        let d = CDGuard(unsafe {
            ZSTD_createCDict(self.bytes.as_ptr().cast(), self.bytes.len(), level)
        });
        let c = CGuard(unsafe { ZSTD_createCCtx() });
        if d.0.is_null() || c.0.is_null() {
            return Err("Zstd dictionary allocation failed".into());
        }
        let mut out = Vec::new();
        out.try_reserve_exact(cap)
            .map_err(|_| "dictionary output allocation failed")?;
        out.resize(cap, 0);
        let n = unsafe {
            ZSTD_compress_usingCDict(
                c.0,
                out.as_mut_ptr().cast(),
                cap,
                input.as_ptr().cast(),
                input.len(),
                d.0,
            )
        };

        if unsafe { ZSTD_isError(n) } != 0 {
            return Err("Zstd dictionary encode failed or exceeded cap".into());
        };
        out.truncate(n);
        Ok(out)
    }
}
impl ZstdDictionary {
    pub fn decode(
        &self,
        payload: &[u8],
        limit: usize,
        memory: usize,
        declared: &DictionaryIdentity,
    ) -> Result<Vec<u8>, String> {
        let frame_size =
            unsafe { ZSTD_findFrameCompressedSize(payload.as_ptr().cast(), payload.len()) };
        if unsafe { ZSTD_isError(frame_size) } != 0 || frame_size != payload.len() {
            return Err("Zstd dictionary payload is trailing or concatenated".into());
        }
        if declared != &self.identity {
            return Err("dictionary SHA-256 identity mismatch".into());
        };
        let id = unsafe { ZSTD_getDictID_fromFrame(payload.as_ptr().cast(), payload.len()) };
        if id != 0 && id != self.identity.dict_id {
            return Err("Zstd frame dictionary ID mismatch".into());
        };
        let dctx = unsafe { ZSTD_estimateDCtxSize() };
        let ddict = unsafe { ZSTD_estimateDDictSize(self.bytes.len(), 0) };
        if unsafe { ZSTD_isError(dctx) } != 0 || unsafe { ZSTD_isError(ddict) } != 0 {
            return Err("Zstd decode memory estimate unavailable".into());
        }
        if self
            .retained_bytes()
            .checked_add(ddict)
            .and_then(|n| n.checked_add(payload.len()))
            .and_then(|n| n.checked_add(limit))
            .and_then(|n| n.checked_add(dctx))
            .is_none_or(|n| n > memory)
        {
            return Err("dictionary decode resource limit".into());
        }
        let d = DDGuard(unsafe { ZSTD_createDDict(self.bytes.as_ptr().cast(), self.bytes.len()) });
        let c = DGuard(unsafe { ZSTD_createDCtx() });
        if d.0.is_null() || c.0.is_null() {
            return Err("Zstd dictionary allocation failed".into());
        };
        let mut out = Vec::new();
        out.try_reserve_exact(limit)
            .map_err(|_| "dictionary output allocation failed")?;
        out.resize(limit, 0);
        let n = unsafe {
            ZSTD_decompress_usingDDict(
                c.0,
                out.as_mut_ptr().cast(),
                limit,
                payload.as_ptr().cast(),
                payload.len(),
                d.0,
            )
        };

        if unsafe { ZSTD_isError(n) } != 0 {
            return Err("Zstd dictionary decode failed or output exceeds cap".into());
        };
        out.truncate(n);
        Ok(out)
    }
}
