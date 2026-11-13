#!/bin/sh
# Runs cache.c under Wine against the original engine's 32-bit ace32.dll.
#
#     tools/ace/cache/run.sh <work folder> <acesdk folder> [remote]
#
# Without "remote", on the local server in <work folder>/data; with it, on
# the evaluation server under Wine (setup in LOCAL.md)
# in its data folder's cache/ subfolder, which is emptied first. ADS_DATA
# overrides where that folder is on this machine.
set -e
here=$(cd "$(dirname "$0")" && pwd)
work=$1
sdk=$2
mkdir -p "$work"
cp "$sdk"/*.dll "$sdk"/*.dat "$sdk"/*.chr "$work"/
i686-w64-mingw32-gcc -O1 -Wall -o "$work/cache.exe" "$here/cache.c"
cd "$work"
if [ "${3:-}" = remote ]; then
    data=${ADS_DATA:-${EVAL_SERVER_PREFIX:?set EVAL_SERVER_PREFIX or ADS_DATA (LOCAL.md)}/drive_c/ads/data}/cache
    rm -rf "$data"
    mkdir -p "$data"
    WINEDEBUG=-all wine cache.exe ace32.dll '\\127.0.0.1:6262\data\cache' 2 | tr -d '\r'
else
    rm -rf data
    mkdir data
    WINEDEBUG=-all wine cache.exe ace32.dll "Z:$(pwd | tr / '\\')\\data" 1 | tr -d '\r'
fi
