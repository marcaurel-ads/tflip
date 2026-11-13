#!/bin/sh
# Runs dates.exe under Wine in a fresh data folder.
#
#     tools/ace/fields/run_dates.sh <work folder> <ace32.dll as a Windows path>  < texts.txt
#
# texts.txt has a text per line, or `format <format>` (see dates.c).
set -e
here=$(cd "$(dirname "$0")" && pwd)
work=$1
dll=$2
mkdir -p "$work"
rm -rf "$work/data"
mkdir "$work/data"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/dates.exe" "$here/dates.c"
cd "$work"
WINEDEBUG=-all wine dates.exe "$dll" "Z:$(echo "$work/data" | tr / '\\')"
