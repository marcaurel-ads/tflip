#!/bin/sh
# Records what AdsDDGrantPermission/AdsDDRevokePermission/AdsDDGetPermissions
# and SQL GRANT/REVOKE do with rights granted to the built-in DB:Public, which
# the engine keeps in a slot of the object itself, enciphered with the
# dictionary key and checked on every read, and how a user holds them, with a
# real Advantage Client Engine under Wine, running the scenarios of public.txt
# with grants.c, and keeps the dictionaries they leave as the goldens
# fixtures/dictionary/ops/public_<scenario>.{add,am,ai}. Clean-room: only the
# official ACE is called; tflip replays the recorded steps and compares its
# dictionaries to the goldens byte for byte (the slots' random parts masked,
# their plaintext decoded).
#
#     tools/ace/dd/public.sh <work folder> <ace64.dll as a Windows path> \
#         > fixtures/dictionary/public.json
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
    "Z:$(echo "$here/public.txt" | tr / '\\')"
for name in publicgrants publicrevoke publicusers; do
    for e in add am ai; do cp "out/$name/db.$e" "$root/fixtures/dictionary/ops/$name.$e"; done
done
echo "copied goldens to fixtures/dictionary/ops/" >&2
