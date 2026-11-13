#!/bin/sh
# Rebuilds the golden dictionaries of fixtures/dictionary/proplong/ (property
# setters that make an object's properties longer than the record's `Property`
# field, or shorter again) with a real Advantage Client Engine, under Wine, and
# prints the memo references after each step. Clean-room: only the official ACE
# is called, and tflip's own output is compared to the result byte for byte.
#
#     tools/ace/dd/proplong.sh <work folder> <ace64.dll as a Windows path>
#
# The work folder gets proplong.exe and out/, rebuilt on every run; each
# scenario's folder is copied into fixtures/dictionary/proplong/.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
work=$1
dll=$2
mkdir -p "$work"
rm -rf "$work/out"
mkdir "$work/out"
PATH="$PATH:/usr/lib/mingw64-toolchain/bin"
x86_64-w64-mingw32-gcc -O1 -Wall -o "$work/proplong.exe" "$here/proplong.c"
cd "$work"
WINEDEBUG=-all wine proplong.exe "$dll" "Z:$(echo "$work/out" | tr / '\\')"
rm -rf "$root/fixtures/dictionary/proplong"
mkdir "$root/fixtures/dictionary/proplong"
for scenario in viewlater proplater; do
    cp -r "out/$scenario" "$root/fixtures/dictionary/proplong/$scenario"
done
