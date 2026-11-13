#!/bin/sh
# Runs collation_dynamic.txt (SQL comparisons over tables opened with a
# dynamic collation) under the default static collation, writing the
# committed fixtures/sql/collation_dynamic.json to standard output.
#
#     tools/ace/sql/run_collation_dynamic.sh <work folder> <acesdk folder, a Unix path> [script]
#
# The data folder holds the charset_german `ca` table without an index and
# the collation_dynamic `ger` (GERMAN_VFP_CI_AS_1252) and `gen`
# (GENERAL_VFP_CI_AS_1252) tables with theirs. The acesdk folder is only
# read; its adslocal.cfg must leave ANSI_CHAR_SET empty.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
work=$1
acesdk=$2
script=${3:-$here/collation_dynamic.txt}
mkdir -p "$work"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/sql.exe" "$here/sql.c"
iconv -f utf-8 -t cp1252 "$script" > "$work/script.txt"
rm -rf "$work/data"
mkdir "$work/data"
cp "$root"/fixtures/adt/charset_german/ca.adt "$work/data/"
cp "$root"/fixtures/adt/collation_dynamic/ger.ad[ti] "$root"/fixtures/adt/collation_dynamic/gen.ad[ti] \
    "$work/data/"
dll="Z:$(echo "$acesdk/ace32.dll" | tr / '\\')"
data="Z:$(echo "$work/data" | tr / '\\')"
WINEDEBUG=-all wine "$work/sql.exe" "$dll" "$data" "$work/script.txt"
