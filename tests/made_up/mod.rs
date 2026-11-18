//! Synthetic data, generated from a seed exactly as the C side of the comparison
//! (`compare/dump_miniz.c`) generates it, and the dump that both the golden tests and
//! `examples/dump.rs` run.

// (The golden tests and the dump each use only part of this module.)
#![allow(dead_code)]

use std::fmt::Write;

use miniz_nrp::{
    AlreadyFinished, Compressor, DecompressError, Flush, Format, Level, Options, Strategy, adler32,
    compress, compress_with, crc32, decompress_with,
};

/// The two programs make the same blocks from the same seed: a 64-bit linear congruential
/// generator, the high half of its state.
pub struct Random {
    pub state: u64,
}

impl Random {
    pub fn next(&mut self) -> u32 {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.state >> 32) as u32
    }
}

/// FNV-1a, 64 bits.
pub struct Hash {
    pub value: u64,
}

impl Hash {
    pub fn new() -> Hash {
        Hash {
            value: 14_695_981_039_346_656_037,
        }
    }

    pub fn add(&mut self, data: &[u8]) {
        for &byte in data {
            self.value ^= u64::from(byte);
            self.value = self.value.wrapping_mul(1_099_511_628_211);
        }
    }
}

const WORDS: [&str; 16] = [
    "the", "texture", "block", "of", "and", "compress", "a", "level", "mip", "to", "pixels", "in",
    "format", "is", "cube", "data",
];

/// Synthetic data of one of eight kinds, the same in both programs.
pub fn make_data(r: &mut Random) -> Vec<u8> {
    let len = match r.next() % 8 {
        0 => r.next() % 16,
        1 => r.next() % 300,
        2 | 3 => r.next() % 5000,
        4 | 5 => r.next() % 40000,
        6 => 60000 + r.next() % 20000,
        _ => r.next() % 200_000,
    } as usize;
    let kind = r.next() % 8;
    let mut buf = Vec::with_capacity(len);
    match kind {
        // noise
        0 => {
            for _ in 0..len {
                buf.push((r.next() >> 13) as u8);
            }
        }
        // four letters
        1 => {
            for _ in 0..len {
                buf.push(b'a' + (r.next() % 4) as u8);
            }
        }
        // words
        2 => {
            while buf.len() < len {
                let word = WORDS[(r.next() % 16) as usize];
                for &byte in word.as_bytes() {
                    if buf.len() < len {
                        buf.push(byte);
                    }
                }
                if buf.len() < len {
                    buf.push(b' ');
                }
            }
        }
        // runs
        3 => {
            while buf.len() < len {
                let byte = r.next() as u8;
                let run = 1 + r.next() % 300;
                for _ in 0..run {
                    if buf.len() < len {
                        buf.push(byte);
                    }
                }
            }
        }
        // copies of what came before
        4 => {
            while buf.len() < len {
                if buf.len() < 64 || r.next().is_multiple_of(4) {
                    buf.push((r.next() >> 9) as u8);
                } else {
                    let reach = buf.len().min(40000);
                    let dist = 1 + r.next() as usize % reach;
                    let n = 3 + r.next() % 260;
                    for _ in 0..n {
                        if buf.len() < len {
                            buf.push(buf[buf.len() - dist]);
                        }
                    }
                }
            }
        }
        // nothing
        5 => buf.resize(len, 0),
        // a slope
        6 => {
            for i in 0..len {
                buf.push((i / 7 + i % 13) as u8);
            }
        }
        // stretches of noise, of one value and of counting
        _ => {
            while buf.len() < len {
                let (stretch, what, value) = (1 + r.next() % 2000, r.next() % 3, r.next());
                for i in 0..stretch {
                    if buf.len() < len {
                        buf.push(match what {
                            0 => (r.next() >> 11) as u8,
                            1 => value as u8,
                            _ => value.wrapping_add(i) as u8,
                        });
                    }
                }
            }
        }
    }
    buf
}

/// A level from -1 (miniz's default) to 10, as the C side numbers them.
fn level(number: i32) -> Level {
    if number < 0 {
        Level::DEFAULT
    } else {
        Level::new(number as u8).unwrap()
    }
}

/// Compression options of three kinds: those of a level, strategy and format, sometimes with
/// the checksum asked for; a random search depth with random flags; and a very shallow search.
fn make_options(r: &mut Random) -> Options {
    match r.next() % 4 {
        0 | 1 => {
            let level = level((r.next() % 12) as i32 - 1);
            let strategy = if !r.next().is_multiple_of(3) {
                0
            } else {
                r.next() % 5
            };
            let strategy = [
                Strategy::Default,
                Strategy::Filtered,
                Strategy::HuffmanOnly,
                Strategy::Rle,
                Strategy::Fixed,
            ][strategy as usize];
            let format = if r.next() & 1 != 0 {
                Format::Zlib
            } else {
                Format::Raw
            };
            let mut options = Options::from_level(level, format, strategy);
            if r.next() & 1 != 0 {
                options.compute_adler32 = true;
            }
            options
        }
        2 => {
            let max_probes = (r.next() % 4096) as u16;
            let mut flag = || r.next().trailing_zeros() >= 2;
            Options {
                max_probes,
                format: if flag() { Format::Zlib } else { Format::Raw },
                compute_adler32: flag(),
                greedy_parsing: flag(),
                rle_matches: flag(),
                filter_matches: flag(),
                force_static_blocks: flag(),
                force_raw_blocks: flag(),
            }
        }
        _ => {
            let max_probes = (r.next() % 8) as u16;
            Options {
                max_probes,
                format: if r.next() & 1 != 0 {
                    Format::Zlib
                } else {
                    Format::Raw
                },
                greedy_parsing: false,
                compute_adler32: false,
                rle_matches: false,
                filter_matches: false,
                force_static_blocks: false,
                force_raw_blocks: false,
            }
        }
    }
}

