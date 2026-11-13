#!/bin/sh
# Rebuilds the replication golden dictionaries in fixtures/dictionary/ops/ with
# a real Advantage Client Engine, under Wine. Clean-room: only the official ACE
# is called, and tflip's own output is compared to the result byte-for-byte.
#
#     tools/ace/dd/replops.sh <work folder> <ace64.dll as a Windows path>
#
# The work folder gets replops.exe and out/, rebuilt on every run; each golden
# <name>.{add,am,ai} is copied into fixtures/dictionary/ops/.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
work=$1
dll=$2
sdk=${ACE_SDK64:-"$HOME/src/ace-sdk/drive_c/Program Files/Advantage 12.0/acesdk"}
mkdir -p "$work/out"
rm -f "$work/out"/*
# The article scenarios publish table t, added from these committed fixtures.
cp "$root/fixtures/dictionary/ops/t.adt" "$root/fixtures/dictionary/ops/t.adi" "$work/out/"
PATH="$PATH:/usr/lib/mingw64-toolchain/bin"
x86_64-w64-mingw32-gcc -O1 -Wall -I "$sdk" -o "$work/replops.exe" "$here/replops.c"
cd "$work"
WINEDEBUG=-all wine replops.exe "$dll" "Z:$(echo "$work/out" | tr / '\\')"
# Most scenarios are out/<name>/<name>.{add,am,ai}. The subscription scenarios
# name the dict db.add (matching the tflip test) and also leave the queue table
# files q.{add,adm,adi}, copied as <name>_q.*.
for d in out/*/; do
    name=$(basename "$d")
    if [ -f "$d$name.add" ]; then
        for e in add am ai; do cp "$d$name.$e" "$root/fixtures/dictionary/ops/"; done
    elif [ -f "${d}db.add" ]; then
        for e in add am ai; do cp "${d}db.$e" "$root/fixtures/dictionary/ops/$name.$e"; done
        for e in add adm adi; do
            [ -f "${d}q.$e" ] && cp "${d}q.$e" "$root/fixtures/dictionary/ops/${name}_q.$e"
        done
    fi
done
echo "copied goldens to fixtures/dictionary/ops/"
