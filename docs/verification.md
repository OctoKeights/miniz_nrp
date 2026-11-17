# Verification

[Overview](../README.md) · [Guide](guide.md) · **Verification** · [Performance](performance.md)

---

miniz_nrp is checked against miniz itself: both are run over the same synthetic data and
their output compared byte for byte.

## The comparison

`compare/compare.sh` builds `compare/dump_miniz.c` against a checkout of miniz and
`examples/dump.rs` against miniz_nrp, runs both, and compares what they print:

```sh
compare/compare.sh <checkout of miniz> [count] [seeds]
```

Both programs generate the same data from each seed and print a running hash of everything
they produce:

- **Compression** of synthetic data of eight kinds and many sizes, with the options of every
  level, strategy and format and with random options, in one call and in pieces with sync
  and full flushes between; and at a random level with `mz_compress2`.

- **Decompression** of each stream intact, into too little room, damaged, truncated, and
  with bytes after it; and of random bytes, some after a valid zlib header.

- **Checksums**: the Adler-32 and CRC-32 of every input.

## Results

| Files | Data | Result |
|---|---|---|
| 30,000 | 800 MB | Every stream identical; every decompression gives the same bytes or the same error |

## Tests

`cargo test` needs no C compiler. `tests/golden.rs` checks miniz_nrp against hashes recorded
from miniz (`tests/data/miniz_expected.txt`, 240 files), unit tests check the checksums and
round trips at every level, strategy and format, and the examples in the README and the
Guide are run as tests. `tests/data/README.md` explains how to record the hashes again.
