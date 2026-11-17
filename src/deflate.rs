//! The compressor (`miniz_tdef.c`, as miniz builds it by default).
//!
//! Input goes into a 32 KiB sliding window with hash chains of three-byte prefixes. Each
//! position is parsed into a literal or a match, greedily or lazily, and recorded as an LZ code
//! with the symbol frequencies it uses. When the code buffer is nearly full, or the block is
//! large and compresses poorly, the block is written with Huffman codes built for it, with the
//! fixed codes, or stored, whichever the options and the result call for.
//!
//! Every decision is miniz's, in miniz's order, including the bounds of its internal buffers,
//! which decide where blocks end: the streams are miniz's byte for byte.

use crate::tables::{
    LARGE_DIST_EXTRA, LARGE_DIST_SYM, LEN_EXTRA, LEN_SYM, SMALL_DIST_EXTRA, SMALL_DIST_SYM,
};
use crate::{Format, Options, adler32_update};
use core::fmt;

/// The sliding window.
const DICT_SIZE: usize = 32 * 1024;
const DICT_MASK: u32 = DICT_SIZE as u32 - 1;
const MIN_MATCH_LEN: u32 = 3;
const MAX_MATCH_LEN: u32 = 258;

/// The LZ codes of a block: up to 64 KiB, a flag byte for every eight codes.
const LZ_CODES_SIZE: usize = 64 * 1024;
/// The most bytes one block may write, which must hold a whole block in the fixed codes.
const BLOCK_OUT_SIZE: usize = LZ_CODES_SIZE * 13 / 10;

const HASH_BITS: u32 = 15;
const HASH_SHIFT: u32 = HASH_BITS.div_ceil(3);
const HASH_SIZE: usize = 1 << HASH_BITS;

const NUM_LITLEN_SYMS: usize = 288;
const NUM_DIST_SYMS: usize = 32;
const NUM_PRECODE_SYMS: usize = 19;

/// The order in which a dynamic block's header lists the code lengths of its precode.
const PRECODE_ORDER: [u8; NUM_PRECODE_SYMS] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// What to do with the input given so far, after a call to [`Compressor::compress`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Flush {
    /// Nothing: wait for more input (`TDEFL_NO_FLUSH`).
    #[default]
    None,
    /// End the current block and pad to a whole byte with an empty stored block, so that
    /// everything given so far can be decompressed from the output so far
    /// (`TDEFL_SYNC_FLUSH`).
    Sync,
    /// As [`Flush::Sync`], and also forget the input so far, so that decompression could start
    /// again from this point (`TDEFL_FULL_FLUSH`).
    Full,
    /// This was the last of the input: end the stream (`TDEFL_FINISH`).
    Finish,
}

/// The error for input given to a [`Compressor`] that has already finished its stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AlreadyFinished;

impl fmt::Display for AlreadyFinished {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the compressor has already finished its stream")
    }
}

impl std::error::Error for AlreadyFinished {}

/// A compressor for input that arrives in pieces (`tdefl_compressor`).
///
/// The stream does not depend on how the input is divided, only on where it is flushed.
///
/// ```
/// use miniz_nrp::{Compressor, Flush, Options, decompress};
///
/// let mut compressor = Compressor::new(&Options::default());
/// compressor.compress(b"Hello, ", Flush::None).unwrap();
/// compressor.compress(b"world!", Flush::Finish).unwrap();
/// let stream = compressor.take_output();
/// assert_eq!(decompress(&stream, 13).unwrap(), b"Hello, world!");
/// ```
pub struct Compressor {
    options: Options,
    /// The candidates to examine in a search, before and after a match of 32 bytes is found.
    max_probes: [u32; 2],
    adler32: u32,
    bits: BitWriter,
    /// The window, followed by a copy of its first `MAX_MATCH_LEN - 1` bytes, so that a match
    /// can be read past the end of the window without wrapping.
    dict: Vec<u8>,
    /// For each window position, the previous position with the same hash.
    chain: Vec<u16>,
    /// For each hash, the latest position with it.
    hash_heads: Vec<u16>,
    /// The position of the first byte not yet parsed, and the bytes from there on in the
    /// window.
    lookahead_pos: u32,
    lookahead_len: u32,
    /// The bytes before `lookahead_pos` that matches may refer to.
    dict_len: u32,
    lz: LzCodes,
    /// The bytes the current block covers, and the window position where it starts.
    block_len: u32,
    block_start: u32,
    blocks_written: u32,
    /// A match held back by lazy parsing while the next position is tried.
    deferred: Option<Deferred>,
    litlen: HuffmanTable<NUM_LITLEN_SYMS>,
    dist: HuffmanTable<NUM_DIST_SYMS>,
    precode: HuffmanTable<NUM_PRECODE_SYMS>,
    finished: bool,
}