/// Decompresses with all the input and all the room at once and adds the outcome to the
/// hash: the status, as miniz numbers it, and when it succeeds, the input it took and the
/// bytes. True when it succeeds.
fn miniz_decompress(hash: &mut Hash, stream: &[u8], room: usize, format: Format) -> bool {
    match decompress_with(stream, format, room) {
        Ok(result) => {
            hash.add(&0u32.to_le_bytes());
            hash.add(&(result.bytes_read as u32).to_le_bytes());
            hash.add(&(result.data.len() as u32).to_le_bytes());
            hash.add(&result.data);
            true
        }
        Err(error) => {
            let status: i32 = match error {
                DecompressError::Truncated => -4,
                DecompressError::ChecksumMismatch => -2,
                DecompressError::Invalid => -1,
                DecompressError::OutputTooLarge => 2,
            };
            hash.add(&status.to_le_bytes());
            false
        }
    }
}

/// The status miniz's `tdefl_compress_buffer` gives: 0 to go on, 1 when done, -2 when the
/// stream was already done.
fn status(compressor: &Compressor, result: Result<(), AlreadyFinished>) -> i32 {
    match result {
        Ok(()) if compressor.is_finished() => 1,
        Ok(()) => 0,
        Err(AlreadyFinished) => -2,
    }
}

/// The `miniz` dump: synthetic data compressed with the options of every level and strategy
/// and with random options, in one call and in pieces with flushes between; each stream
/// decompressed whole, into too little room, damaged, truncated and with bytes after it; and
/// random bytes decompressed.
pub fn miniz(seed: u64, count: u32) -> String {
    let mut r = Random { state: seed };
    let mut hash = Hash::new();
    let (mut done, mut refused) = (0u32, 0u32);
    let (mut in_total, mut out_total) = (0u64, 0u64);
    let mut out = String::new();
    let mut tally = |ok: bool| if ok { done += 1 } else { refused += 1 };
    for i in 0..count {
        let data = make_data(&mut r);
        let len = data.len();
        let options = make_options(&mut r);
        let format = options.format;
        let stream = compress_with(&data, &options);
        hash.add(&(stream.len() as u32).to_le_bytes());
        hash.add(&stream);
        in_total += len as u64;
        out_total += stream.len() as u64;

        // The checksums, and what mz_compress2 writes for a level.
        hash.add(&adler32(&data).to_le_bytes());
        hash.add(&crc32(&data).to_le_bytes());
        if i % 4 == 0 {
            let level = level((r.next() % 12) as i32 - 1);
            let packed = compress(&data, level);
            hash.add(&(packed.len() as u32).to_le_bytes());
            hash.add(&packed);
        }

        // In pieces, with flushes between.
        let mut comp = Compressor::new(&options);
        let mut pos = 0;
        while pos < len {
            let n = (1 + r.next() as usize % 5000).min(len - pos);
            let how = match r.next() % 8 {
                0 => Flush::Sync,
                1 => Flush::Full,
                _ => Flush::None,
            };
            let result = comp.compress(&data[pos..pos + n], how);
            hash.add(&status(&comp, result).to_le_bytes());
            pos += n;
        }
        let result = comp.compress(&[], Flush::Finish);
        hash.add(&status(&comp, result).to_le_bytes());
        hash.add(&comp.adler32().to_le_bytes());
        let pieces = comp.take_output();
        hash.add(&(pieces.len() as u32).to_le_bytes());
        hash.add(&pieces);
        tally(miniz_decompress(&mut hash, &pieces, len, format));

        // The stream back: whole, into too little room, damaged, cut short, with more after.
        tally(miniz_decompress(&mut hash, &stream, len, format));
        if len > 0 {
            miniz_decompress(
                &mut hash,
                &stream,
                len - 1 - r.next() as usize % len,
                format,
            );
        }
        let mut copy = stream.clone();
        for _ in 0..=(r.next() % 3) {
            let bit = r.next() % (stream.len() as u32 * 8);
            copy[(bit >> 3) as usize] ^= 1 << (bit & 7);
        }
        tally(miniz_decompress(&mut hash, &copy, len + 64, format));
        miniz_decompress(
            &mut hash,
            &stream[..r.next() as usize % stream.len()],
            len,
            format,
        );
        let mut copy = stream.clone();
        for _ in 0..4 {
            copy.push(r.next() as u8);
        }
        miniz_decompress(&mut hash, &copy, len, format);

        // Random bytes as a stream.
        let mut garbage: Vec<u8> = (0..=(r.next() % 64))
            .map(|_| (r.next() >> 7) as u8)
            .collect();
        if r.next() & 1 != 0 {
            // (A zlib header in front, so that it gets past that.)
            garbage[0] = 0x78;
            if garbage.len() > 1 {
                garbage[1] = 0x9C;
            }
        }
        tally(miniz_decompress(
            &mut hash,
            &garbage,
            4096,
            if garbage[0] == 0x78 {
                Format::Zlib
            } else {
                Format::Raw
            },
        ));

        if (i + 1) % 200 == 0 || i + 1 == count {
            writeln!(out, "miniz {} {:016x}", i + 1, hash.value).unwrap();
        }
    }
    out + &format!(
        "miniz bytes in: {in_total}, out: {out_total}; streams decompressed: {done}, refused: {refused}\n"
    )
}
