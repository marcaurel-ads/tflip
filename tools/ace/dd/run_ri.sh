#!/bin/sh
# Builds ri.exe and records how the engine enforces referential integrity, on a
# fresh copy of the dictionary_ri fixture, under Wine.
#
#     tools/ace/dd/run_ri.sh <work folder> <ace32.dll as a Windows path>  > ri.json
#
# With a third argument `gen`, it instead (re)creates the committed fixture
# fixtures/adt/dictionary_ri from scratch (the `build` pass of ri.exe):
#
#     tools/ace/dd/run_ri.sh <work folder> <ace32.dll as a Windows path> gen
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
work=$1
dll=$2
mode=$3
mkdir -p "$work"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/ri.exe" "$here/ri.c"
win() { echo "Z:$(echo "$1" | tr / '\\')"; }

if [ "$mode" = gen ]; then
    rm -rf "$work/build"
    mkdir -p "$work/build"
    cd "$work"
    WINEDEBUG=-all wine ri.exe "$dll" "$(win "$work/build/ri.add")" build
    dest=$root/fixtures/adt/dictionary_ri
    rm -rf "$dest"
    mkdir -p "$dest"
    cp "$work/build"/* "$dest/"
    # The per-table JSON dumps the other fixtures carry, with adsdump (the same
    # tool the committed fixtures are dumped with). Set ADSDUMP to its path.
    if [ -n "$ADSDUMP" ]; then
        for t in p cr cc cn cd; do
            WINEDEBUG=-all wine "$ADSDUMP" -t ansi "$t" adt "$(win "$dest/ri.add")" ADSSYS "" \
                > "$dest/$t.json"
        done
    fi
    echo "wrote $dest" >&2
    exit 0
fi

rm -rf "$work/data"
mkdir -p "$work/data/dictionary_ri"
cp "$root/fixtures/adt/dictionary_ri"/* "$work/data/dictionary_ri/"
cd "$work"
WINEDEBUG=-all wine ri.exe "$dll" "$(win "$work/data/dictionary_ri/ri.add")"
