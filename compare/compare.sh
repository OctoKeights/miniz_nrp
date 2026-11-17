#!/bin/sh
# Compares miniz_nrp with miniz (https://github.com/richgel999/miniz).
#
#   compare/compare.sh <checkout of miniz> [count] [seeds] [dumps]
#
# compare/dump_miniz.c is built against miniz's compressor and decompressor as they build by
# default, and examples/dump.rs from the port; both run the same made-up data for each seed and
# what they print is compared byte for byte. Dump: miniz, with a hundredth of [count] (200,000
# by default) as the number of made-up files, each some tens of thousands of bytes, compressed
# three ways and decompressed seven.
set -u
if [ $# -lt 1 ]; then
    echo "usage: $0 <checkout of miniz> [count] [seeds] [dumps]" >&2
    exit 2
fi
here=$(cd "$(dirname "$0")/.." && pwd)
source=$1
blocks=${2:-200000}
seeds=${3:-3}
dumps=${4:-miniz}
tmp=${TMPDIR:-/tmp}/miniz_nrp-compare
mkdir -p "$tmp" || exit 2
if ! [ -f "$source/miniz_tdef.c" ]; then
    echo "$source does not look like checkout of miniz" >&2
    exit 2
fi

(cd "$here" && cargo build --release -q --ignore-rust-version --example dump) || exit 2
rust=$(cd "$here" && cargo metadata --format-version 1 --no-deps 2>/dev/null | sed 's/.*"target_directory":"\([^"]*\)".*/\1/')/release/examples/dump

# (The header its build would make, and no .zip code, which is not ported.)
mkdir -p "$tmp/miniz" && echo "#define MINIZ_EXPORT" > "$tmp/miniz/miniz_export.h" || exit 2
${CC:-cc} -O2 -DMINIZ_NO_ARCHIVE_APIS -I "$tmp/miniz" -I "$source" "$here/compare/dump_miniz.c" "$source/miniz.c" "$source/miniz_tdef.c" "$source/miniz_tinfl.c" -o "$tmp/dump-miniz" || exit 2

bad=0
for what in $dumps; do
    extra=
    case $what in
    miniz)
        reference=$tmp/dump-miniz
        count=$((blocks / 100))
        ;;
    *)
        echo "unknown dump $what" >&2
        exit 2
        ;;
    esac
    seed=1
    while [ "$seed" -le "$seeds" ]; do
        # shellcheck disable=SC2086
        "$reference" "$what" "$seed" "$count" $extra > "$tmp/reference.txt" 2> /dev/null
        # shellcheck disable=SC2086
        "$rust" "$what" "$seed" "$count" $extra > "$tmp/rust.txt"
        if [ ! -s "$tmp/reference.txt" ]; then
            echo "$what, seed $seed: NOT COMPARED, the original's side printed nothing"
            bad=$((bad + 1))
        elif cmp -s "$tmp/reference.txt" "$tmp/rust.txt"; then
            echo "$what, seed $seed: the same ($(tail -1 "$tmp/rust.txt"))"
        else
            echo "$what, seed $seed: DIFFERENT"
            bad=$((bad + 1))
        fi
        seed=$((seed + 1))
    done
done
[ "$bad" -eq 0 ]
