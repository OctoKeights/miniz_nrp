//! The decompressor (`miniz_tinfl.c`).
//!
//! miniz's `tinfl_decompress` can stop wherever its input or its room for output runs out and
//! resume when called again. With all of its input and all the room at once, which is how it
//! is used here, it never resumes: where it would stop to ask for more input, the stream is
//! truncated, and where it would stop for more room, the stream is too large. Its steps are
//! otherwise kept in order, so it reads exactly the bytes miniz reads and accepts and rejects
//! exactly the streams miniz does. It behaves as miniz does on a 64-bit processor (with a
//! 64-bit bit buffer), on every machine.

use crate::{DecompressError, Decompressed, adler32};

const NUM_LITLEN_SYMS: usize = 288;
const NUM_DIST_SYMS: usize = 32;
const NUM_PRECODE_SYMS: usize = 19;

/// Codes of up to this many bits are decoded with one look-up; longer ones continue in a
/// tree.
const FAST_BITS: u32 = 10;
const FAST_SIZE: usize = 1 << FAST_BITS;

const LENGTH_BASE: [u16; 31] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258, 0, 0,
];
const LENGTH_EXTRA: [u8; 31] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0, 0, 0,
];
const DIST_BASE: [u16; 32] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577, 0, 0,
];
const DIST_EXTRA: [u8; 32] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13, 0, 0,
];
/// The order in which a dynamic block's header lists the code lengths of its precode.
const PRECODE_ORDER: [u8; NUM_PRECODE_SYMS] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// Decompresses `input`, a zlib stream if `zlib` and raw DEFLATE otherwise, to no more than
/// `max_len` bytes.
pub(crate) fn inflate(
    input: &[u8],
    zlib: bool,
    max_len: usize,
) -> Result<Decompressed, DecompressError> {
    let mut inflater = Inflater {
        bits: BitReader {
            input,
            pos: 0,
            buffer: 0,
            count: 0,
        },
        // (Room is taken as it is needed, so that a small stream that claims to be large
        // costs nothing.)
        out: Vec::new(),
        max_len,
        litlen: Table::new(),
        dist: Table::new(),
        precode: Table::new(),
        litlen_lens: [0; NUM_LITLEN_SYMS],
        dist_lens: [0; NUM_DIST_SYMS],
        precode_lens: [0; NUM_PRECODE_SYMS],
    };
    let checksum = inflater.run(zlib)?;
    if zlib && adler32(&inflater.out) != checksum {
        return Err(DecompressError::ChecksumMismatch);
    }
    Ok(Decompressed {
        data: inflater.out,
        bytes_read: inflater.bits.pos,
    })
}

/// The input, read a bit at a time, least significant first.
struct BitReader<'a> {
    input: &'a [u8],
    pos: usize,
    buffer: u64,
    count: u32,
}

impl BitReader<'_> {
    #[inline]
    fn read_byte(&mut self) -> Result<u32, DecompressError> {
        let byte = *self.input.get(self.pos).ok_or(DecompressError::Truncated)?;
        self.pos += 1;
        Ok(u32::from(byte))
    }

    /// Reads bytes into the buffer until it holds at least `n` bits (`TINFL_NEED_BITS`).
    #[inline]
    fn fill(&mut self, n: u32) -> Result<(), DecompressError> {
        loop {
            let byte = self.read_byte()?;
            self.buffer |= u64::from(byte) << self.count;
            self.count += 8;
            if self.count >= n {
                return Ok(());
            }
        }
    }

    #[inline]
    fn skip(&mut self, n: u32) -> Result<(), DecompressError> {
        if self.count < n {
            self.fill(n)?;
        }
        self.buffer >>= n;
        self.count -= n;
        Ok(())
    }

    #[inline]
    fn read(&mut self, n: u32) -> Result<u32, DecompressError> {
        if self.count < n {
            self.fill(n)?;
        }
        let bits = (self.buffer & ((1 << n) - 1)) as u32;
        self.buffer >>= n;
        self.count -= n;
        Ok(bits)
    }

    /// Reads a byte, from the buffer if it has any bits left, else from the input.
    #[inline]
    fn read_aligned_byte(&mut self) -> Result<u32, DecompressError> {
        if self.count != 0 {
            self.read(8)
        } else {
            self.read_byte()
        }
    }

    /// Decodes the next symbol of `table` (`TINFL_HUFF_DECODE`).
    ///
    /// With fewer than two bytes of input left, it reads only as many bytes as the code
    /// needs, so that it never reads past the end of the stream.
    #[inline]
    fn decode(&mut self, table: &Table) -> Result<u32, DecompressError> {
        if self.count < 15 {
            if self.input.len() - self.pos < 2 {
                loop {
                    let entry = table.fast_entry(self.buffer);
                    if entry >= 0 {
                        let len = (entry >> 9) as u32;
                        if len != 0 && self.count >= len {
                            break;
                        }
                    } else if self.count > FAST_BITS {
                        let mut len = FAST_BITS;
                        let mut entry = entry;
                        loop {
                            entry = table.child(entry, self.buffer, len);
                            len += 1;
                            if entry >= 0 || self.count < len + 1 {
                                break;
                            }
                        }
                        if entry >= 0 {
                            break;
                        }
                    }
                    let byte = self.read_byte()?;
                    self.buffer |= u64::from(byte) << self.count;
                    self.count += 8;
                    if self.count >= 15 {
                        break;
                    }
                }
            } else {
                let two = u16::from_le_bytes([self.input[self.pos], self.input[self.pos + 1]]);
                self.buffer |= u64::from(two) << self.count;
                self.pos += 2;
                self.count += 16;
            }
        }
        let (sym, len) = table.lookup(self.buffer);
        self.buffer >>= len;
        self.count = self.count.wrapping_sub(len);
        Ok(sym)
    }
}

