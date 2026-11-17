# Performance

[Overview](../README.md) · [Guide](guide.md) · [Verification](verification.md) · **Performance**

---

`examples/bench.rs` compresses and decompresses 4 MiB of synthetic text-like data (words
from a small vocabulary) and binary-like data (runs, repeats and noise) at several levels,
and reports the best of seven runs:

```sh
cargo run --release --example bench
```

## Throughput

Measured on one core of an Intel Xeon at 2.1 GHz, built for generic x86-64, in megabytes of
uncompressed data per second:

| Data | Level | Output size | Compress | Decompress |
|---|---|---|---|---|
| Text-like | 0 | 100.0% | 97 MB/s | 2,040 MB/s |
| Text-like | 1 | 25.4% | 98 MB/s | 281 MB/s |
| Text-like | 3 | 20.7% | 53 MB/s | 334 MB/s |
| Text-like | 6 | 18.7% | 12 MB/s | 406 MB/s |
| Text-like | 9 | 17.9% | 4 MB/s | 437 MB/s |
| Binary-like | 0 | 100.0% | 97 MB/s | 1,790 MB/s |
| Binary-like | 1 | 15.6% | 137 MB/s | 457 MB/s |
| Binary-like | 3 | 14.7% | 102 MB/s | 530 MB/s |
| Binary-like | 6 | 13.6% | 55 MB/s | 552 MB/s |
| Binary-like | 9 | 13.7% | 35 MB/s | 561 MB/s |

Level 0 compresses slowly for a level that stores, because miniz still passes every byte
through its window and hash chains before deciding to store the block; that is kept, since it
decides where the blocks end. The higher levels are slow on the text-like data because its
small vocabulary fills the hash chains with candidates.

## Compared with 0.1

Version 0.2 rewrote the crate as idiomatic Rust. Measured alternately with 0.1 on the same
machine:

| Data | Level | Compress | Decompress |
|---|---|---|---|
| Text-like | 1 | 37% faster | 60% faster |
| Text-like | 3 | 22% faster | 39% faster |
| Text-like | 6 | 11% faster | 50% faster |
| Text-like | 9 | 5% faster | 51% faster |
| Binary-like | 1 | 4% faster | 58% faster |
| Binary-like | 3 | 14% faster | 74% faster |
| Binary-like | 6 | 21% faster | 68% faster |
| Binary-like | 9 | 30% faster | 61% faster |

Level 0 is unchanged within measurement noise in both directions: decompression of stored
blocks is limited by copying memory.

Most of the gain in decompression comes from copying matches as whole slices where they do
not overlap their output, and runs of one byte as a fill; most of the gain in compression
from comparing candidate matches eight bytes at a time and writing output in place rather
than through a separate block buffer.
