#!/bin/sh
# Rebuilds the golden dictionaries of fixtures/dictionary/proclong/ (stored
# procedures whose parameter lists are longer than the record's `Property`
# field, through AdsDDAddProcedure, SQL CREATE PROCEDURE and the property
# setter) with a real Advantage Client Engine, under Wine, and prints the memo
# references after each step. Clean-room: only the official ACE is called, and
# tflip's own output is compared to the result byte for byte.
#
#     tools/ace/dd/proclong.sh <work folder> <ace32.dll as a Windows path>
#
# The work folder gets proclong.exe and out/, rebuilt on every run; each
# scenario's folder is copied into fixtures/dictionary/proclong/.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
work=$1
dll=$2
mkdir -p "$work"
rm -rf "$work/out"
mkdir "$work/out"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/proclong.exe" "$here/proclong.c"
cd "$work"
WINEDEBUG=-all wine proclong.exe "$dll" "Z:$(echo "$work/out" | tr / '\\')"
rm -rf "$root/fixtures/dictionary/proclong"
mkdir "$root/fixtures/dictionary/proclong"
for scenario in proclongapi proclongext proclongsql; do
    cp -r "out/$scenario" "$root/fixtures/dictionary/proclong/$scenario"
done
