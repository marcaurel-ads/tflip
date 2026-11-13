#!/bin/sh
# Runs bookmarks.exe on a fresh copy of fixtures/adt/indexes, under Wine.
#
#     tools/ace/bookmarks/run.sh <work folder> <ace32.dll as a Windows path> [<ops file>] > bookmarks.json
#
# With the original engine's ace32.dll this makes fixtures/bookmarks/bookmarks.json.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
work=$1
dll=$2
ops=${3:-$here/ops.txt}
mkdir -p "$work"
rm -rf "$work/data"
mkdir "$work/data"
cp "$root"/fixtures/adt/indexes/indexed.ad[ti] "$work/data/"
cp "$ops" "$work/ops.txt"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/bookmarks.exe" "$here/bookmarks.c"
cd "$work"
WINEDEBUG=-all wine bookmarks.exe "$dll" "$(winepath -w "$work/data")" ops.txt 2>/dev/null | tr -d '\r'
