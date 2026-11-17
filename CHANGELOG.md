# Changelog

## 0.2.0

- **Breaking: an idiomatic API.** Compression levels are a `Level` type (0 to 10, or
  `Level::DEFAULT`, miniz's default), so `compress(&data, Level::DEFAULT)` replaces
  `compress(&data, -1)`. miniz's compression flags are an `Options` struct of named fields,
  with a `Format` (zlib or raw DEFLATE) and, through `Options::from_level`, a `Strategy`;
  `compress_with` compresses with them. Decompression returns a `Result` with a
  `DecompressError` saying why it failed: `decompress` reads a zlib stream, replacing
  `uncompress`, and `decompress_with` reads either format and reports the input it
  occupied. Streaming compression is a `Compressor` with a `Flush` enum, whose `compress`
  returns a `Result` in place of a status code. The checksums are `adler32` and `crc32`,
  with `adler32_update` and `crc32_update` to continue one. The `tdef` and `tinfl` modules
  are no longer public.

- **Faster.** Compression is 4% to 37% faster and decompression 39% to 74% faster at levels
  1 to 9, depending on the data and level (see [Performance](docs/performance.md)). Output
  is unchanged, checked against miniz on 30,000 files (800 MB).

- **Rewritten as idiomatic Rust.** The code no longer mirrors miniz's C line by line.

- **Documentation.** Every public item is documented, the README is a short overview, and
  the detail is in [Guide](docs/guide.md), [Verification](docs/verification.md) and
  [Performance](docs/performance.md).

## 0.1.0

- First release: miniz's compressor and decompressor, moved out of
  [rustbcx](https://github.com/OctoKeights/rustbcx), where it was `rustbcx::miniz`.
