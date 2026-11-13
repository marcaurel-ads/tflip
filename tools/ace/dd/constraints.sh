#!/bin/sh
# Builds constraints.exe and records how the engine applies a dictionary's
# field defaults, field constraints, record validation and triggers on write,
# on a fresh copy of the dictionary_constraints fixture, under Wine.
#
#     tools/ace/dd/constraints.sh <work folder> <ace32.dll as a Windows path>  > constraints.json
#
# With a third argument `remote` and a fourth, the server's path to the work
# folder's data/ (the work folder in a folder the server shares), it runs on
# the remote server instead (SQL_SERVER_TYPE=2, see constraints.c); the
# committed constraints.json was recorded that way, constraints_local.json on
# the local server:
#
#     tools/ace/dd/constraints.sh <work> <dll> remote '\\127.0.0.1:6262\data\<work>\data'
#
# With a third argument `gen`, it instead (re)creates the committed fixture
# fixtures/adt/dictionary_constraints from constraints.sql:
#
#     tools/ace/dd/constraints.sh <work folder> <ace32.dll as a Windows path> gen
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
    WINEDEBUG=-all wine constraints.exe "$dll" "$(win "$work/build/constraints.add")" build \
        < "$here/constraints.sql"
    dest=$root/fixtures/adt/dictionary_constraints
    rm -rf "$dest"
    mkdir -p "$dest"
    cp "$work/build"/* "$dest/"
    # The per-table JSON dumps the other fixtures carry, with adsdump (the same
    # tool the committed fixtures are dumped with). Set ADSDUMP to its path.
    if [ -n "$ADSDUMP" ]; then
        for t in log t u w w2 v x g h k; do
            WINEDEBUG=-all wine "$ADSDUMP" -t ansi "$t" adt "$(win "$dest/constraints.add")" ADSSYS "" \
                > "$dest/$t.json"
        done
    fi
    echo "wrote $dest" >&2
    exit 0
fi

rm -rf "$work/data"
mkdir -p "$work/data/dictionary_constraints"
cp "$root/fixtures/adt/dictionary_constraints"/* "$work/data/dictionary_constraints/"
cd "$work"
if [ "$mode" = remote ]; then
    SQL_SERVER_TYPE=2 WINEDEBUG=-all wine constraints.exe "$dll" \
        "$4\\dictionary_constraints\\constraints.add" < "$here/constraints.ops"
else
    WINEDEBUG=-all wine constraints.exe "$dll" "$(win "$work/data/dictionary_constraints/constraints.add")" \
        < "$here/constraints.ops"
fi
