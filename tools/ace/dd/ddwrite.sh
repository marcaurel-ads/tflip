#!/bin/sh
# Rebuilds the golden dictionaries of fixtures/dictionary/viewlong/ and
# fixtures/dictionary/dbftables/ and fixtures/dictionary/dbfcorners/ with a
# real Advantage Client Engine, under Wine. Clean-room: only the official ACE is called, and tflip's own output is
# compared to the result byte for byte.
#
#     tools/ace/dd/ddwrite.sh <work folder> <ace32.dll as a Windows path>
#
# The work folder gets ddwrite.exe and out/, rebuilt on every run; each
# scenario's folder (the dictionary, the tables as added and, in free/, as they
# were before) is copied into fixtures/dictionary/<scenario>/.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
work=$1
dll=$2
mkdir -p "$work"
rm -rf "$work/out"
mkdir "$work/out"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/ddwrite.exe" "$here/ddwrite.c"
cd "$work"
WINEDEBUG=-all wine ddwrite.exe "$dll" "Z:$(echo "$work/out" | tr / '\\')"
for scenario in viewlong dbftables dbfcorners; do
    rm -rf "$root/fixtures/dictionary/$scenario"
    cp -r "out/$scenario" "$root/fixtures/dictionary/$scenario"
done
