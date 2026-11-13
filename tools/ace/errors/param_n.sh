#!/bin/bash
# Runs param_n.exe on a fresh copy of fixtures/adt/indexes, fixtures/adt/all_types and the dictionary of
# fixtures/errors/param_n/indexed_table (see param_n_dict.sh), under Wine.
#
#     tools/ace/errors/param_n.sh <work folder> <ace32.dll as a Windows path> [<cases file>] > param_n.json
#
# With the original engine's ace32.dll this makes fixtures/errors/param_n.json
# from param_n.txt.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
work=$1
dll=$2
cases=${3:-$here/param_n.txt}
mkdir -p "$work"
rm -rf "$work/data"
mkdir "$work/data"
cp "$root"/fixtures/adt/indexes/indexed.ad[ti] "$work/data/"
cp "$root"/fixtures/adt/all_types/all_types.ad[tm] "$work/data/"
cp "$root"/fixtures/errors/param_n/indexed_table.{add,ai,am} "$work/data/"
cp "$cases" "$work/cases.txt"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/param_n.exe" "$here/param_n.c"
cd "$work"
WINEDEBUG=-all wine param_n.exe "$dll" "$(winepath -w "$work/data")" cases.txt 2>/dev/null | tr -d '\r'
