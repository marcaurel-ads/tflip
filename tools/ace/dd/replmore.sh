#!/bin/sh
# Runs replmore.c under Wine and prints what it finds:
#
#     tools/ace/dd/replmore.sh <work folder> <ace64.dll as a Windows path>
#
# The work folder gets replmore.exe and out/, rebuilt on every run.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
work=$1
dll=$2
sdk=${ACE_SDK64:-"$HOME/src/ace-sdk/drive_c/Program Files/Advantage 12.0/acesdk"}
mkdir -p "$work/out"
rm -rf "$work/out"/*
cp "$root/fixtures/dictionary/ops/t.adt" "$root/fixtures/dictionary/ops/t.adi" "$work/out/"
PATH="$PATH:/usr/lib/mingw64-toolchain/bin"
x86_64-w64-mingw32-gcc -O1 -Wall -I "$sdk" -o "$work/replmore.exe" "$here/replmore.c"
cd "$work"
WINEDEBUG=-all wine replmore.exe "$dll" "Z:$(echo "$work/out" | tr / '\\')"
