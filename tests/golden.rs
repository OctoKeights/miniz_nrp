//! `miniz_nrp` against hashes recorded from miniz, so that any change in output shows without a
//! C compiler.
//!
//! `tests/data/miniz_expected.txt` is what `compare/dump_miniz.c`, built against miniz v3.1.0,
//! prints for `miniz 3 120` and `miniz 8 120`: hashes of synthetic data compressed with the
//! options of every level and strategy and with random options, in one call and in pieces, and
//! of what each stream decompresses to, intact and damaged, or the error it gives.
//! `compare/compare.sh` runs the full comparison.

mod made_up;

#[test]
fn compresses_and_decompresses_as_miniz_does() {
    let printed = [made_up::miniz(3, 120), made_up::miniz(8, 120)].concat();
    let expected = include_str!("data/miniz_expected.txt");
    for (line, (ours, theirs)) in printed.lines().zip(expected.lines()).enumerate() {
        assert_eq!(ours, theirs, "line {}", line + 1);
    }
    assert_eq!(printed.lines().count(), expected.lines().count());
}
