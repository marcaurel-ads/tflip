#!/bin/bash
# Runs param_n_mg.exe under Wine against the ADS 12 evaluation server
# (under Wine, setup in LOCAL.md):
#
#     tools/ace/errors/param_n_mg.sh <work folder> <ace32.dll as a Windows path> [<server path>] [<cases file>] > param_n_mg.json
#
# With the original engine's ace32.dll this makes
# fixtures/errors/param_n_mg.json from param_n_mg.txt.
set -e
here=$(cd "$(dirname "$0")" && pwd)
work=$1
dll=$2
server=${3:-127.0.0.1}
cases=${4:-$here/param_n_mg.txt}
mkdir -p "$work"
cp "$cases" "$work/cases.txt"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/param_n_mg.exe" "$here/param_n_mg.c"
mkdir -p "$work/local"
cd "$work"
WINEDEBUG=-all wine param_n_mg.exe "$dll" "$server" cases.txt "$(winepath -w "$work/local")" 2>/dev/null | tr -d '\r'
