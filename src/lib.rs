//! zlib and raw DEFLATE compression in pure, safe Rust: a native port of the compressor and
//! decompressor of Rich Geldreich's [miniz](https://github.com/richgel999/miniz) v3.1.0.
//!
//! The compressor produces the same streams as miniz, byte for byte, at every level and with
//! every option. The decompressor accepts and rejects exactly the streams miniz does.
//!
//! - [`compress`] and [`decompress`] are the short way to and from a zlib stream.
//!
//! - [`compress_with`] compresses with full [`Options`], built from a [`Level`], a [`Format`]
//!   and a [`Strategy`] or set field by field; [`decompress_with`] decompresses either format
//!   and reports how much input the stream occupied.
//!
//! - [`Compressor`] compresses input that arrives in pieces, with [`Flush`] points between.
//!
//! - [`adler32`] and [`crc32`] compute the two checksums, and their `_update` forms continue
//!   one.
//!
//! ```
//! use miniz_nrp::{Level, compress, decompress};
//!
//! let data = b"to be or not to be, that is the question; ".repeat(100);
//! let stream = compress(&data, Level::DEFAULT);
//! assert_eq!(decompress(&stream, data.len()).unwrap(), data);
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod deflate;
mod inflate;
mod tables;

pub use deflate::{AlreadyFinished, Compressor, Flush};

use core::fmt;

/// A compression level, from 0 (none) through 1 (fastest) and 9 (best) to 10 (best at any
/// cost in time), or miniz's default.
///
/// Higher levels search further for repeated data. Levels 1 to 3 take the first match found
/// (greedy parsing); levels 4 and above look one byte ahead for a better one (lazy parsing).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Level(i8);

impl Level {
    /// Level 0: data stored without compression.
    pub const NONE: Level = Level(0);

    /// Level 1: the fastest compression.
    pub const FASTEST: Level = Level(1);

    /// miniz's default (`MZ_DEFAULT_COMPRESSION`), which `mz_compress` uses. It searches as
    /// thoroughly as level 6 but parses greedily, as levels 1 to 3 do, so it is not quite
    /// level 6: its streams are a little larger and quicker to make.
    pub const DEFAULT: Level = Level(-1);

    /// Level 9: the best compression at a reasonable speed.
    pub const BEST: Level = Level(9);

    /// Level 10: the best compression miniz offers, which can be much slower than level 9.
    pub const UBER: Level = Level(10);

    /// The level `level`, if it is 0 to 10.
    #[must_use]
    pub const fn new(level: u8) -> Option<Level> {
        if level <= 10 {
            Some(Level(level as i8))
        } else {
            None
        }
    }

    /// The level as a number, or `None` for [`Level::DEFAULT`].
    #[must_use]
    pub const fn get(self) -> Option<u8> {
        if self.0 < 0 { None } else { Some(self.0 as u8) }
    }
}

impl Default for Level {
    fn default() -> Self {
        Level::DEFAULT
    }
}

impl TryFrom<u8> for Level {
    type Error = UnsupportedLevel;

    fn try_from(level: u8) -> Result<Self, Self::Error> {
        Level::new(level).ok_or(UnsupportedLevel(level))
    }
}

/// The error for a compression level above 10.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnsupportedLevel(pub u8);

impl fmt::Display for UnsupportedLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "compression level {} is not supported (use 0 to 10)",
            self.0
        )
    }
}

impl std::error::Error for UnsupportedLevel {}

/// The container around the compressed data.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Format {
    /// A zlib stream: a two-byte header, the DEFLATE data, and the Adler-32 checksum of the
    /// uncompressed data.
    #[default]
    Zlib,
    /// Raw DEFLATE data, with no header or checksum.
    Raw,
}

