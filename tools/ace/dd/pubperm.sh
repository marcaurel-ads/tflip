#!/bin/sh
# Records what AdsDDGrantPermission/AdsDDRevokePermission/AdsDDGetPermissions
# and SQL GRANT/REVOKE do on publications, articles and subscriptions, with a
# real Advantage Client Engine under Wine, and rebuilds the golden
# dictionaries fixtures/dictionary/ops/{perm{pub,sub}*,revkinds,revnone,colrev*}
# .{add,am,ai} (see pubperm.c). Clean-room: only the official ACE is called;
# tflip replays the recorded calls and compares its dictionaries to the goldens
# byte-for-byte (the per-dictionary-key ciphertext masked, its plaintext
# decoded). For the both-ways check, run the built pubperm.exe itself against
# tflip's ace64.dll (this script would overwrite the goldens).
#
#     tools/ace/dd/pubperm.sh <work folder> <ace64.dll as a Windows path> \
#         > fixtures/dictionary/pubperm.json
#
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
work=$1
dll=$2
sdk=${ACE_SDK64:-"$HOME/src/ace-sdk/drive_c/Program Files/Advantage 12.0/acesdk"}
mkdir -p "$work/out"
rm -rf "$work/out"/*
# The "bits" scenario publishes table t, added from these committed fixtures.
cp "$root/fixtures/dictionary/ops/t.adt" "$root/fixtures/dictionary/ops/t.adi" "$work/out/"
PATH="$PATH:/usr/lib/mingw64-toolchain/bin"
x86_64-w64-mingw32-gcc -O1 -Wall -I "$sdk" -o "$work/pubperm.exe" "$here/pubperm.c" >&2
cd "$work"
WINEDEBUG=-all wine pubperm.exe "$dll" "Z:$(echo "$work/out" | tr / '\\')"
for name in permpub permsub permpubrev permsubrevnone permpubinh permpubrevg permpubri permpubrg permpubgg permpubgrg permpuballg revkinds revnone colrev colrev0; do
    for e in add am ai; do cp "out/$name/db.$e" "$root/fixtures/dictionary/ops/$name.$e"; done
done
echo "copied goldens to fixtures/dictionary/ops/" >&2