/// A match found at a position whose byte is `literal`, held back in case the next position
/// has a longer one.
#[derive(Clone, Copy)]
struct Deferred {
    literal: u8,
    dist: u32,
    len: u32,
}

impl Compressor {
    /// A compressor that compresses with `options` (`tdefl_init`).
    pub fn new(options: &Options) -> Compressor {
        let probes = u32::from(options.max_probes.min(0xFFF));
        Compressor {
            options: *options,
            max_probes: [1 + probes.div_ceil(3), 1 + (probes >> 2).div_ceil(3)],
            adler32: 1,
            bits: BitWriter::default(),
            dict: vec![0; DICT_SIZE + MAX_MATCH_LEN as usize - 1],
            chain: vec![0; DICT_SIZE],
            hash_heads: vec![0; HASH_SIZE],
            lookahead_pos: 0,
            lookahead_len: 0,
            dict_len: 0,
            lz: LzCodes::new(),
            block_len: 0,
            block_start: 0,
            blocks_written: 0,
            deferred: None,
            litlen: HuffmanTable::new(),
            dist: HuffmanTable::new(),
            precode: HuffmanTable::new(),
            finished: false,
        }
    }

    /// Compresses all of `input`, then does what `flush` asks (`tdefl_compress_buffer`).
    ///
    /// The compressed bytes collect in the compressor until [`Compressor::take_output`] takes
    /// them. [`Flush::Finish`] ends the stream, after which the compressor accepts nothing
    /// more.
    pub fn compress(&mut self, input: &[u8], flush: Flush) -> Result<(), AlreadyFinished> {
        if self.finished {
            return Err(AlreadyFinished);
        }
        self.compress_input(input, flush);
        if self.options.format == Format::Zlib || self.options.compute_adler32 {
            self.adler32 = adler32_update(self.adler32, input);
        }
        if flush != Flush::None {
            debug_assert_eq!(self.lookahead_len, 0);
            self.flush_block(flush);
            self.finished = flush == Flush::Finish;
            if flush == Flush::Full {
                self.hash_heads.fill(0);
                self.chain.fill(0);
                self.dict_len = 0;
            }
        }
        Ok(())
    }

    /// Whether the stream has been finished with [`Flush::Finish`].
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// The compressed bytes not yet taken.
    pub fn output(&self) -> &[u8] {
        &self.bits.out
    }

