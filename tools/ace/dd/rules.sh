#!/bin/sh
# Records how the engine orders a dictionary's constraints, triggers and
# referential integrity, reads defaults, minimums and maximums as
# expressions, and which of them a field takes, on a fresh copy of the
# dictionary_rules fixture, under Wine, with constraints.exe (see
# constraints.c), on the local server:
#
#     tools/ace/dd/rules.sh <work folder> <ace32.dll as a Windows path>  > rules.json
#
# With a third argument `gen`, it instead (re)creates the committed fixture
# fixtures/adt/dictionary_rules from rules.sql:
#
#     tools/ace/dd/rules.sh <work folder> <ace32.dll as a Windows path> gen
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
work=$1
dll=$2
mode=$3
mkdir -p "$work"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/constraints.exe" "$here/constraints.c"
win() { echo "Z:$(echo "$1" | tr / '\\')"; }

if [ "$mode" = gen ]; then
    rm -rf "$work/build"
    mkdir -p "$work/build"
    cd "$work"
    WINEDEBUG=-all wine constraints.exe "$dll" "$(win "$work/build/rules.add")" build \
        < "$here/rules.sql"
    dest=$root/fixtures/adt/dictionary_rules
    rm -rf "$dest"
    mkdir -p "$dest"
    cp "$work/build"/* "$dest/"
    echo "wrote $dest" >&2
    exit 0
fi

rm -rf "$work/data"
mkdir -p "$work/data/dictionary_rules"
cp "$root/fixtures/adt/dictionary_rules"/* "$work/data/dictionary_rules/"
cd "$work"
WINEDEBUG=-all wine constraints.exe "$dll" "$(win "$work/data/dictionary_rules/rules.add")" \
    < "$here/rules.ops"
