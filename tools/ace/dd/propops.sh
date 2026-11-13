#!/bin/sh
# Rebuilds the user / group / procedure property-setter golden dictionaries in
# fixtures/dictionary/ops/ with a real Advantage Client Engine, under Wine.
# Clean-room: only the official ACE is called, and tflip's own output is compared
# to the result byte-for-byte.
#
#     tools/ace/dd/propops.sh <work folder> <ace64.dll as a Windows path>
#
# The work folder gets propops.exe and out/, rebuilt on every run; each golden
# <name>.{add,am,ai} is copied into fixtures/dictionary/ops/.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
work=$1
dll=$2
sdk=${ACE_SDK64:-"$HOME/src/ace-sdk/drive_c/Program Files/Advantage 12.0/acesdk"}
mkdir -p "$work/out"
rm -f "$work/out"/*
PATH="$PATH:/usr/lib/mingw64-toolchain/bin"
x86_64-w64-mingw32-gcc -O1 -Wall -I "$sdk" -o "$work/propops.exe" "$here/propops.c"
cd "$work"
WINEDEBUG=-all wine propops.exe "$dll" "Z:$(echo "$work/out" | tr / '\\')"
for d in out/*/; do
    name=$(basename "$d")
    [ -f "$d$name.add" ] || continue
    for e in add am ai; do cp "$d$name.$e" "$root/fixtures/dictionary/ops/"; done
done
echo "copied goldens to fixtures/dictionary/ops/"
