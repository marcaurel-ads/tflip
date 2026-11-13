#!/bin/sh
# Records what SQL GRANT/REVOKE and AdsDDGrantPermission/AdsDDRevokePermission/
# AdsDDGetPermissions with object type 0 (and the rights to create) do, with a
# real Advantage Client Engine under Wine, running the scenarios of grants.txt
# (see grants.c), and keeps the dictionaries they leave as the goldens
# fixtures/dictionary/ops/grants_<scenario>.{add,am,ai}. Clean-room: only the
# official ACE is called; tflip replays the recorded steps and compares its
# dictionaries to the goldens byte for byte (the per-dictionary-key ciphertext
# masked, its plaintext decoded).
#
#     tools/ace/dd/grants.sh <work folder> <ace64.dll as a Windows path> \
#         > fixtures/dictionary/grants.json
#
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
work=$1
dll=$2
mkdir -p "$work/out"
rm -rf "$work/out"/*
# Every `table NAME` step copies this committed table.
cp "$root/fixtures/dictionary/ops/t.adt" "$work/out/"
PATH="$PATH:/usr/lib/mingw64-toolchain/bin"
x86_64-w64-mingw32-gcc -O1 -Wall -o "$work/grants.exe" "$here/grants.c" >&2
cd "$work"
WINEDEBUG=-all wine grants.exe "$dll" "Z:$(echo "$work/out" | tr / '\\')" \
    "Z:$(echo "$here/grants.txt" | tr / '\\')"
for name in sqlgrant sqlrevoke sqlcreate partial noraise raise api0 apirules apicreate columns; do
    for e in add am ai; do cp "out/$name/db.$e" "$root/fixtures/dictionary/ops/grants_$name.$e"; done
done
echo "copied goldens to fixtures/dictionary/ops/" >&2
