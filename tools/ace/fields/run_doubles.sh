#!/bin/sh
# Runs doubles.exe under Wine in a fresh data folder.
#
#     tools/ace/fields/run_doubles.sh <work folder> <ace32.dll as a Windows path>  < values.txt
#
# values.txt has a double's bits (16 hex digits) per line.
set -e
here=$(cd "$(dirname "$0")" && pwd)
work=$1
dll=$2
mkdir -p "$work"
rm -rf "$work/data"
mkdir "$work/data"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/doubles.exe" "$here/doubles.c"
cd "$work"
WINEDEBUG=-all wine doubles.exe "$dll" "Z:$(echo "$work/data" | tr / '\\')"