    /// Takes the compressed bytes made so far.
    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.bits.out)
    }

    /// The Adler-32 checksum of the input so far (`tdefl_get_adler32`): computed for zlib
    /// streams, and for raw DEFLATE when [`Options::compute_adler32`] asks for it, and 1
    /// otherwise.
    pub fn adler32(&self) -> u32 {
        self.adler32
    }

    /// Takes the input into the window and parses it into LZ codes, writing blocks as the code
    /// buffer fills. Unless flushing, the last bytes stay in the lookahead for the next call.
    fn compress_input(&mut self, input: &[u8], flush: Flush) {
        let mut src = input;
        while !src.is_empty() || (flush != Flush::None && self.lookahead_len != 0) {
            self.fill_lookahead(&mut src);
            if flush == Flush::None && self.lookahead_len < MAX_MATCH_LEN {
                break;
            }
            let len_to_move = self.parse_next();
            self.lookahead_pos = self.lookahead_pos.wrapping_add(len_to_move);
            debug_assert!(self.lookahead_len >= len_to_move);
            self.lookahead_len -= len_to_move;
            self.dict_len = (self.dict_len + len_to_move).min(DICT_SIZE as u32);
            // Time to write the block?
            if self.lz.pos > LZ_CODES_SIZE - 8
                || (self.block_len > 31 * 1024
                    && (((self.lz.pos as u32 * 115) >> 7) >= self.block_len
                        || self.options.force_raw_blocks))
            {
                self.flush_block(Flush::None);
            }
        }
    }

    /// Moves bytes from `src` into the window until the lookahead holds `MAX_MATCH_LEN`,
    /// inserting each position into the hash chains once its three bytes are known.
    fn fill_lookahead(&mut self, src: &mut &[u8]) {
        let end = self.lookahead_pos.wrapping_add(self.lookahead_len);
        if self.lookahead_len + self.dict_len >= MIN_MATCH_LEN - 1 {
            // Two bytes before the first new one are already in place.
            let n = src.len().min((MAX_MATCH_LEN - self.lookahead_len) as usize);
            let (taken, rest) = src.split_at(n);
            *src = rest;
            self.lookahead_len += n as u32;
            let mut dst = (end & DICT_MASK) as usize;
            let mut ins_pos = end.wrapping_sub(2);
            let mut hash = (u32::from(self.dict[(ins_pos & DICT_MASK) as usize]) << HASH_SHIFT)
                ^ u32::from(self.dict[(ins_pos.wrapping_add(1) & DICT_MASK) as usize]);
            for &c in taken {
                self.put_in_window(dst, c);
                hash = ((hash << HASH_SHIFT) ^ u32::from(c)) & (HASH_SIZE as u32 - 1);
                self.insert(ins_pos, hash);
                dst = (dst + 1) & DICT_MASK as usize;
                ins_pos = ins_pos.wrapping_add(1);
            }
        } else {
            // The first bytes of the stream, or of the window after a full flush.
            while let Some((&c, rest)) = src.split_first() {
                if self.lookahead_len >= MAX_MATCH_LEN {
                    break;
                }
                *src = rest;
                let dst =
                    (self.lookahead_pos.wrapping_add(self.lookahead_len) & DICT_MASK) as usize;
                self.put_in_window(dst, c);
                self.lookahead_len += 1;
                if self.lookahead_len + self.dict_len >= MIN_MATCH_LEN {
                    let ins_pos = self
                        .lookahead_pos
                        .wrapping_add(self.lookahead_len - 1)
                        .wrapping_sub(2);
                    let hash = ((u32::from(self.dict[(ins_pos & DICT_MASK) as usize])
                        << (HASH_SHIFT * 2))
                        ^ (u32::from(self.dict[(ins_pos.wrapping_add(1) & DICT_MASK) as usize])
                            << HASH_SHIFT)
                        ^ u32::from(c))
                        & (HASH_SIZE as u32 - 1);
                    self.insert(ins_pos, hash);
                }
            }
        }
        self.dict_len = (DICT_SIZE as u32 - self.lookahead_len).min(self.dict_len);
    }

    #[inline]
    fn put_in_window(&mut self, at: usize, byte: u8) {
        self.dict[at] = byte;
        if at < MAX_MATCH_LEN as usize - 1 {
            self.dict[DICT_SIZE + at] = byte;
        }
    }

    #[inline]
    fn insert(&mut self, pos: u32, hash: u32) {
        self.chain[(pos & DICT_MASK) as usize] = self.hash_heads[hash as usize];
        self.hash_heads[hash as usize] = pos as u16;
    }

    /// Parses the position at the start of the lookahead, and gives back how many bytes to
    /// move past.
    fn parse_next(&mut self) -> u32 {
        let options = self.options;
        let cur_pos = self.lookahead_pos & DICT_MASK;
        let (mut dist, mut len) = (0, self.deferred.map_or(MIN_MATCH_LEN - 1, |d| d.len));
        if options.rle_matches || options.force_raw_blocks {
            if self.dict_len != 0 && !options.force_raw_blocks {
                let previous = self.dict[(cur_pos.wrapping_sub(1) & DICT_MASK) as usize];
                let at = cur_pos as usize;
                len = self.dict[at..at + self.lookahead_len as usize]
                    .iter()
                    .take_while(|&&b| b == previous)
                    .count() as u32;
                if len < MIN_MATCH_LEN {
                    len = 0;
                } else {
                    dist = 1;
                }
            }
        } else {
            (dist, len) = self.find_match(dist, len);
        }
        // A three-byte match far back costs more than its literals.
        if (len == MIN_MATCH_LEN && dist >= 8 * 1024)
            || cur_pos == dist
            || (options.filter_matches && len <= 5)
        {
            (dist, len) = (0, 0);
        }
        let byte = self.dict[cur_pos as usize];
        match self.deferred {
            Some(deferred) if len > deferred.len => {
                // The next position has the longer match: the deferred one's byte goes out as
                // a literal.
                self.record_literal(deferred.literal);
                if len >= 128 {
                    self.record_match(len, dist);
                    self.deferred = None;
                    len
                } else {
                    self.deferred = Some(Deferred {
                        literal: byte,
                        dist,
                        len,
                    });
                    1
                }
            }
            Some(deferred) => {
                self.record_match(deferred.len, deferred.dist);
                self.deferred = None;
                deferred.len - 1
            }
            None if dist == 0 => {
                self.record_literal(byte);
                1
            }
            None if options.greedy_parsing || options.rle_matches || len >= 128 => {
                self.record_match(len, dist);
                len
            }
            None => {
                self.deferred = Some(Deferred {
                    literal: byte,
                    dist,
                    len,
                });
                1
            }
        }
    }

    /// Searches the hash chain at the start of the lookahead for a match longer than `len`
    /// (`tdefl_find_match`), and gives back the best found: its distance and length.
    #[inline]
    fn find_match(&self, mut best_dist: u32, mut best_len: u32) -> (u32, u32) {
        let lookahead_pos = self.lookahead_pos;
        let (max_dist, max_len) = (self.dict_len, self.lookahead_len);
        debug_assert!(max_len <= MAX_MATCH_LEN);
        if max_len <= best_len {
            return (best_dist, best_len);
        }
        let dict = &self.dict[..];
        let pos = (lookahead_pos & DICT_MASK) as usize;
        let mut probe_pos = pos;
        let mut probes_left = self.max_probes[usize::from(best_len >= 32)];
        // The two bytes a longer match must have at the end of the best length so far.
        let mut c0 = dict[pos + best_len as usize];
        let mut c1 = dict[pos + best_len as usize - 1];
        loop {
            // Follow the chain to the next candidate with those two bytes, three links per
            // probe.
            let dist = 'probe: loop {
                probes_left -= 1;
                if probes_left == 0 {
                    return (best_dist, best_len);
                }
                for _ in 0..3 {
                    let next = u32::from(self.chain[probe_pos]);
                    if next == 0 {
                        return (best_dist, best_len);
                    }
                    let dist = u32::from(lookahead_pos.wrapping_sub(next) as u16);
                    if dist > max_dist {
                        return (best_dist, best_len);
                    }
                    probe_pos = (next & DICT_MASK) as usize;
                    if dict[probe_pos + best_len as usize] == c0
                        && dict[probe_pos + best_len as usize - 1] == c1
                    {
                        break 'probe dist;
                    }
                }
            };
            if dist == 0 {
                return (best_dist, best_len);
            }
            let len = common_prefix(
                &dict[pos..pos + max_len as usize],
                &dict[probe_pos..probe_pos + max_len as usize],
            ) as u32;
            if len > best_len {
                (best_dist, best_len) = (dist, len);
                if len == max_len {
                    return (best_dist, best_len);
                }
                c0 = dict[pos + len as usize];
                c1 = dict[pos + len as usize - 1];
            }
        }
    }

    #[inline]
    fn record_literal(&mut self, literal: u8) {
        self.block_len += 1;
        self.lz.push_literal(literal);
        self.litlen.count(usize::from(literal));
    }

    #[inline]
    fn record_match(&mut self, len: u32, dist: u32) {
        debug_assert!(len >= MIN_MATCH_LEN && (1..=DICT_SIZE as u32).contains(&dist));
        self.block_len += len;
        let (len, dist) = (len - MIN_MATCH_LEN, dist - 1);
        self.lz.push_match(len as u8, dist as u16);
        self.dist.count(dist_symbol(dist as u16).0);
        self.litlen.count(usize::from(LEN_SYM[len as usize]));
    }

    /// Writes the current block (`tdefl_flush_block`), then what `flush` asks for.
    fn flush_block(&mut self, flush: Flush) {
        // A stored block needs the whole block still in the window.
        let can_store = self.lookahead_pos.wrapping_sub(self.block_start) <= self.dict_len;
        let store = self.options.force_raw_blocks && can_store;
        self.bits.limit = self.bits.out.len() + BLOCK_OUT_SIZE - 16;
        self.bits.out.reserve(BLOCK_OUT_SIZE);
        self.lz.close();

        if self.options.format == Format::Zlib && self.blocks_written == 0 {
            let cmf = 0x78;
            let header = cmf << 8 | self.options.header_level() << 6;
            let flg = (header + 31 - header % 31) & 0xFF;
            self.bits.write(cmf, 8);
            self.bits.write(flg, 8);
        }

        self.bits.write(u32::from(flush == Flush::Finish), 1);

        let saved = self.bits.save();
        let mut fits = false;
        if !store {
            fits = self.write_block(self.options.force_static_blocks || self.block_len < 48);
        }
        let expanded =
            self.block_len != 0 && self.bits.out.len() - saved.len + 1 >= self.block_len as usize;
        if (store || expanded) && can_store {
            // Stored, in place of what was written.
            self.bits.restore(saved);
            self.bits.write(0, 2);
            self.bits.align();
            let len = self.block_len;
            self.bits.write(len & 0xFFFF, 16);
            self.bits.write(!len & 0xFFFF, 16);
            let start = (self.block_start & DICT_MASK) as usize;
            let first = (len as usize).min(DICT_SIZE - start);
            self.bits.write_aligned(&self.dict[start..start + first]);
            self.bits.write_aligned(&self.dict[..len as usize - first]);
        } else if !fits {
            // (Dynamic codes that overflow the output: in practice, never.)
            self.bits.restore(saved);
            self.write_block(true);
        }

        match flush {
            Flush::None => {}
            Flush::Finish => {
                self.bits.align();
                if self.options.format == Format::Zlib {
                    for byte in self.adler32.to_be_bytes() {
                        self.bits.write(u32::from(byte), 8);
                    }
                }
            }
            Flush::Sync | Flush::Full => {
                // An empty stored block.
                self.bits.write(0, 3);
                self.bits.align();
                self.bits.write(0, 16);
                self.bits.write(0xFFFF, 16);
            }
        }
        debug_assert!(self.bits.out.len() < self.bits.limit);

        self.litlen.counts.fill(0);
        self.dist.counts.fill(0);
        self.lz.clear();
        self.block_start = self.block_start.wrapping_add(self.block_len);
        self.block_len = 0;
        self.blocks_written += 1;
    }

    /// Writes the block's header and codes in the fixed codes or codes of its own, and says
    /// whether it fitted in the room for a block.
    fn write_block(&mut self, fixed_codes: bool) -> bool {
        if fixed_codes {
            self.start_fixed_block();
        } else {
            self.start_dynamic_block();
        }
        self.write_lz_codes()
    }

    fn start_fixed_block(&mut self) {
        let lens = &mut self.litlen.lens;
        lens[..144].fill(8);
        lens[144..256].fill(9);
        lens[256..280].fill(7);
        lens[280..].fill(8);
        self.dist.lens.fill(5);
        self.litlen.assign_codes_from_lens(15);
        self.dist.assign_codes_from_lens(15);
        self.bits.write(1, 2);
    }

    fn start_dynamic_block(&mut self) {
        self.litlen.counts[256] = 1;
        self.litlen.build(15);
        self.dist.build(15);

        let num_lit_codes = (257..286)
            .rev()
            .find(|&n| self.litlen.lens[n] != 0)
            .map_or(257, |n| n + 1);
        let num_dist_codes = (1..30)
            .rev()
            .find(|&n| self.dist.lens[n] != 0)
            .map_or(1, |n| n + 1);

        // The code lengths of both tables, run-length encoded in the precode's symbols.
        let lens = self.litlen.lens[..num_lit_codes]
            .iter()
            .chain(&self.dist.lens[..num_dist_codes]);
        self.precode.counts.fill(0);
        let mut rle = CodeLengthRle {
            counts: &mut self.precode.counts,
            packed: [0; NUM_LITLEN_SYMS + NUM_DIST_SYMS],
            len: 0,
            zeros: 0,
            repeats: 0,
        };
        let mut prev = 0xFF;
        for &len in lens {
            if len == 0 {
                rle.end_repeats(prev);
                rle.zeros += 1;
                if rle.zeros == 138 {
                    rle.end_zeros();
                }
            } else {
                rle.end_zeros();
                if len != prev {
                    rle.end_repeats(prev);
                    rle.push(len);
                } else {
                    rle.repeats += 1;
                    if rle.repeats == 6 {
                        rle.end_repeats(prev);
                    }
                }
            }
            prev = len;
        }
        if rle.repeats != 0 {
            rle.end_repeats(prev);
        } else {
            rle.end_zeros();
        }
        let (packed, packed_len) = (rle.packed, rle.len);

        self.precode.build(7);

        self.bits.write(2, 2);
        self.bits.write(num_lit_codes as u32 - 257, 5);
        self.bits.write(num_dist_codes as u32 - 1, 5);
        let num_precode_lens = PRECODE_ORDER
            .iter()
            .rposition(|&sym| self.precode.lens[usize::from(sym)] != 0)
            .map_or(0, |i| i + 1)
            .max(4);
        self.bits.write(num_precode_lens as u32 - 4, 4);
        for &sym in &PRECODE_ORDER[..num_precode_lens] {
            self.bits
                .write(u32::from(self.precode.lens[usize::from(sym)]), 3);
        }
        let mut packed = packed[..packed_len].iter();
        while let Some(&sym) = packed.next() {
            let sym = usize::from(sym);
            debug_assert!(sym < NUM_PRECODE_SYMS);
            self.bits.write_code(&self.precode, sym);
            if sym >= 16 {
                let extra = packed.next().copied().unwrap_or(0);
                self.bits.write(u32::from(extra), [2, 3, 7][sym - 16]);
            }
        }
    }

    /// Writes the block's LZ codes and its end, and says whether they fitted.
    fn write_lz_codes(&mut self) -> bool {
        let codes = &self.lz.codes[..self.lz.pos];
        let (bits, litlen, dist) = (&mut self.bits, &self.litlen, &self.dist);
        let mut i = 0;
        let mut flags = 1u32;
        while i < codes.len() {
            if flags == 1 {
                flags = u32::from(codes[i]) | 0x100;
                i += 1;
            }
            if flags & 1 != 0 {
                let len = usize::from(codes[i]);
                let match_dist = u16::from_le_bytes([codes[i + 1], codes[i + 2]]);
                i += 3;

                bits.write_code(litlen, usize::from(LEN_SYM[len]));
                let extra = u32::from(LEN_EXTRA[len]);
                bits.write(len as u32 & ((1 << extra) - 1), extra);

                let (sym, extra) = dist_symbol(match_dist);
                bits.write_code(dist, sym);
                bits.write(u32::from(match_dist) & ((1 << extra) - 1), extra);
            } else {
                bits.write_code(litlen, usize::from(codes[i]));
                i += 1;
            }
            flags >>= 1;
        }
        bits.write_code(litlen, 256);
        bits.out.len() < bits.limit
    }
}

