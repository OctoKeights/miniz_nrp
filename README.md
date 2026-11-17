# miniz_nrp

**zlib and DEFLATE compression in pure, safe Rust, bit-identical to miniz.**

**Overview** · [Guide](docs/guide.md) · [Verification](docs/verification.md) · [Performance](docs/performance.md)

---

miniz_nrp is a native Rust port of the compressor and decompressor of Rich Geldreich's
[miniz](https://github.com/richgel999/miniz) v3.1.0. Its compressor produces the same streams
as miniz, byte for byte, at every level and with every option, and its decompressor accepts
and rejects exactly the streams miniz does.

## Highlights

- **Bit-identical output.** Every stream matches miniz's, which matters for formats and tools
  that store or compare miniz's output, such as tinyexr's and stb's.

- **Exact decompression.** Valid streams decompress to the same bytes, and damaged,
  truncated or oversized ones fail for the same reason.

- **Streaming compression.** Input can arrive in pieces, with sync and full flushes between.

- **Zero dependencies.** Standard library only: no C compiler, no build script.

- **No `unsafe` code.** Enforced by `#![forbid(unsafe_code)]`.

## Installation

```toml
[dependencies]
miniz_nrp = "0.2"
```

## Quick start

```rust
use miniz_nrp::{DecompressError, Level, compress, crc32, decompress};

let data = b"to be or not to be, that is the question; ".repeat(100);

let stream = compress(&data, Level::DEFAULT);
assert_eq!(decompress(&stream, data.len()).unwrap(), data);

// A limit that is too small is reported, not exceeded.
assert_eq!(
    decompress(&stream, data.len() - 1),
    Err(DecompressError::OutputTooLarge)
);

assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
```

## Scope

Raw DEFLATE and zlib streams, both ways, with the Adler-32 and CRC-32 checksums. miniz's
copy of zlib's streaming API (`mz_deflate`, `mz_inflate`), its `.zip` archive functions and
its PNG writer are not ported.

For formats that store libdeflate's exact output, such as OpenEXR's DWA compression,
[libdeflate_nrp](https://github.com/OctoKeights/libdeflate_nrp) is the counterpart.

## Licence

MIT, as miniz is; see [`LICENSE`](LICENSE), which keeps miniz's copyright notices.

Bug reports and contributions are welcome, above all any stream that miniz_nrp compresses or
decompresses differently from miniz.
