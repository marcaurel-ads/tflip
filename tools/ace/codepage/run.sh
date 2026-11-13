#!/bin/sh
# Runs codepage.exe under Wine against a fresh data folder, recording how
# AdsSetStringFromCodePage converts each code page.
#
#     tools/ace/codepage/run.sh <work folder> <ace32.dll as a Windows path>  > setfromcodepage.json
#
# The work folder gets codepage.exe and data/, rebuilt on every run.
set -e
here=$(cd "$(dirname "$0")" && pwd)
work=$1
dll=$2
mkdir -p "$work"
rm -rf "$work/data"
mkdir "$work/data"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/codepage.exe" "$here/codepage.c"
cd "$work"
WINEDEBUG=-all wine codepage.exe "$dll" "Z:$(echo "$work/data" | tr / '\\')"
