#!/bin/sh
# Runs collation_compare.txt (SQL comparisons over the charset_german `ca`
# table) under the default static collation and under ANSI_CHAR_SET=German,
# to see whether the SQL engine compares text by the collation's weights.
# Writes compare_default.json and compare_german.json in the work folder;
# the German run is the committed fixtures/sql/collation_compare.json.
#
#     tools/ace/sql/run_collation_compare.sh <work folder> <acesdk folder, a Unix path>
#
# Like run_collation.sh, this runs against the acesdk folder itself and swaps
# ANSI_CHAR_SET into its adslocal.cfg, restoring it afterwards. The data
# folder holds ca.adt without its index (built under German, it would be
# refused under the default collation with 5175).
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
work=$1
acesdk=$2
mkdir -p "$work"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/sql.exe" "$here/sql.c"
iconv -f utf-8 -t cp1252 "$here/collation_compare.txt" > "$work/collation_compare.txt"

dll="Z:$(echo "$acesdk/ace32.dll" | tr / '\\')"
cp "$acesdk/adslocal.cfg" "$work/adslocal.cfg.bak"
restore() { cp "$work/adslocal.cfg.bak" "$acesdk/adslocal.cfg"; }
trap restore EXIT

run() {
    name=$1
    ansi=$2
    rm -rf "$work/data"
    mkdir "$work/data"
    cp "$root"/fixtures/adt/charset_german/ca.adt "$work/data/"
    data="Z:$(echo "$work/data" | tr / '\\')"
    sed "s/^ANSI_CHAR_SET=.*/ANSI_CHAR_SET=$ansi/" "$work/adslocal.cfg.bak" > "$acesdk/adslocal.cfg"
    wineserver -k 2>/dev/null || true
    sleep 1
    WINEDEBUG=-all wine "$work/sql.exe" "$dll" "$data" "$work/collation_compare.txt" \
        > "$work/compare_$name.json"
}

run default ''      # empty = the default (identity) collation
run german German
echo "wrote $work/compare_default.json and $work/compare_german.json"