/// The distance symbol and its extra bits for a match distance less 1.
#[inline]
fn dist_symbol(dist: u16) -> (usize, u32) {
    let dist = usize::from(dist);
    if dist < 512 {
        (
            usize::from(SMALL_DIST_SYM[dist]),
            u32::from(SMALL_DIST_EXTRA[dist]),
        )
    } else {
        (
            usize::from(LARGE_DIST_SYM[dist >> 8]),
            u32::from(LARGE_DIST_EXTRA[dist >> 8]),
        )
    }
}

/// How many bytes `a` and `b`, of the same length, have in common at their start.
#[inline]
fn common_prefix(a: &[u8], b: &[u8]) -> usize {
    const WORD: usize = 8;
    let mut len = 0;
    for (x, y) in a.as_chunks::<WORD>().0.iter().zip(b.as_chunks::<WORD>().0) {
        let (x, y) = (u64::from_le_bytes(*x), u64::from_le_bytes(*y));
        if x != y {
            return len + ((x ^ y).trailing_zeros() / 8) as usize;
        }
        len += WORD;
    }
    len + a[len..]
        .iter()
        .zip(&b[len..])
        .take_while(|(x, y)| x == y)
        .count()
}

/// The output, written a bit at a time, least significant first, with room for no more than
/// `limit` bytes: bytes past it are dropped, which the block writer detects.
#[derive(Default)]
struct BitWriter {
    out: Vec<u8>,
    limit: usize,
    buffer: u32,
    count: u32,
}

