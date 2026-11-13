#!/bin/sh
# Rebuilds the golden dictionary of fixtures/dictionary/removelong/ (objects
# whose properties continue in the `More Property` memo, removed) with a real
# Advantage Client Engine, under Wine, and prints the memo references after
# each step. Clean-room: only the official ACE is called, and tflip's own
# output is compared to the result byte for byte.
#
#     tools/ace/dd/removelong.sh <work folder> <ace32.dll as a Windows path>
#
# The work folder gets removelong.exe and out/, rebuilt on every run; the
# scenario's folder is copied into fixtures/dictionary/removelong/.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
work=$1
dll=$2
mkdir -p "$work"
rm -rf "$work/out"
mkdir "$work/out"
i686-w64-mingw32-gcc -O0 -Wall -o "$work/removelong.exe" "$here/removelong.c"
cd "$work"
WINEDEBUG=-all wine removelong.exe "$dll" "Z:$(echo "$work/out" | tr / '\\')"
rm -rf "$root/fixtures/dictionary/removelong"
mkdir "$root/fixtures/dictionary/removelong"
for scenario in removed extents; do
    cp -r "out/$scenario" "$root/fixtures/dictionary/removelong/$scenario"
done
