#!/bin/sh
# Runs cursor_set.exe on a fresh copy of the all_types fixture, under Wine.
#
#     tools/ace/errors/cursor_set.sh <work folder> <ace32.dll as a Windows path> > cursor_set.json
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
work=$1
dll=$2
mkdir -p "$work"
rm -rf "$work/data"
mkdir -p "$work/data/orig"
cp "$root"/fixtures/adt/all_types/all_types.ad[tm] "$work/data/"
cp "$root"/fixtures/adt/all_types/all_types.ad[tm] "$work/data/orig/"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/cursor_set.exe" "$here/cursor_set.c"
cd "$work"
WINEDEBUG=-all wine cursor_set.exe "$dll" "$(winepath -w "$work/data")"
wineserver -k
