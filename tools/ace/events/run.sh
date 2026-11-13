#!/bin/sh
# Runs ev.exe on a script of this folder, under Wine, against the ADS 12
# evaluation server (under Wine, setup in LOCAL.md):
#
#     tools/ace/events/run.sh <work folder> <ace32.dll as a Windows path> <script> <server folder> [local folder]
#             > fixtures/events/<script>.json
#
# <server folder> is where the script makes its dictionaries, as the server
# names it (\\127.0.0.1:6262\data\evh); the [local folder] is the same folder
# on this machine ($EVAL_SERVER_PREFIX/drive_c/ads/data/evh), which is
# emptied first (the scripts create their dictionary, which fails if it is
# there). EV_SERVER_TYPE=1 runs on the local server instead: <server folder>
# is then a Windows path of a folder, emptied as well.
set -e
here=$(cd "$(dirname "$0")" && pwd)
work=$1
dll=$2
script=$3
share=$4
local=$5
mkdir -p "$work"
if [ -n "$local" ]; then
    rm -rf "$local"
    mkdir -p "$local"
fi
i686-w64-mingw32-gcc -O1 -Wall -o "$work/ev.exe" "$here/ev.c"
iconv -f utf-8 -t cp1252 "$here/$script" > "$work/$script"
cd "$work"
WINEDEBUG=-all wine ev.exe "$dll" "$share" "$script"
