#!/bin/sh
# Runs fields.exe on fresh copies of the all_types and adi_keys fixtures,
# under Wine.
#
#     tools/ace/fields/run.sh <work folder> <ace32.dll as a Windows path>  > getters.json
#
# The work folder gets fields.exe and data/, rebuilt on every run.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
work=$1
dll=$2
mkdir -p "$work"
rm -rf "$work/data"
mkdir "$work/data"
cp "$root"/fixtures/adt/all_types/all_types.ad[tm] "$root"/fixtures/adt/adi_keys/keys.ad[ti] "$work/data/"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/fields.exe" "$here/fields.c"
cd "$work"
WINEDEBUG=-all wine fields.exe "$dll" "Z:$(echo "$work/data" | tr / '\\')"
