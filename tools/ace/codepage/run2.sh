#!/bin/sh
# Runs codepage2.exe under Wine against a fresh data folder, recording how
# AdsSetStringFromCodePage converts the four double-byte code pages. Needs a
# patched DLL that ships OpenSSL/ICU (see LOCAL.md); the stock SDK DLL converts
# these pages too, but the patched build is what the other probes use.
#
#     tools/ace/codepage/run2.sh <work folder> <ace32.dll as a Windows path>  > dbcs.json
#
# The work folder gets codepage2.exe and data/, rebuilt on every run.
set -e
here=$(cd "$(dirname "$0")" && pwd)
work=$1
dll=$2
mkdir -p "$work"
rm -rf "$work/data"
mkdir "$work/data"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/codepage2.exe" "$here/codepage2.c"
cd "$work"
WINEDEBUG=-all wine codepage2.exe "$dll" "Z:$(echo "$work/data" | tr / '\\')"