/// Where a [`BitWriter`] was, to return to.
#[derive(Clone, Copy)]
struct Saved {
    len: usize,
    buffer: u32,
    count: u32,
}

impl BitWriter {
    #[inline]
    fn write(&mut self, bits: u32, len: u32) {
        debug_assert!(u64::from(bits) < 1u64 << len);
        self.buffer |= bits << self.count;
        self.count += len;
        while self.count >= 8 {
            if self.out.len() < self.limit {
                self.out.push(self.buffer as u8);
            }
            self.buffer >>= 8;
            self.count -= 8;
        }
    }

    #[inline]
    fn write_code<const N: usize>(&mut self, table: &HuffmanTable<N>, sym: usize) {
        debug_assert!(table.lens[sym] != 0);
        self.write(u32::from(table.codes[sym]), u32::from(table.lens[sym]));
    }

    /// Pads to a whole byte with zeros.
    fn align(&mut self) {
        if self.count != 0 {
            self.write(0, 8 - self.count);
        }
    }

    /// Writes whole bytes, when the output is at a byte boundary.
    fn write_aligned(&mut self, bytes: &[u8]) {
        debug_assert_eq!(self.count, 0);
        let room = self.limit.saturating_sub(self.out.len());
        self.out.extend_from_slice(&bytes[..bytes.len().min(room)]);
    }