/// The decoding tables of one of a block's three codes: entries for every 10-bit prefix,
/// each a symbol and its length, or for a longer code, the root of a binary tree in `tree`.
/// (Tree nodes are negative, `!index`; leaves are symbols.)
struct Table {
    fast: [i16; FAST_SIZE],
    tree: [i16; NUM_LITLEN_SYMS * 2],
}

impl Table {
    fn new() -> Table {
        Table {
            fast: [0; FAST_SIZE],
            tree: [0; NUM_LITLEN_SYMS * 2],
        }
    }

    #[inline]
    fn fast_entry(&self, buffer: u64) -> i32 {
        i32::from(self.fast[(buffer & (FAST_SIZE as u64 - 1)) as usize])
    }

    /// The child of tree node `node` that bit `bit` of `buffer` selects.
    #[inline]
    fn child(&self, node: i32, buffer: u64, bit: u32) -> i32 {
        i32::from(self.tree[(!node + ((buffer >> bit) & 1) as i32) as usize])
    }

    /// The symbol whose code starts `buffer`, and the code's length.
    #[inline]
    fn lookup(&self, buffer: u64) -> (u32, u32) {
        let entry = self.fast_entry(buffer);
        if entry >= 0 {
            ((entry & 511) as u32, (entry >> 9) as u32)
        } else {
            let mut len = FAST_BITS;
            let mut entry = entry;
            loop {
                entry = self.child(entry, buffer, len);
                len += 1;
                if entry >= 0 {
                    return (entry as u32, len);
                }
            }
        }
    }

    /// Builds the tables for canonical codes of the lengths `lens`; an incomplete or
    /// oversubscribed set of lengths is invalid, unless it has only one code.
    fn build(&mut self, lens: &[u8]) -> Result<(), DecompressError> {
        let mut num_codes = [0u32; 16];
        for &len in lens {
            num_codes[usize::from(len)] += 1;
        }
        let mut next_code = [0u32; 17];
        let (mut used, mut total) = (0, 0);
        for len in 1..=15 {
            used += num_codes[len];
            total = (total + num_codes[len]) << 1;
            next_code[len + 1] = total;
        }
        if total != 65536 && used > 1 {
            return Err(DecompressError::Invalid);
        }
        self.fast = [0; FAST_SIZE];
        self.tree = [0; NUM_LITLEN_SYMS * 2];
        let mut tree_next: i32 = -1;
        for (sym, &len) in lens.iter().enumerate() {
            if len == 0 {
                continue;
            }
            let len = u32::from(len);
            let code = next_code[len as usize];
            next_code[len as usize] += 1;
            let mut rev_code = code.reverse_bits() >> (32 - len);
            if len <= FAST_BITS {
                let entry = ((len << 9) | sym as u32) as i16;
                while (rev_code as usize) < FAST_SIZE {
                    self.fast[rev_code as usize] = entry;
                    rev_code += 1 << len;
                }
                continue;
            }
            let slot = rev_code as usize & (FAST_SIZE - 1);
            let mut node = i32::from(self.fast[slot]);
            if node == 0 {
                self.fast[slot] = tree_next as i16;
                node = tree_next;
                tree_next -= 2;
            }
            rev_code >>= FAST_BITS - 1;
            for _ in FAST_BITS + 1..len {
                rev_code >>= 1;
                node -= (rev_code & 1) as i32;
                let at = (-node - 1) as usize;
                if self.tree[at] == 0 {
                    self.tree[at] = tree_next as i16;
                    node = tree_next;
                    tree_next -= 2;
                } else {
                    node = i32::from(self.tree[at]);
                }
            }
            rev_code >>= 1;
            node -= (rev_code & 1) as i32;
            self.tree[(-node - 1) as usize] = sym as i16;
        }
        Ok(())
    }
}

