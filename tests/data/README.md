# Recorded results

`miniz_expected.txt` is what miniz makes of made-up data, as `compare/dump_miniz.c` prints it;
`tests/golden.rs` checks that the port prints the same. To make it again, run the comparison
once (it builds the original's side), then that by itself:

    compare/compare.sh <checkout of miniz> 3000 1
    ref="${TMPDIR:-/tmp}/miniz_nrp-compare/dump-miniz"
    { "$ref" miniz 3 120; "$ref" miniz 8 120; } > tests/data/miniz_expected.txt

No picture or file from anywhere is in this repository: every file and stream the tests and
the comparison use is made up from a seed.
