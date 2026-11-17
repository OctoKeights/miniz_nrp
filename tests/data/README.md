# Recorded results

`miniz_expected.txt` holds the hashes `compare/dump_miniz.c` prints when built against miniz
v3.1.0, for synthetic data. `tests/golden.rs` checks that miniz_nrp prints the same.

To record them again, run the comparison once, which builds miniz's side, then run that
program directly:

    compare/compare.sh <checkout of miniz> 3000 1
    ref="${TMPDIR:-/tmp}/miniz_nrp-compare/dump-miniz"
    { "$ref" miniz 3 120; "$ref" miniz 8 120; } > tests/data/miniz_expected.txt

All test data is generated from seeds; no external files are used.
