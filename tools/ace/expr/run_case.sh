#!/bin/sh
# Evaluates UPPER/LOWER on the charset_german `ca` fixture under the default
# static collation and under ANSI_CHAR_SET=German, to see whether the collation
# changes how expressions fold case.
#
#     tools/ace/expr/run_case.sh <work folder> <acesdk folder, a Unix path>
#
# The second argument is the folder that holds ace32.dll and its runtime files
# (ansi.chr, extend.chr, adslocal.cfg, ...); the local server reads adslocal.cfg
# from the DLL's folder, so each run uses an "engine" folder of symlinks to
# those files with an adslocal.cfg of its own. Writes case_default.json and
# case_german.json in the work folder.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
work=$1
acesdk=$2
mkdir -p "$work"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/probe.exe" "$here/probe.c"

# A data folder with only the table (its index was built under German; opening
# it under the default collation fails, and expression evaluation needs no index).
rm -rf "$work/data"
mkdir "$work/data"
cp "$root"/fixtures/adt/charset_german/ca.adt "$work/data/"
win_data="Z:$(echo "$work/data" | tr / '\\')"

run() {
    name=$1
    ansi=$2
    engine=$work/engine-$name
    rm -rf "$engine"
    mkdir "$engine"
    for f in "$acesdk"/*; do
        [ "$(basename "$f")" = adslocal.cfg ] || ln -sf "$f" "$engine/"
    done
    printf '[settings]\r\nANSI_CHAR_SET=%s\r\nOEM_CHAR_SET=USA\r\n' "$ansi" > "$engine/adslocal.cfg"
    win_dll="Z:$(echo "$engine/ace32.dll" | tr / '\\')"
    WINEDEBUG=-all wine "$work/probe.exe" "$win_dll" "$win_data" "$here/case.txt" \
        > "$work/case_$name.json"
}

run default ""
run german German
echo "wrote $work/case_default.json and $work/case_german.json" >&2
