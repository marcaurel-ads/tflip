#!/bin/sh
# Builds findservers.exe and runs it under Wine (or copy the .exe to a real
# Windows box and run it there -- see docs/ADS_BUGS.md, AdsFindServers).
#
#     tools/ace/findservers/run.sh <work folder> <ace32.dll as a Windows path> \
#         [<remote Data Source for the control connect>]  > findservers.json
#
# AdsFindServers needs no data folder; it broadcasts for servers. The optional
# third argument is a Data Source string for a plain remote connect, to tell a
# broken discovery apart from "no server running".
set -e
here=$(cd "$(dirname "$0")" && pwd)
work=$1
dll=$2
ds=$3
mkdir -p "$work"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/findservers.exe" "$here/findservers.c"
cd "$work"
WINEDEBUG=-all wine findservers.exe "$dll" "$ds"
