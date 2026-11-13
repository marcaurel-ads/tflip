#!/bin/sh
# Runs dd.exe on fresh copies of the dictionary fixtures, under Wine.
#
#     tools/ace/dd/run.sh <work folder> <ace32.dll as a Windows path>  > dd.json
#
# The work folder gets dd.exe and data/, rebuilt on every run. Each
# dictionary is copied with its tables into data/<fixture>/.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
work=$1
dll=$2
mkdir -p "$work"
rm -rf "$work/data"
dicts=""
for fixture in dictionary/dict psm/psm dictionary_props/props; do
    dir=${fixture%/*}
    mkdir -p "$work/data/$dir"
    cp "$root/fixtures/adt/$dir"/* "$work/data/$dir/"
    rm -f "$work/data/$dir"/*.json
    dicts="$dicts $fixture.add"
done
i686-w64-mingw32-gcc -O1 -Wall -o "$work/dd.exe" "$here/dd.c"
cd "$work"
# shellcheck disable=SC2086
WINEDEBUG=-all wine dd.exe "$dll" "Z:$(echo "$work/data" | tr / '\\')" $dicts
