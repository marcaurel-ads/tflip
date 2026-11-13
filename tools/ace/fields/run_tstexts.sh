#!/bin/sh
# Runs tstexts.exe under Wine in a fresh data folder.
#
#     tools/ace/fields/run_tstexts.sh <work folder> <ace32.dll as a Windows path>  < texts.txt
#
# texts.txt has a text per line, or `format`/`epoch` lines (see tstexts.c).
set -e
here=$(cd "$(dirname "$0")" && pwd)
work=$1
dll=$2
mkdir -p "$work"
rm -rf "$work/data"
mkdir "$work/data"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/tstexts.exe" "$here/tstexts.c"
cd "$work"
WINEDEBUG=-all wine tstexts.exe "$dll" "Z:$(echo "$work/data" | tr / '\\')"