struct Inflater<'a> {
    bits: BitReader<'a>,
    out: Vec<u8>,
    max_len: usize,
    litlen: Table,
    dist: Table,
    precode: Table,
    litlen_lens: [u8; NUM_LITLEN_SYMS],
    dist_lens: [u8; NUM_DIST_SYMS],
    precode_lens: [u8; NUM_PRECODE_SYMS],
}

impl Inflater<'_> {
    #[inline]
    fn push(&mut self, byte: u8) -> Result<(), DecompressError> {
        if self.out.len() >= self.max_len {
            return Err(DecompressError::OutputTooLarge);
        }
        self.out.push(byte);
        Ok(())
    }

    /// Decompresses the whole stream, and gives back the checksum it ends with (1 when it is
    /// raw DEFLATE).
    fn run(&mut self, zlib: bool) -> Result<u32, DecompressError> {
        if zlib {
            let cmf = self.bits.read_byte()?;
            let flg = self.bits.read_byte()?;
            if (cmf * 256 + flg) % 31 != 0 || flg & 32 != 0 || cmf & 15 != 8 {
                return Err(DecompressError::Invalid);
            }
        }
        loop {
            let header = self.bits.read(3)?;
            match header >> 1 {
                0 => self.stored_block()?,
                1 => {
                    self.litlen_lens[..144].fill(8);
                    self.litlen_lens[144..256].fill(9);
                    self.litlen_lens[256..280].fill(7);
                    self.litlen_lens[280..].fill(8);
                    self.dist_lens = [5; NUM_DIST_SYMS];
                    self.dist.build(&self.dist_lens)?;
                    self.litlen.build(&self.litlen_lens)?;
                    self.huffman_block()?;
                }
                2 => {
                    let (num_litlen, num_dist) = self.dynamic_header()?;
                    self.dist.build(&self.dist_lens[..num_dist])?;
                    self.litlen.build(&self.litlen_lens[..num_litlen])?;
                    self.huffman_block()?;
                }
                _ => return Err(DecompressError::Invalid),
            }
            if header & 1 != 0 {
                break;
            }
        }

        // Return to a byte boundary, and give back any whole bytes read ahead of the end of
        // the stream.
        self.bits.skip(self.bits.count & 7)?;
        while self.bits.pos > 0 && self.bits.count >= 8 {
            self.bits.pos -= 1;
            self.bits.count -= 8;
        }
        self.bits.buffer &= !(!0u64).checked_shl(self.bits.count).unwrap_or(0);

        let mut checksum = 1;
        if zlib {
            for _ in 0..4 {
                checksum = (checksum << 8) | self.bits.read_aligned_byte()?;
            }
        }
        Ok(checksum)
    }

    fn stored_block(&mut self) -> Result<(), DecompressError> {
        self.bits.skip(self.bits.count & 7)?;
        let mut header = [0u32; 4];
        for byte in &mut header {
            *byte = self.bits.read_aligned_byte()?;
        }
        let mut left = header[0] | header[1] << 8;
        if left != 0xFFFF ^ (header[2] | header[3] << 8) {
            return Err(DecompressError::Invalid);
        }
        while left != 0 && self.bits.count != 0 {
            let byte = self.bits.read(8)?;
            self.push(byte as u8)?;
            left -= 1;
        }
        while left != 0 {
            if self.out.len() >= self.max_len {
                return Err(DecompressError::OutputTooLarge);
            }
            let input = &self.bits.input[self.bits.pos..];
            if input.is_empty() {
                return Err(DecompressError::Truncated);
            }
            let n = (self.max_len - self.out.len())
                .min(input.len())
                .min(left as usize);
            self.out.extend_from_slice(&input[..n]);
            self.bits.pos += n;
            left -= n as u32;
        }
        Ok(())
    }

    /// Reads a dynamic block's code lengths into `litlen_lens` and `dist_lens`, and gives
    /// back how many of each there are.
    fn dynamic_header(&mut self) -> Result<(usize, usize), DecompressError> {
        let num_litlen = self.bits.read(5)? as usize + 257;
        let num_dist = self.bits.read(5)? as usize + 1;
        let num_precode = self.bits.read(4)? as usize + 4;
        self.precode_lens = [0; NUM_PRECODE_SYMS];
        for &sym in &PRECODE_ORDER[..num_precode] {
            self.precode_lens[usize::from(sym)] = self.bits.read(3)? as u8;
        }
        self.precode.build(&self.precode_lens)?;

        // Both tables' lengths, one run after another: room for a run of 138 past the end.
        let mut lens = [0u8; NUM_LITLEN_SYMS + NUM_DIST_SYMS + 137];
        let total = num_litlen + num_dist;
        let mut n = 0;
        while n < total {
            let sym = self.bits.decode(&self.precode)?;
            if sym < 16 {
                lens[n] = sym as u8;
                n += 1;
                continue;
            }
            if sym == 16 && n == 0 {
                return Err(DecompressError::Invalid);
            }
            let k = sym as usize - 16;
            let run = self.bits.read([2, 3, 7][k])? as usize + [3, 3, 11][k];
            let len = if sym == 16 { lens[n - 1] } else { 0 };
            lens[n..n + run].fill(len);
            n += run;
        }
        if n != total {
            return Err(DecompressError::Invalid);
        }
        self.litlen_lens[..num_litlen].copy_from_slice(&lens[..num_litlen]);
        self.dist_lens[..num_dist].copy_from_slice(&lens[num_litlen..total]);
        Ok((num_litlen, num_dist))
    }

    /// Decodes a block's codes, up to its end code.
    fn huffman_block(&mut self) -> Result<(), DecompressError> {
        loop {
            let sym = self.literals()?;
            if sym == 256 {
                return Ok(());
            }

            let k = sym as usize - 257;
            let mut len = u32::from(LENGTH_BASE[k]);
            let extra = u32::from(LENGTH_EXTRA[k]);
            if extra != 0 {
                len += self.bits.read(extra)?;
            }
            let k = self.bits.decode(&self.dist)? as usize;
            let mut dist = u32::from(DIST_BASE[k]);
            let extra = u32::from(DIST_EXTRA[k]);
            if extra != 0 {
                dist += self.bits.read(extra)?;
            }

            let (dist, len) = (dist as usize, len as usize);
            let end = self.out.len();
            if dist == 0 || dist > end {
                return Err(DecompressError::Invalid);
            }
            if end + len > self.max_len {
                return Err(DecompressError::OutputTooLarge);
            }
            let from = end - dist;
            if dist >= len {
                self.out.extend_from_within(from..from + len);
            } else if dist == 1 {
                let byte = self.out[from];
                self.out.resize(end + len, byte);
            } else {
                // The copy overlaps what it writes.
                self.out.reserve(len);
                for i in from..from + len {
                    let byte = self.out[i];
                    self.out.push(byte);
                }
            }
        }
    }

    /// Decodes and writes literals up to the next length or end code, and gives that back.
    fn literals(&mut self) -> Result<u32, DecompressError> {
        loop {
            let bits = &mut self.bits;
            if bits.input.len() - bits.pos < 4 || self.max_len - self.out.len() < 2 {
                // Near the end of the input or the room: one symbol at a time, carefully.
                let sym = bits.decode(&self.litlen)?;
                if sym >= 256 {
                    return Ok(sym & 511);
                }
                self.push(sym as u8)?;
            } else {
                // Up to two symbols from one refill of the buffer.
                if bits.count < 30 {
                    let at = bits.pos;
                    let word = u32::from_le_bytes(bits.input[at..at + 4].try_into().unwrap());
                    bits.buffer |= u64::from(word) << bits.count;
                    bits.pos += 4;
                    bits.count += 32;
                }
                let (first, len) = self.litlen.lookup(bits.buffer);
                bits.buffer >>= len;
                bits.count = bits.count.wrapping_sub(len);
                if len == 0 {
                    return Err(DecompressError::Invalid);
                }
                if first & 256 != 0 {
                    return Ok(first & 511);
                }
                let (second, len) = self.litlen.lookup(bits.buffer);
                bits.buffer >>= len;
                bits.count = bits.count.wrapping_sub(len);
                if len == 0 {
                    return Err(DecompressError::Invalid);
                }
                self.out.push(first as u8);
                if second & 256 != 0 {
                    return Ok(second & 511);
                }
                self.out.push(second as u8);
            }
        }
    }
}
