# Guide

[Overview](../README.md) · **Guide** · [Verification](verification.md) · [Performance](performance.md)

---

- [Compression levels](#compression-levels)
- [Options](#options)
- [Compressing in pieces](#compressing-in-pieces)
- [Decompressing](#decompressing)
- [Checksums](#checksums)
- [Correspondence with miniz](#correspondence-with-miniz)
- [Differences from miniz](#differences-from-miniz)

## Compression levels

| Level | Constant | Candidates searched | Parsing |
|---|---|---|---|
| 0 | `Level::NONE` | none: data stored as is | |
| 1 | `Level::FASTEST` | 1 | Greedy |
| 2, 3 | | 6, 32 | Greedy |
| 4 to 8 | | 16 to 512 | Lazy |
| 9 | `Level::BEST` | 768 | Lazy |
| 10 | `Level::UBER` | 1,500 | Lazy |
| default | `Level::DEFAULT` | 128, as level 6 | Greedy |

Greedy parsing takes each match as it is found; lazy parsing first checks whether the next
position has a longer one. `Level::DEFAULT` is miniz's `MZ_DEFAULT_COMPRESSION`, the level
`mz_compress` uses: it searches as level 6 does but parses greedily, so its streams differ
from level 6's.

`Level::new` accepts 0 to 10 and returns `None` otherwise; `Level::try_from` does the same with
an error.

## Options

`compress` writes a zlib stream at a level. For raw DEFLATE, a strategy, or finer control,
`compress_with` takes `Options`: miniz's compression flags as named fields.
`Options::from_level` gives the options miniz derives from a level, format and strategy
(`tdefl_create_comp_flags_from_zip_params`), and any field can then be changed:

```rust
use miniz_nrp::{Format, Level, Options, Strategy, compress_with, decompress_with};

let data = vec![42u8; 10_000];
let options = Options::from_level(Level::BEST, Format::Raw, Strategy::Rle);
let stream = compress_with(&data, &options);

let result = decompress_with(&stream, Format::Raw, data.len()).unwrap();
assert_eq!(result.data, data);
```

| Strategy | Effect |
|---|---|
| `Strategy::Default` | General-purpose compression |
| `Strategy::Filtered` | Matches of five bytes or fewer are discarded |
| `Strategy::HuffmanOnly` | No matches: Huffman coding of the bytes only |
| `Strategy::Rle` | Only runs of the preceding byte |
| `Strategy::Fixed` | Only DEFLATE's fixed Huffman codes |

## Compressing in pieces

A `Compressor` accepts input in pieces. Each call to `compress` takes all the input it is
given and a `Flush`:

- `Flush::None` waits for more input.

- `Flush::Sync` ends the current block and pads to a byte boundary, so that everything given
  so far can be decompressed from the output so far.

- `Flush::Full` does the same and forgets the input so far, so that decompression could start
  again from that point.

- `Flush::Finish` ends the stream. After it, `compress` returns `Err(AlreadyFinished)`.

The stream does not depend on how the input is divided, only on where it is flushed:

```rust
use miniz_nrp::{Compressor, Flush, Options, compress_with};

let data = b"a stream compressed in pieces, a stream compressed in pieces".repeat(50);
let options = Options::default();

let mut compressor = Compressor::new(&options);
for piece in data.chunks(100) {
    compressor.compress(piece, Flush::None).unwrap();
}
compressor.compress(&[], Flush::Finish).unwrap();

assert_eq!(compressor.take_output(), compress_with(&data, &options));
```

`take_output` hands over the bytes compressed so far at any point, and `adler32` gives the
checksum of the input so far.

## Decompressing

`decompress` reads a zlib stream; `decompress_with` reads either format and also reports how
many bytes of input the stream occupied, so that streams placed one after another can be read
in turn. Both take a limit on the decompressed size, and fail with a `DecompressError`:

- `Truncated` when the input ends before the stream does.

- `Invalid` for anything that is not a valid stream, including a bad zlib header.

- `ChecksumMismatch` when a zlib stream is complete but its Adler-32 is wrong.

- `OutputTooLarge` when the stream holds more than the limit.

Output is allocated as it is produced, so a large limit costs nothing for a small stream.
Input after the end of the stream is ignored.

## Checksums

`adler32` and `crc32` compute the checksums of a slice; `adler32_update` and `crc32_update`
continue a checksum over more data:

```rust
use miniz_nrp::{adler32, adler32_update, crc32, crc32_update};

let (head, tail) = b"Wikipedia".split_at(4);
assert_eq!(adler32_update(adler32(head), tail), 0x11E6_0398);
assert_eq!(crc32_update(crc32(head), tail), crc32(b"Wikipedia"));
```

## Correspondence with miniz

| miniz | miniz_nrp |
|---|---|
| `mz_compress2(dest, len, source, source_len, level)` | `compress(source, level)` |
| `mz_uncompress` | `decompress(stream, max_len)` |
| `tdefl_compress_mem_to_heap(source, len, out_len, flags)` | `compress_with(source, &options)` |
| `tdefl_create_comp_flags_from_zip_params(level, window_bits, strategy)` | `Options::from_level(level, format, strategy)` |
| `tdefl_init` with `tdefl_compress_buffer` | `Compressor::new` with `Compressor::compress` |
| `TDEFL_NO_FLUSH`, `TDEFL_SYNC_FLUSH`, `TDEFL_FULL_FLUSH`, `TDEFL_FINISH` | `Flush::None`, `Flush::Sync`, `Flush::Full`, `Flush::Finish` |
| `tdefl_get_adler32` | `Compressor::adler32` |
| `tinfl_decompress` with all input and output at once | `decompress_with(stream, format, max_len)` |
| `TINFL_FLAG_PARSE_ZLIB_HEADER` | `Format::Zlib` |
| `mz_adler32`, `mz_crc32` | `adler32_update`, `crc32_update` |

| `tdefl` flag | `Options` field |
|---|---|
| The low 12 bits: probes | `max_probes` |
| `TDEFL_WRITE_ZLIB_HEADER` | `format: Format::Zlib` |
| `TDEFL_COMPUTE_ADLER32` | `compute_adler32` |
| `TDEFL_GREEDY_PARSING_FLAG` | `greedy_parsing` |
| `TDEFL_RLE_MATCHES` | `rle_matches` |
| `TDEFL_FILTER_MATCHES` | `filter_matches` |
| `TDEFL_FORCE_ALL_STATIC_BLOCKS` | `force_static_blocks` |
| `TDEFL_FORCE_ALL_RAW_BLOCKS` | `force_raw_blocks` |

## Differences from miniz

- **Output is collected, not written to a callback or a caller's buffer.** The compressor
  keeps its output until `take_output` takes it. Where miniz writes blocks into a buffer of
  the caller's, the blocks it writes are the same.

- **Decompression takes all its input at once.** miniz's decompressor can pause when its
  input or output runs out and resume when given more; here, running out of input is
  `Truncated` and of room `OutputTooLarge`, exactly as miniz reports them when given all its
  input and output at once (`tinfl_decompress_mem_to_mem`).

- **No status codes.** Errors are `Result`s: `DecompressError` in place of `tinfl_status`,
  and `AlreadyFinished` in place of `TDEFL_STATUS_BAD_PARAM`.

- **`TDEFL_NONDETERMINISTIC_PARSING_FLAG` is not offered**, since it lets output depend on
  the contents of uninitialised memory.
