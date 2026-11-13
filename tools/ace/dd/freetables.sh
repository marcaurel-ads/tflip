#!/bin/bash
# Records what a dictionary connection gets from free tables, the rights
# properties, an index made by a user without ALTER and a dictionary table
# whose files are missing, with a real Advantage Client Engine under Wine,
# running the steps of freetables.txt (see freetables.c) on a copy of the
# dictionary_props fixture with the free tables and the dictionary of
# fixtures/adt/dictionary beside it. Keeps the result as
# fixtures/dictionary/freetables.json and the files the engine made again as
# fixtures/dictionary/freetables/saved_*.
#
#     tools/ace/dd/freetables.sh <work folder> <ace32.dll as a Windows path>
#
# With a third argument, the server's path of the work folder
# (\\127.0.0.1:6262\data\freetables, the work folder being what that serves),
# it goes to a remote server instead, which answered every step alike.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
work=$1
dll=$2
rm -rf "$work/data"
mkdir -p "$work/data"
cp "$root"/fixtures/adt/dictionary_props/* "$work/data/"
cp "$root"/fixtures/adt/indexes/indexed.ad[ti] "$root"/fixtures/cdx/dbf_types/dbf_types.* "$work/data/"
cp "$root"/fixtures/adt/dictionary/dict.a* "$root"/fixtures/adt/dictionary/customers.ad? \
    "$root"/fixtures/adt/dictionary/orders*.ad? "$work/data/"
rm -f "$work/data"/*.json
i686-w64-mingw32-gcc -O1 -Wall -o "$work/freetables.exe" "$here/freetables.c"
cd "$work"
WINEDEBUG=-all wine freetables.exe "$dll" "$(winepath -w "$work/data")" \
    "$(winepath -w "$here/freetables.txt")" $3 2>/dev/null | tr -d '\r' >freetables.json
cp freetables.json "$root/fixtures/dictionary/freetables.json"
mkdir -p "$root/fixtures/dictionary/freetables"
cp "$work"/data/saved_* "$root/fixtures/dictionary/freetables/"