    fn save(&self) -> Saved {
        Saved {
            len: self.out.len(),
            buffer: self.buffer,
            count: self.count,
        }
    }

    fn restore(&mut self, saved: Saved) {
        self.out.truncate(saved.len);
        self.buffer = saved.buffer;
        self.count = saved.count;
    }
}

/// A block's LZ codes: a literal is its byte, and a match three bytes (its length less 3 and
/// its distance less 1), with a flag byte before every eight codes saying which are matches.
struct LzCodes {
    codes: Vec<u8>,
    /// Where the next code goes, where the current flag byte is, and how many codes it has
    /// room for.
    pos: usize,
    flags_pos: usize,
    flags_left: u32,
}

impl LzCodes {
    fn new() -> LzCodes {
        LzCodes {
            codes: vec![0; LZ_CODES_SIZE],
            pos: 1,
            flags_pos: 0,
            flags_left: 8,
        }
    }

    #[inline]
    fn push_literal(&mut self, literal: u8) {
        self.codes[self.pos] = literal;
        self.pos += 1;
        self.codes[self.flags_pos] >>= 1;
        self.next_flag();
    }

    #[inline]
    fn push_match(&mut self, len: u8, dist: u16) {
        self.codes[self.pos..self.pos + 3].copy_from_slice(&[len, dist as u8, (dist >> 8) as u8]);
        self.pos += 3;
        self.codes[self.flags_pos] = (self.codes[self.flags_pos] >> 1) | 0x80;
        self.next_flag();
    }

    #[inline]
    fn next_flag(&mut self) {
        self.flags_left -= 1;
        if self.flags_left == 0 {
            self.flags_left = 8;
            self.flags_pos = self.pos;
            self.pos += 1;
        }
    }

