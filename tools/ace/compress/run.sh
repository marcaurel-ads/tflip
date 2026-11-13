#!/bin/sh
# Runs compress.c under Wine against the original engine's 32-bit ace32.dll.
#
#     tools/ace/compress/run.sh <work folder> <acesdk folder> [remote]
#
# Without "remote", on the local server in <work folder>/data; with it, on
# the evaluation server under Wine (setup in LOCAL.md),
# in its data folder.
set -e
here=$(cd "$(dirname "$0")" && pwd)
work=$1
sdk=$2
mkdir -p "$work"
cp "$sdk"/*.dll "$sdk"/*.dat "$sdk"/*.chr "$work"/
PATH="$PATH:/usr/lib/mingw64-toolchain/bin"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/compress.exe" "$here/compress.c"
cd "$work"
if [ "${3:-}" = remote ]; then
    WINEDEBUG=-all wine compress.exe ace32.dll '\\127.0.0.1:6262\data' REMOTE | tr -d '\r'
else
    mkdir -p data
    WINEDEBUG=-all wine compress.exe ace32.dll "Z:$(pwd | tr / '\\')\\data" LOCAL | tr -d '\r'
fi
