//! Compression and decompression throughput on synthetic data.
//!
//! ```sh
//! cargo run --release --example bench
//! ```
//!
//! Each figure is the best of several runs, in megabytes of uncompressed data per second.

use miniz_nrp::{Level, compress, decompress};
use std::time::Instant;

const RUNS: usize = 7;

/// Four megabytes resembling text: words from a small vocabulary, with punctuation.
fn text_like(len: usize) -> Vec<u8> {
    const WORDS: [&str; 16] = [
        "the",
        "quick",
        "brown",
        "fox",
        "jumps",
        "over",
        "lazy",
        "dog",
        "texture",
        "block",
        "compression",
        "stream",
        "of",
        "and",
        "a",
        "level",
    ];
    let mut state = 1u64;
    let mut data = Vec::with_capacity(len + 16);
    while data.len() < len {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        data.extend_from_slice(WORDS[(state >> 60) as usize].as_bytes());
        data.push(if (state >> 58).trailing_zeros() >= 2 {
            b'.'
        } else {
            b' '
        });
    }
    data.truncate(len);
    data
}

/// Four megabytes resembling binary data: runs, repeats and noise.
fn binary_like(len: usize) -> Vec<u8> {
    let mut state = 2u64;
    let mut data = Vec::with_capacity(len);
    while data.len() < len {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let run = 1 + (state >> 59) as usize;
        match state >> 56 & 3 {
            0 => data.extend(std::iter::repeat_n((state >> 40) as u8, run * 4)),
            1 if data.len() > 300 => {
                let from = data.len() - 1 - (state >> 33) as usize % 300;
                for k in 0..run * 3 {
                    data.push(data[from + k % (data.len() - from)]);
                }
            }
            _ => data.extend((0..run).map(|k| (state >> (k % 7 * 8)) as u8)),
        }
    }
    data.truncate(len);
    data
}

fn main() {
    let len = 4 << 20;
    for (name, data) in [
        ("text-like", text_like(len)),
        ("binary-like", binary_like(len)),
    ] {
        for number in [0, 1, 3, 6, 9] {
            let level = Level::new(number).unwrap();
            let (mut compress_time, mut decompress_time) = (f64::MAX, f64::MAX);
            let mut stream = Vec::new();
            for _ in 0..RUNS {
                let start = Instant::now();
                stream = compress(&data, level);
                compress_time = compress_time.min(start.elapsed().as_secs_f64());
                let start = Instant::now();
                let out = decompress(&stream, data.len()).unwrap();
                decompress_time = decompress_time.min(start.elapsed().as_secs_f64());
                assert_eq!(out, data);
            }
            let megabytes = data.len() as f64 / 1e6;
            println!(
                "{name:12} level {} ratio {:5.1}%   compress {:7.1} MB/s   decompress {:7.1} MB/s",
                number,
                100.0 * stream.len() as f64 / data.len() as f64,
                megabytes / compress_time,
                megabytes / decompress_time
            );
        }
    }
}