    /// Moves the last flag byte's flags to its low bits, and drops it if it has none.
    fn close(&mut self) {
        let flags = &mut self.codes[self.flags_pos];
        *flags = (u32::from(*flags) >> self.flags_left) as u8;
        self.pos -= usize::from(self.flags_left == 8);
    }

    fn clear(&mut self) {
        self.pos = 1;
        self.flags_pos = 0;
        self.flags_left = 8;
    }
}

/// The run-length encoding of code lengths in a dynamic block's header: runs of a length are
/// symbol 16 (3 to 6 repeats of the previous length), and runs of zeros 17 (3 to 10) or 18
/// (11 to 138).
struct CodeLengthRle<'a> {
    counts: &'a mut [u16; NUM_PRECODE_SYMS],
    packed: [u8; NUM_LITLEN_SYMS + NUM_DIST_SYMS],
    len: usize,
    zeros: u32,
    repeats: u32,
}

impl CodeLengthRle<'_> {
    fn push(&mut self, sym: u8) {
        self.counts[usize::from(sym)] = self.counts[usize::from(sym)].wrapping_add(1);
        self.packed[self.len] = sym;
        self.len += 1;
    }

    fn push_run(&mut self, sym: u8, extra: u32) {
        self.push(sym);
        self.packed[self.len] = extra as u8;
        self.len += 1;
    }

    /// Writes the pending repeats of `prev` (`TDEFL_RLE_PREV_CODE_SIZE`).
    fn end_repeats(&mut self, prev: u8) {
        match self.repeats {
            0 => {}
            1 | 2 => {
                for _ in 0..self.repeats {
                    self.push(prev);
                }
            }
            n => self.push_run(16, n - 3),
        }
        self.repeats = 0;
    }

    /// Writes the pending zeros (`TDEFL_RLE_ZERO_CODE_SIZE`).
    fn end_zeros(&mut self) {
        match self.zeros {
            0 => {}
            1 | 2 => {
                for _ in 0..self.zeros {
                    self.push(0);
                }
            }
            n @ 3..=10 => self.push_run(17, n - 3),
            n => self.push_run(18, n - 11),
        }
        self.zeros = 0;
    }
}

/// The longest code length the length-limiting step accepts.
const MAX_SUPPORTED_CODE_LEN: usize = 32;

/// One of a block's three Huffman codes: the symbol counts it is built from, and its codes
/// (bit-reversed, ready to write) and their lengths.
struct HuffmanTable<const N: usize> {
    counts: [u16; N],
    codes: [u16; N],
    lens: [u8; N],
}

/// A symbol and its count, or, as the code lengths are worked out, a parent or a depth.
#[derive(Clone, Copy, Default)]
struct SymbolKey {
    key: u16,
    symbol: u16,
}

impl<const N: usize> HuffmanTable<N> {
    fn new() -> Self {
        HuffmanTable {
            counts: [0; N],
            codes: [0; N],
            lens: [0; N],
        }
    }

    #[inline]
    fn count(&mut self, sym: usize) {
        self.counts[sym] = self.counts[sym].wrapping_add(1);
    }

    /// Builds length-limited codes for the counts (`tdefl_optimize_huffman_table`).
    fn build(&mut self, max_len: usize) {
        let mut syms0 = [SymbolKey::default(); N];
        let mut syms1 = [SymbolKey::default(); N];
        let mut used = 0;
        for (sym, &count) in self.counts.iter().enumerate() {
            if count != 0 {
                syms0[used] = SymbolKey {
                    key: count,
                    symbol: sym as u16,
                };
                used += 1;
            }
        }
        let syms = if radix_sort_by_key(&mut syms0[..used], &mut syms1[..used]) {
            &mut syms1[..used]
        } else {
            &mut syms0[..used]
        };
        assign_code_lengths(syms);

        let mut num_codes = [0i32; 1 + MAX_SUPPORTED_CODE_LEN];
        for sym in syms.iter() {
            num_codes[usize::from(sym.key)] += 1;
        }
        limit_code_lengths(&mut num_codes, used, max_len);

        self.lens = [0; N];
        self.codes = [0; N];
        // The least frequent symbols, at the start, get the longest codes.
        let mut next = syms.len();
        for (len, &count) in num_codes.iter().enumerate().take(max_len + 1).skip(1) {
            for _ in 0..count {
                next -= 1;
                self.lens[usize::from(syms[next].symbol)] = len as u8;
            }
        }
        self.assign_codes(&num_codes, max_len);
    }

    /// Assigns canonical codes for the lengths already in place.
    fn assign_codes_from_lens(&mut self, max_len: usize) {
        let mut num_codes = [0i32; 1 + MAX_SUPPORTED_CODE_LEN];
        for &len in &self.lens {
            num_codes[usize::from(len)] += 1;
        }
        self.assign_codes(&num_codes, max_len);
    }