/// A tuning of the compressor for particular kinds of data, as zlib's strategies are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Strategy {
    /// General-purpose compression (`MZ_DEFAULT_STRATEGY`).
    #[default]
    Default,
    /// Matches of five bytes or fewer are not used, which suits data that is mostly small
    /// variations on its neighbours (`MZ_FILTERED`).
    Filtered,
    /// No matches at all: Huffman coding of the bytes only (`MZ_HUFFMAN_ONLY`).
    HuffmanOnly,
    /// Only matches of the preceding byte, that is, runs (`MZ_RLE`).
    Rle,
    /// Only DEFLATE's fixed Huffman codes, never codes built for the data (`MZ_FIXED`).
    Fixed,
}

/// Everything that decides how data is compressed: miniz's compression flags as named
/// fields.
///
/// [`Options::from_level`] gives the options miniz derives from a level, format and strategy;
/// the fields can then be adjusted, or set directly.
///
/// ```
/// use miniz_nrp::{Format, Level, Options, Strategy, compress_with};
///
/// let mut options = Options::from_level(Level::BEST, Format::Raw, Strategy::Default);
/// options.max_probes = 2000;
/// let stream = compress_with(b"some data, some data", &options);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
// (Independent switches, each one of miniz's flags; `Options::from_level` sets them.)
#[allow(clippy::struct_excessive_bools)]
pub struct Options {
    /// zlib or raw DEFLATE (`TDEFL_WRITE_ZLIB_HEADER`).
    pub format: Format,
    /// The most candidates examined in each search for a repeat, from 0 (no searching:
    /// Huffman coding only) to 4,095; larger values are taken as 4,095. Searches that have
    /// already found a match of 32 bytes or more examine about a quarter as many.
    pub max_probes: u16,
    /// Take each match as it is found, without looking ahead for a better one
    /// (`TDEFL_GREEDY_PARSING_FLAG`).
    pub greedy_parsing: bool,
    /// Compute the Adler-32 checksum of the input even for raw DEFLATE, for
    /// [`Compressor::adler32`] (`TDEFL_COMPUTE_ADLER32`). zlib streams always compute it.
    pub compute_adler32: bool,
    /// Search only for runs of the preceding byte (`TDEFL_RLE_MATCHES`).
    pub rle_matches: bool,
    /// Discard matches of five bytes or fewer (`TDEFL_FILTER_MATCHES`).
    pub filter_matches: bool,
    /// Use only DEFLATE's fixed Huffman codes (`TDEFL_FORCE_ALL_STATIC_BLOCKS`).
    pub force_static_blocks: bool,
    /// Store the data without compression (`TDEFL_FORCE_ALL_RAW_BLOCKS`).
    pub force_raw_blocks: bool,
}

/// The candidates searched at each level from 0 to 10, as miniz has them.
const PROBES_BY_LEVEL: [u16; 11] = [0, 1, 6, 32, 16, 32, 128, 256, 512, 768, 1500];

impl Options {
    /// The options miniz derives from a level, format and strategy
    /// (`tdefl_create_comp_flags_from_zip_params`).
    ///
    /// Level 0 stores the data whatever the strategy.
    pub fn from_level(level: Level, format: Format, strategy: Strategy) -> Options {
        let mut options = Options {
            format,
            max_probes: PROBES_BY_LEVEL[level.get().map_or(6, usize::from)],
            greedy_parsing: level.0 <= 3,
            compute_adler32: false,
            rle_matches: false,
            filter_matches: false,
            force_static_blocks: false,
            force_raw_blocks: false,
        };
        if level == Level::NONE {
            options.force_raw_blocks = true;
        } else {
            match strategy {
                Strategy::Default => {}
                Strategy::Filtered => options.filter_matches = true,
                Strategy::HuffmanOnly => options.max_probes = 0,
                Strategy::Rle => options.rle_matches = true,
                Strategy::Fixed => options.force_static_blocks = true,
            }
        }
        options
    }

    /// The level whose search depth these options have, as the zlib header records it.
    fn header_level(&self) -> u32 {
        match PROBES_BY_LEVEL
            .iter()
            .position(|&p| p == self.max_probes.min(0xFFF))
        {
            Some(0 | 1) => 0,
            Some(2..=5) => 1,
            Some(6) => 2,
            _ => 3,
        }
    }
}

