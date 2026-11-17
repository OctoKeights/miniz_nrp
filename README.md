# miniz_nrp

**zlib and deflate in pure, safe Rust: a native Rust port of miniz.**

miniz_nrp is a line-for-line port of the compressor and decompressor of Rich Geldreich's
[miniz](https://github.com/richgel999/miniz). It gives the bytes miniz gives at every level and
with every flag, and takes and refuses the streams miniz takes and refuses.

## Highlights

- **Bit-identical output.** Every stream matches miniz's byte for byte.

- **The same refusals.** Damaged, truncated and oversized streams are refused where miniz
  refuses them.

- **Zero dependencies.** Standard library only: no C compiler, no build script.

- **No `unsafe` code.** Enforced by `#![forbid(unsafe_code)]`.

## Installation

```toml
[dependencies]
miniz_nrp = "0.1"
```

## Quick start

```rust
use miniz_nrp::{compress, mz_crc32, uncompress, MZ_CRC32_INIT};

let text = b"the quick brown fox jumps over the lazy dog, again and again and again";

// A zlib stream at level 6 (0 stores, 1 is fastest, 9 is best, 10 more at any cost).
let packed = compress(text, 6);

// Back again, with no more than the room given.
assert_eq!(uncompress(&packed, text.len()).as_deref(), Some(&text[..]));
assert!(uncompress(&packed, text.len() - 1).is_none());

assert_eq!(mz_crc32(MZ_CRC32_INIT, b"123456789"), 0xCBF4_3926);
```

For raw deflate, flushing in pieces and every flag, use the compressor (`tdef`) and the
decompressor (`tinfl`) directly.

## What is ported

| Module | Original | What it does |
|---|---|---|
| `tdef` | `miniz_tdef.c` | The compressor: zlib or raw deflate, every level and strategy, in one go or in pieces |
| `tinfl` | `miniz_tinfl.c` | The decompressor |
| the crate | `miniz.c` | Adler-32 and CRC-32, and `compress`/`uncompress`, the short way to and from zlib |

Left out are miniz's copy of zlib's stream functions (`mz_deflate`, `mz_inflate` and theirs),
`.zip` archives, and its PNG writer.

## Differences from the original

The code is translated from C to Rust: the compressor writes through its own buffer only, and
the decompressor is the original's one function as it runs when given all its input and all its
room at once. What it computes is kept: the same compressed bytes as miniz as it builds by
default, and the same streams taken and refused.

## Verification

`compare/compare.sh` builds `compare/dump_miniz.c` against a checkout of miniz and
`examples/dump.rs` against the port, runs both over the same made-up data and compares what
they print byte for byte:

```sh
compare/compare.sh <checkout of miniz> [count] [seeds]
```

Made-up data of eight kinds and many sizes is compressed at every level, with every strategy
and with randomised flags, in one call and incrementally with flushes between. Each stream is
decompressed whole, into too little room, damaged, truncated, and with bytes after it. In the
last full run, 30,000 files totalling 800 megabytes, each compressed two or three ways and
decompressed seven, came out identical.

`cargo test` needs no C compiler: `tests/golden.rs` checks the port against hashes recorded
from miniz (`tests/data`).

## Licence

MIT, as miniz is: see [LICENSE](LICENSE), which keeps miniz's copyright notices.