    fn assign_codes(&mut self, num_codes: &[i32; 1 + MAX_SUPPORTED_CODE_LEN], max_len: usize) {
        let mut next_code = [0u32; MAX_SUPPORTED_CODE_LEN + 1];
        let mut code = 0u32;
        for len in 2..=max_len {
            code = code.wrapping_add(num_codes[len - 1] as u32) << 1;
            next_code[len] = code;
        }
        for (sym, &len) in self.lens.iter().enumerate() {
            if len != 0 {
                let len = usize::from(len);
                let code = next_code[len];
                next_code[len] += 1;
                self.codes[sym] = (code.reverse_bits() >> (32 - len)) as u16;
            }
        }
    }
}

/// Sorts symbols by key, least first, in one or two passes of a stable radix sort, and says
/// whether the result is in `b` (else `a`).
fn radix_sort_by_key(a: &mut [SymbolKey], b: &mut [SymbolKey]) -> bool {
    let n = a.len();
    let mut hist = [[0u32; 256]; 2];
    for sym in a.iter() {
        hist[0][usize::from(sym.key & 0xFF)] += 1;
        hist[1][usize::from(sym.key >> 8)] += 1;
    }
    // One pass when every key is below 256.
    let passes = if hist[1][0] == n as u32 { 1 } else { 2 };
    let mut in_b = false;
    for (pass, hist) in hist.iter().enumerate().take(passes) {
        let mut offsets = [0usize; 256];
        let mut total = 0;
        for (offset, &count) in offsets.iter_mut().zip(hist) {
            *offset = total;
            total += count as usize;
        }
        let (from, to): (&[SymbolKey], &mut [SymbolKey]) = if in_b { (b, a) } else { (a, b) };
        let shift = pass * 8;
        for sym in from {
            let slot = &mut offsets[usize::from(sym.key >> shift) & 0xFF];
            to[*slot] = *sym;
            *slot += 1;
        }
        in_b = !in_b;
    }
    in_b
}

/// Replaces each key, a count in ascending order, with the length of the symbol's optimal
/// code, in place (Moffat and Katajainen, "In-place calculation of minimum-redundancy codes",
/// 1995; `tdefl_calculate_minimum_redundancy`).
fn assign_code_lengths(a: &mut [SymbolKey]) {
    let n = a.len();
    match n {
        0 => return,
        1 => {
            a[0].key = 1;
            return;
        }
        _ => {}
    }
    // First pass: build the tree, with each internal node's weight, then its parent.
    a[0].key = a[0].key.wrapping_add(a[1].key);
    let (mut root, mut leaf) = (0, 2);
    for next in 1..n - 1 {
        if leaf >= n || a[root].key < a[leaf].key {
            a[next].key = a[root].key;
            a[root].key = next as u16;
            root += 1;
        } else {
            a[next].key = a[leaf].key;
            leaf += 1;
        }
        if leaf >= n || (root < next && a[root].key < a[leaf].key) {
            a[next].key = a[next].key.wrapping_add(a[root].key);
            a[root].key = next as u16;
            root += 1;
        } else {
            a[next].key = a[next].key.wrapping_add(a[leaf].key);
            leaf += 1;
        }
    }
    // Second pass: each internal node's depth.
    a[n - 2].key = 0;
    for next in (0..n.saturating_sub(2)).rev() {
        a[next].key = a[usize::from(a[next].key)].key.wrapping_add(1);
    }
    // Third pass: each leaf's depth.
    let (mut avail, mut used, mut depth) = (1usize, 0usize, 0u16);
    let mut root = n as isize - 2;
    let mut next = n - 1;
    while avail > 0 {
        while root >= 0 && a[root as usize].key == depth {
            used += 1;
            root -= 1;
        }
        while avail > used {
            a[next].key = depth;
            next = next.wrapping_sub(1);
            avail -= 1;
        }
        avail = 2 * used;
        depth += 1;
        used = 0;
    }
}

/// Limits the code lengths, given as the number of codes of each length, to `max_len`,
/// lengthening shorter codes to keep the code complete (`tdefl_huffman_enforce_max_code_size`).
fn limit_code_lengths(
    num_codes: &mut [i32; 1 + MAX_SUPPORTED_CODE_LEN],
    used: usize,
    max_len: usize,
) {
    if used <= 1 {
        return;
    }
    for len in max_len + 1..=MAX_SUPPORTED_CODE_LEN {
        num_codes[max_len] += num_codes[len];
    }
    let mut total = 0u32;
    for len in (1..=max_len).rev() {
        total = total.wrapping_add((num_codes[len] as u32) << (max_len - len));
    }
    while total != 1 << max_len {
        num_codes[max_len] -= 1;
        if let Some(len) = (1..max_len).rev().find(|&len| num_codes[len] != 0) {
            num_codes[len] -= 1;
            num_codes[len + 1] += 2;
        }
        total = total.wrapping_sub(1);
    }
}