impl Default for Options {
    /// The options for [`Level::DEFAULT`], [`Format::Zlib`] and [`Strategy::Default`].
    fn default() -> Self {
        Options::from_level(Level::DEFAULT, Format::Zlib, Strategy::Default)
    }
}

/// Compresses `data` to a zlib stream at `level` (`mz_compress2`).
#[must_use]
pub fn compress(data: &[u8], level: Level) -> Vec<u8> {
    compress_with(
        data,
        &Options::from_level(level, Format::Zlib, Strategy::Default),
    )
}

/// Compresses `data` with `options` (`tdefl_compress_mem_to_heap`).
///
/// Data that does not compress can come out slightly larger than it went in.
#[must_use]
pub fn compress_with(data: &[u8], options: &Options) -> Vec<u8> {
    let mut compressor = Compressor::new(options);
    compressor.compress_unfinished(data, Flush::Finish);
    compressor.take_output()
}

/// Decompresses a zlib stream that holds no more than `max_len` bytes (`mz_uncompress`).
///
/// Any input after the end of the stream is ignored.
///
/// # Errors
///
/// A [`DecompressError`] if the stream is cut short, is not a valid zlib stream, fails its
/// checksum, or holds more than `max_len` bytes.
pub fn decompress(stream: &[u8], max_len: usize) -> Result<Vec<u8>, DecompressError> {
    decompress_with(stream, Format::Zlib, max_len).map(|result| result.data)
}

/// Decompresses a zlib or raw DEFLATE stream that holds no more than `max_len` bytes, and
/// reports how much of the input it occupied (`tinfl_decompress`, given all its input at
/// once).
///
/// Any input after the end of the stream is ignored, so streams placed one after another can
/// be read in turn:
///
/// ```
/// use miniz_nrp::{Format, Level, Options, Strategy, compress_with, decompress_with};
///
/// let options = Options::from_level(Level::DEFAULT, Format::Raw, Strategy::Default);
/// let mut input = compress_with(b"first", &options);
/// input.extend(compress_with(b"second", &options));
///
/// let first = decompress_with(&input, Format::Raw, 16).unwrap();
/// assert_eq!(first.data, b"first");
/// let second = decompress_with(&input[first.bytes_read..], Format::Raw, 16).unwrap();
/// assert_eq!(second.data, b"second");
/// ```
///
/// # Errors
///
/// A [`DecompressError`] if the stream is cut short, is not valid, fails its checksum (zlib
/// only), or holds more than `max_len` bytes.
pub fn decompress_with(
    stream: &[u8],
    format: Format,
    max_len: usize,
) -> Result<Decompressed, DecompressError> {
    inflate::inflate(stream, format == Format::Zlib, max_len)
}

/// The result of a successful [`decompress_with`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decompressed {
    /// The decompressed bytes.
    pub data: Vec<u8>,
    /// The bytes of input the stream occupied. Any input after it was not read.
    pub bytes_read: usize,
}

/// Why a stream could not be decompressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DecompressError {
    /// The input ends before the stream does (`TINFL_STATUS_FAILED_CANNOT_MAKE_PROGRESS`).
    Truncated,
    /// The input is not a valid stream (`TINFL_STATUS_FAILED`).
    Invalid,
    /// The stream is complete, but its checksum is not that of the bytes it holds
    /// (`TINFL_STATUS_ADLER32_MISMATCH`).
    ChecksumMismatch,
    /// The stream holds more bytes than the limit allows (`TINFL_STATUS_HAS_MORE_OUTPUT`).
    OutputTooLarge,
}

impl fmt::Display for DecompressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            DecompressError::Truncated => "the compressed stream is truncated",
            DecompressError::Invalid => "invalid compressed data",
            DecompressError::ChecksumMismatch => "the stream's checksum does not match its data",
            DecompressError::OutputTooLarge => "the stream decompresses to more than the limit",
        })
    }
}

impl std::error::Error for DecompressError {}

