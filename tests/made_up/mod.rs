//! Made-up data, the same from a seed here as in the C side of the comparison
//! (`compare/`), and the dumps the tests run as well as the comparison (`examples/dump.rs`).

// (The arithmetic is written as the C side has it; not every user of this module uses all of
// it.)
#![allow(
    clippy::manual_is_multiple_of,
    clippy::needless_range_loop,
    clippy::manual_div_ceil,
    clippy::manual_checked_ops,
    dead_code
)]

/// The two programs make the same blocks from the same seed: a 64-bit linear congruential
/// generator, the high half of its state.
pub struct Random {
    pub state: u64,
}

impl Random {
    pub fn next(&mut self) -> u32 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
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
            value: 14695981039346656037,
        }
    }

    pub fn add(&mut self, data: &[u8]) {
        for &byte in data {
            self.value ^= u64::from(byte);
            self.value = self.value.wrapping_mul(1099511628211);
        }
    }
}

const WORDS: [&str; 16] = [
    "the", "texture", "block", "of", "and", "compress", "a", "level", "mip", "to", "pixels", "in",
    "format", "is", "cube", "data",
];

/// Made-up data of one of eight kinds, the same in both programs.
pub fn make_data(r: &mut Random) -> Vec<u8> {
    let len = match r.next() % 8 {
        0 => r.next() % 16,
        1 => r.next() % 300,
        2 | 3 => r.next() % 5000,
        4 | 5 => r.next() % 40000,
        6 => 60000 + r.next() % 20000,
        _ => r.next() % 200000,
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
                if buf.len() < 64 || r.next() % 4 == 0 {
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

fn make_tdefl_flags(r: &mut Random) -> u32 {
    use miniz_nrp::tdef::*;
    match r.next() % 4 {
        0 | 1 => {
            let level = (r.next() % 12) as i32 - 1;
            let strategy = if r.next() % 3 != 0 {
                0
            } else {
                (r.next() % 5) as i32
            };
            let window_bits = if r.next() & 1 != 0 { 15 } else { -15 };
            let mut flags = tdefl_create_comp_flags_from_zip_params(level, window_bits, strategy);
            if r.next() & 1 != 0 {
                flags |= TDEFL_COMPUTE_ADLER32;
            }
            flags
        }
        2 => {
            let mut flags = r.next() % 4096;
            for flag in [
                TDEFL_WRITE_ZLIB_HEADER,
                TDEFL_COMPUTE_ADLER32,
                TDEFL_GREEDY_PARSING_FLAG,
                TDEFL_RLE_MATCHES,
                TDEFL_FILTER_MATCHES,
                TDEFL_FORCE_ALL_STATIC_BLOCKS,
                TDEFL_FORCE_ALL_RAW_BLOCKS,
            ] {
                if r.next() & 3 == 0 {
                    flags |= flag;
                }
            }
            flags
        }
        _ => {
            let mut flags = r.next() % 8;
            if r.next() & 1 != 0 {
                flags |= TDEFL_WRITE_ZLIB_HEADER;
            }
            flags
        }
    }
}

/// Decompresses with all the input and all the room at once and adds what came of it to the
/// hash: the status, and when it is done how much of the input it took and the bytes. True
/// when it is done.
fn miniz_decompress(hash: &mut Hash, stream: &[u8], room: usize, zlib: bool) -> bool {
    use miniz_nrp::tinfl::*;
    match tinfl_decompress(
        stream,
        room,
        if zlib {
            TINFL_FLAG_PARSE_ZLIB_HEADER
        } else {
            0
        },
    ) {
        Ok((bytes, taken)) => {
            hash.add(&0u32.to_le_bytes());
            hash.add(&(taken as u32).to_le_bytes());
            hash.add(&(bytes.len() as u32).to_le_bytes());
            hash.add(&bytes);
            true
        }
        Err(error) => {
            let status: i32 = match error {
                TinflError::CannotMakeProgress => -4,
                TinflError::Adler32Mismatch => -2,
                TinflError::Failed => -1,
                TinflError::HasMoreOutput => 2,
            };
            hash.add(&status.to_le_bytes());
            false
        }
    }
}

/// The `miniz` dump: made-up data compressed with flags from every level and strategy and with
/// made-up flags, in one go and in pieces with flushes between; each stream decompressed
/// whole, into too little room, damaged, cut short and with bytes after it; and made-up bytes
/// decompressed.
pub fn miniz(seed: u64, count: u32) -> String {
    use miniz_nrp::tdef::*;
    use miniz_nrp::{MZ_ADLER32_INIT, MZ_CRC32_INIT, compress, mz_adler32, mz_crc32};
    let mut r = Random { state: seed };
    let mut hash = Hash::new();
    let (mut done, mut refused) = (0u32, 0u32);
    let (mut in_total, mut out_total) = (0u64, 0u64);
    let mut out = String::new();
    let mut tally = |ok: bool| if ok { done += 1 } else { refused += 1 };
    for i in 0..count {
        let data = make_data(&mut r);
        let len = data.len();
        let flags = make_tdefl_flags(&mut r);
        let zlib = flags & TDEFL_WRITE_ZLIB_HEADER != 0;
        let stream = tdefl_compress_mem_to_heap(&data, flags);
        hash.add(&(stream.len() as u32).to_le_bytes());
        hash.add(&stream);
        in_total += len as u64;
        out_total += stream.len() as u64;

        // The checksums, and what mz_compress2 writes for a level.
        hash.add(&mz_adler32(MZ_ADLER32_INIT, &data).to_le_bytes());
        hash.add(&mz_crc32(MZ_CRC32_INIT, &data).to_le_bytes());
        if i % 4 == 0 {
            let level = (r.next() % 12) as i32 - 1;
            let packed = compress(&data, level);
            hash.add(&(packed.len() as u32).to_le_bytes());
            hash.add(&packed);
        }

        // In pieces, with flushes between.
        let mut comp = TdeflCompressor::new(flags);
        let mut pos = 0;
        while pos < len {
            let n = (1 + r.next() as usize % 5000).min(len - pos);
            let how = match r.next() % 8 {
                0 => TdeflFlush::SyncFlush,
                1 => TdeflFlush::FullFlush,
                _ => TdeflFlush::NoFlush,
            };
            hash.add(&(comp.compress_buffer(&data[pos..pos + n], how) as i32).to_le_bytes());
            pos += n;
        }
        hash.add(&(comp.compress_buffer(&[], TdeflFlush::Finish) as i32).to_le_bytes());
        hash.add(&comp.adler32().to_le_bytes());
        let pieces = comp.take_output();
        hash.add(&(pieces.len() as u32).to_le_bytes());
        hash.add(&pieces);
        tally(miniz_decompress(&mut hash, &pieces, len, zlib));

        // The stream back: whole, into too little room, damaged, cut short, with more after.
        tally(miniz_decompress(&mut hash, &stream, len, zlib));
        if len > 0 {
            miniz_decompress(&mut hash, &stream, len - 1 - r.next() as usize % len, zlib);
        }
        let mut copy = stream.clone();
        for _ in 0..1 + r.next() % 3 {
            let bit = r.next() % (stream.len() as u32 * 8);
            copy[(bit >> 3) as usize] ^= 1 << (bit & 7);
        }
        tally(miniz_decompress(&mut hash, &copy, len + 64, zlib));
        miniz_decompress(
            &mut hash,
            &stream[..r.next() as usize % stream.len()],
            len,
            zlib,
        );
        let mut copy = stream.clone();
        for _ in 0..4 {
            copy.push(r.next() as u8);
        }
        miniz_decompress(&mut hash, &copy, len, zlib);

        // Made-up bytes as a stream.
        let mut garbage: Vec<u8> = (0..1 + r.next() % 64)
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
            garbage[0] == 0x78,
        ));

        if (i + 1) % 200 == 0 || i + 1 == count {
            out += &format!("miniz {} {:016x}\n", i + 1, hash.value);
        }
    }
    out + &format!(
        "miniz bytes in: {in_total}, out: {out_total}; streams decompressed: {done}, refused: {refused}\n"
    )
}
