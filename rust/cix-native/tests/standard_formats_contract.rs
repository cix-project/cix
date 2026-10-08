use cix_native::standard_formats::{
    decode, encode, StandardError, StandardFormat, StandardOptions,
};
use flate2::{read::GzDecoder, read::ZlibDecoder};
use std::io::Read;

fn options() -> StandardOptions {
    StandardOptions {
        output_limit: 4 << 20,
        // XZ level 6's native encoder state is admitted from liblzma before
        // the provider allocates it.
        memory_limit: 1 << 30,
        ..StandardOptions::default()
    }
}

#[test]
fn each_format_emits_its_native_stream_and_roundtrips() {
    let input = b"standard format bytes\n".repeat(500);
    for format in [
        StandardFormat::Gzip,
        StandardFormat::Zlib,
        StandardFormat::Deflate,
        StandardFormat::Bzip2,
        StandardFormat::Xz,
        StandardFormat::Zstd,
        StandardFormat::Brotli,
        StandardFormat::Lz4Frame,
        StandardFormat::SnappyFramed,
    ] {
        let archive = encode(format, &input, &options()).unwrap();
        assert!(!archive.starts_with(b"CIX"), "{format:?}");
        assert_eq!(
            decode(format, &archive, &options()).unwrap(),
            input,
            "{format:?}"
        );
    }
}

#[test]
fn standard_magic_and_cross_decoder_contracts() {
    let input = b"cross decoder input".repeat(100);
    let gzip = encode(StandardFormat::Gzip, &input, &options()).unwrap();
    assert_eq!(&gzip[..2], b"\x1f\x8b");
    let mut gzip_out = Vec::new();
    GzDecoder::new(gzip.as_slice())
        .read_to_end(&mut gzip_out)
        .unwrap();
    assert_eq!(gzip_out, input);

    let zlib = encode(StandardFormat::Zlib, &input, &options()).unwrap();
    assert_eq!(zlib[0] & 0x0f, 8);
    let mut zlib_out = Vec::new();
    ZlibDecoder::new(zlib.as_slice())
        .read_to_end(&mut zlib_out)
        .unwrap();
    assert_eq!(zlib_out, input);

    let lz4 = encode(StandardFormat::Lz4Frame, &input, &options()).unwrap();
    assert_eq!(&lz4[..4], b"\x04\x22\x4d\x18");
    let snappy = encode(StandardFormat::SnappyFramed, &input, &options()).unwrap();
    assert!(snappy.starts_with(b"\xff\x06\0\0sNaPpY"));
    assert!(encode(StandardFormat::Bzip2, &input, &options())
        .unwrap()
        .starts_with(b"BZh"));
    assert!(encode(StandardFormat::Xz, &input, &options())
        .unwrap()
        .starts_with(b"\xfd7zXZ\0"));
    assert_eq!(
        &encode(StandardFormat::Zstd, &input, &options()).unwrap()[..4],
        b"\x28\xb5\x2f\xfd"
    );
}

#[test]
fn rejects_truncated_trailing_and_concatenated_streams() {
    for format in [
        StandardFormat::Gzip,
        StandardFormat::Zlib,
        StandardFormat::Deflate,
        StandardFormat::Lz4Frame,
        StandardFormat::SnappyFramed,
    ] {
        let archive = encode(format, b"format integrity", &options()).unwrap();
        assert!(
            decode(format, &archive[..archive.len() - 1], &options()).is_err(),
            "{format:?}"
        );
        let mut trailing = archive;
        trailing.push(0);
        assert!(decode(format, &trailing, &options()).is_err(), "{format:?}");
    }
    let first = encode(StandardFormat::Gzip, b"first", &options()).unwrap();
    let second = encode(StandardFormat::Gzip, b"second", &options()).unwrap();
    let mut concatenated = first;
    concatenated.extend_from_slice(&second);
    assert!(decode(StandardFormat::Gzip, &concatenated, &options()).is_err());
}

#[test]
fn limits_are_checked_before_output_allocation_and_empty_is_valid() {
    let invalid = StandardOptions {
        level: 10,
        ..options()
    };
    assert!(matches!(
        encode(StandardFormat::Zlib, b"x", &invalid),
        Err(StandardError::InvalidOptions(_))
    ));
    let too_small = StandardOptions {
        output_limit: 32,
        memory_limit: 32,
        ..options()
    };
    assert!(matches!(
        decode(StandardFormat::Zlib, b"not a stream", &too_small),
        Err(StandardError::InvalidOptions(_))
    ));
    for format in [StandardFormat::Lz4Frame, StandardFormat::SnappyFramed] {
        let archive = encode(format, b"", &options()).unwrap();
        assert_eq!(
            decode(format, &archive, &options()).unwrap(),
            b"",
            "{format:?}"
        );
    }
    let cap = StandardOptions {
        output_limit: 8,
        memory_limit: 1024,
        ..options()
    };
    assert!(encode(StandardFormat::SnappyFramed, b"x", &cap).is_err());
}
