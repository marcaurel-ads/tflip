#!/bin/sh
# Runs login.exe on a fresh copy of the dictionary fixture, under Wine.
#
#     tools/ace/dd/run_login.sh <work folder> <ace32.dll as a Windows path>  > login.json
#
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
work=$1
dll=$2
mkdir -p "$work"
rm -rf "$work/data"
mkdir -p "$work/data/dictionary"
cp "$root/fixtures/adt/dictionary"/* "$work/data/dictionary/"
rm -f "$work/data/dictionary"/*.json
i686-w64-mingw32-gcc -O1 -Wall -o "$work/login.exe" "$here/login.c"
cd "$work"
WINEDEBUG=-all wine login.exe "$dll" "Z:$(echo "$work/data/dictionary/dict.add" | tr / '\\')"
