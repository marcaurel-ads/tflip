#!/bin/sh
# Builds keytype.exe and records the key type, length and seek of expression
# orders, on a fresh copy of the expr_keys fixture, under Wine.
#
#     tools/ace/expr/run_keytype.sh <work folder> <ace32.dll as a Windows path>  > keytype.json
#
# With a third argument `gen`, it instead (re)creates the committed fixture
# fixtures/adt/expr_keys from scratch (the `build` pass). Set ADSDUMP to the
# adsdump tool's path to also write the per-table JSON dumps the other fixtures
# carry.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
work=$1
dll=$2
mode=$3
mkdir -p "$work"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/keytype.exe" "$here/keytype.c"
win() { echo "Z:$(echo "$1" | tr / '\\')"; }

if [ "$mode" = gen ]; then
    rm -rf "$work/build"
    mkdir -p "$work/build"
    cd "$work"
    WINEDEBUG=-all wine keytype.exe "$dll" "$(win "$work/build")" build
    dest=$root/fixtures/adt/expr_keys
    rm -rf "$dest"
    mkdir -p "$dest"
    cp "$work/build"/*.adt "$work/build"/*.adi "$dest/"
    if [ -n "$ADSDUMP" ]; then
        WINEDEBUG=-all wine "$ADSDUMP" -t ansi "$(win "$dest/keys.adt")" adt > "$dest/keys.json"
    fi
    echo "wrote $dest" >&2
    exit 0
fi

rm -rf "$work/data"
mkdir -p "$work/data/expr_keys"
cp "$root/fixtures/adt/expr_keys"/*.adt "$root/fixtures/adt/expr_keys"/*.adi "$work/data/expr_keys/"
cd "$work"
WINEDEBUG=-all wine keytype.exe "$dll" "$(win "$work/data/expr_keys")"
