#!/bin/sh
# Runs rights.exe on a fresh copy of the dictionary_props fixture, under Wine.
#
#     tools/ace/dd/run_rights.sh <work folder> <ace32.dll as a Windows path>  > rights.json
#
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
work=$1
dll=$2
mkdir -p "$work"
rm -rf "$work/data"
mkdir -p "$work/data/dictionary_props"
cp "$root/fixtures/adt/dictionary_props"/* "$work/data/dictionary_props/"
rm -f "$work/data/dictionary_props"/*.json
i686-w64-mingw32-gcc -O1 -Wall -o "$work/rights.exe" "$here/rights.c"
cd "$work"
WINEDEBUG=-all wine rights.exe "$dll" "Z:$(echo "$work/data/dictionary_props/props.add" | tr / '\\')"