/// The Adler-32 checksum of `data`, as zlib streams record it (`mz_adler32`).
#[must_use]
pub fn adler32(data: &[u8]) -> u32 {
    adler32_update(1, data)
}

/// Continues the Adler-32 checksum `adler` over `data`: the checksum of everything given so
/// far.
#[must_use]
pub fn adler32_update(adler: u32, data: &[u8]) -> u32 {
    // 5,552 bytes is the most the two sums can take before they must be reduced.
    const CHUNK: usize = 5552;
    let (mut s1, mut s2) = (adler & 0xFFFF, adler >> 16);
    for chunk in data.chunks(CHUNK) {
        for &byte in chunk {
            s1 += u32::from(byte);
            s2 += s1;
        }
        s1 %= 65521;
        s2 %= 65521;
    }
    (s2 << 16) | s1
}

/// The CRC-32 checksum of `data`, as gzip and PNG use it (`mz_crc32`).
#[must_use]
pub fn crc32(data: &[u8]) -> u32 {
    crc32_update(0, data)
}

/// Continues the CRC-32 checksum `crc` over `data`: the checksum of everything given so far.
#[must_use]
pub fn crc32_update(crc: u32, data: &[u8]) -> u32 {
    !data.iter().fold(!crc, |crc, &byte| {
        (crc >> 8) ^ tables::CRC_TABLE[((crc ^ u32::from(byte)) & 0xFF) as usize]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_checksums_are_the_known_ones() {
        assert_eq!(adler32(b""), 1);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        // In pieces as in one go, across the point where the Adler sums are reduced.
        let data: Vec<u8> = (0..20_000u32)
            .map(|i| (i * 7 + i / 255) as u8 | 0x80)
            .collect();
        let (a, b) = data.split_at(5553);
        assert_eq!(adler32_update(adler32(a), b), adler32(&data));
        assert_eq!(crc32_update(crc32(a), b), crc32(&data));
    }

    #[test]
    fn levels_are_0_to_10_or_the_default() {
        assert_eq!(Level::new(10), Some(Level::UBER));
        assert_eq!(Level::new(11), None);
        assert_eq!(Level::try_from(12), Err(UnsupportedLevel(12)));
        assert_eq!(Level::DEFAULT.get(), None);
        assert_eq!(Level::BEST.get(), Some(9));
        // The default searches as level 6 does and parses as level 3 does.
        let default = Options::default();
        let six = Options::from_level(Level::new(6).unwrap(), Format::Zlib, Strategy::Default);
        assert_eq!(default.max_probes, six.max_probes);
        assert!(default.greedy_parsing && !six.greedy_parsing);
    }

    fn sample() -> Vec<u8> {
        let mut data = Vec::new();
        let mut state = 7u32;
        for i in 0..70_000u32 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            // Runs, repeats and noise.
            data.push(match (i / 3000) % 4 {
                0 => (i / 50) as u8,
                1 => b"the quick brown fox "[(i % 20) as usize],
                2 => (state >> 24) as u8,
                _ => (state >> 30) as u8,
            });
        }
        data
    }

    #[test]
    fn data_round_trips_at_every_level_and_strategy() {
        let data = sample();
        let mut sizes = Vec::new();
        for level in [Level::DEFAULT]
            .into_iter()
            .chain((0..=10).filter_map(Level::new))
        {
            let stream = compress(&data, level);
            assert_eq!(decompress(&stream, data.len()).unwrap(), data, "{level:?}");
            sizes.push(stream.len());
            for strategy in [
                Strategy::Filtered,
                Strategy::HuffmanOnly,
                Strategy::Rle,
                Strategy::Fixed,
            ] {
                for format in [Format::Zlib, Format::Raw] {
                    let options = Options::from_level(level, format, strategy);
                    let stream = compress_with(&data, &options);
                    let result = decompress_with(&stream, format, data.len()).unwrap();
                    assert_eq!(result.data, data, "{level:?} {strategy:?} {format:?}");
                    assert_eq!(result.bytes_read, stream.len());
                }
            }
        }
        // Level 0 stores, and level 10 is the smallest.
        assert!(sizes[1] > data.len() && sizes[11] < data.len() / 2 && sizes[11] <= sizes[2]);
        // Nothing in, a stream of nothing out.
        assert_eq!(decompress(&compress(&[], Level::BEST), 0).unwrap(), b"");
    }

    #[test]
    fn bad_streams_are_refused_with_the_reason() {
        let data = sample();
        let stream = compress(&data, Level::new(6).unwrap());
        assert_eq!(
            decompress(&stream, data.len() - 1),
            Err(DecompressError::OutputTooLarge)
        );
        assert_eq!(
            decompress(&stream[..stream.len() - 1], data.len()),
            Err(DecompressError::Truncated)
        );
        let mut damaged = stream.clone();
        *damaged.last_mut().unwrap() ^= 1;
        assert_eq!(
            decompress(&damaged, data.len()),
            Err(DecompressError::ChecksumMismatch)
        );
        assert_eq!(
            decompress(b"\x78\x9C\xFF", 100),
            Err(DecompressError::Invalid)
        );
    }

    #[test]
    fn damaged_streams_never_give_more_than_the_limit() {
        // Random bytes, and valid streams with bytes changed: whatever comes of them, nothing
        // panics and no more than the limit comes out.
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) as u32
        };
        let options = Options {
            format: Format::Raw,
            max_probes: 128,
            greedy_parsing: false,
            ..Options::default()
        };
        let good = compress_with(
            &(0..3000u32)
                .map(|i| (i % 251) as u8 ^ (i / 97) as u8)
                .collect::<Vec<u8>>(),
            &options,
        );
        for i in 0..6000 {
            let stream: Vec<u8> = if i % 2 == 0 {
                (0..=(next() % 80)).map(|_| next() as u8).collect()
            } else {
                let mut stream = good.clone();
                for _ in 0..=(next() % 4) {
                    let at = next() as usize % stream.len();
                    stream[at] = next() as u8;
                }
                stream
            };
            let limit = next() as usize % 5000;
            if let Ok(result) = decompress_with(&stream, Format::Raw, limit) {
                assert!(result.data.len() <= limit && result.bytes_read <= stream.len());
            }
            let _ = decompress(&stream, limit);
        }
    }

    #[test]
    fn input_in_pieces_gives_the_same_stream() {
        let data: Vec<u8> = (0..100_000u32)
            .map(|i| (i.wrapping_mul(i) >> 7) as u8 ^ (i / 1000) as u8)
            .collect();
        let options = Options::from_level(Level::new(6).unwrap(), Format::Zlib, Strategy::Default);
        let whole = compress_with(&data, &options);
        let mut compressor = Compressor::new(&options);
        for piece in data.chunks(777) {
            compressor.compress(piece, Flush::None).unwrap();
            assert!(!compressor.is_finished());
        }
        compressor.compress(&[], Flush::Finish).unwrap();
        assert!(compressor.is_finished());
        assert_eq!(compressor.adler32(), adler32(&data));
        assert_eq!(compressor.take_output(), whole);
        // Once finished, it accepts no more.
        assert_eq!(
            compressor.compress(b"more", Flush::Finish),
            Err(AlreadyFinished)
        );
        assert_eq!(compressor.take_output(), b"");
        // A sync flush in the middle leaves a stream that still holds everything.
        let mut compressor = Compressor::new(&options);
        compressor.compress(&data[..40_000], Flush::Sync).unwrap();
        let flushed = compressor.output().len();
        assert_eq!(
            decompress(compressor.output(), data.len()),
            Err(DecompressError::Truncated)
        );
        compressor.compress(&data[40_000..], Flush::Finish).unwrap();
        let stream = compressor.take_output();
        assert!(stream.len() > flushed);
        assert_eq!(decompress(&stream, data.len()).unwrap(), data);
    }
}

/// The README's examples, compiled and run as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

/// The Guide's examples, compiled and run as doctests.
#[cfg(doctest)]
#[doc = include_str!("../docs/guide.md")]
struct GuideDoctests;
