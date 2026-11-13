#!/bin/sh
# Runs collation.txt (SQL UPPER/LOWER over the charset_german `ca` table)
# under the default static collation and under ANSI_CHAR_SET=German, to see
# whether the SQL engine folds case by the collation. Writes
# collation_default.json and collation_german.json in the work folder, and
# the committed golden fixtures/sql/collation_case.json is distilled from the
# German run (byte, UPPER byte, LOWER byte per record).
#
#     tools/ace/sql/run_collation.sh <work folder> <acesdk folder, a Unix path>
#
# The second argument holds ace32.dll and its runtime files. Unlike the
# expression probe, the SQL engine deadlocks Wine's loader when ace32.dll is
# loaded from a folder of symlinks or a copy (it needs its own install dir),
# so this runs against the acesdk folder itself and swaps ANSI_CHAR_SET into
# its adslocal.cfg for the German run, restoring it afterwards.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
work=$1
acesdk=$2
mkdir -p "$work"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/sql.exe" "$here/sql.c"
iconv -f utf-8 -t cp1252 "$here/collation.txt" > "$work/collation.txt"

# A data folder with only the `ca` table (its index was built under German;
# UPPER/LOWER need no index, so leave it out to open it collation-free).
rm -rf "$work/data"
mkdir "$work/data"
cp "$root"/fixtures/adt/charset_german/ca.adt "$work/data/"

dll="Z:$(echo "$acesdk/ace32.dll" | tr / '\\')"
data="Z:$(echo "$work/data" | tr / '\\')"
cp "$acesdk/adslocal.cfg" "$work/adslocal.cfg.bak"
restore() { cp "$work/adslocal.cfg.bak" "$acesdk/adslocal.cfg"; }
trap restore EXIT

run() {
    name=$1
    ansi=$2
    sed "s/^ANSI_CHAR_SET=.*/ANSI_CHAR_SET=$ansi/" "$work/adslocal.cfg.bak" > "$acesdk/adslocal.cfg"
    wineserver -k 2>/dev/null || true
    sleep 1
    WINEDEBUG=-all wine "$work/sql.exe" "$dll" "$data" "$work/collation.txt" \
        > "$work/collation_$name.json"
}

run default ''      # empty = the default (identity) collation
run german German
echo "wrote $work/collation_default.json and $work/collation_german.json"
