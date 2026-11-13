#!/bin/sh
# Makes the dictionaries of variants.txt with an ACE, under Wine.
#
#     tools/ace/dd/variants.sh <work folder> <ace32.dll as a Windows path>
#
# The work folder gets variants.exe and dd/, rebuilt on every run; each
# dictionary is dd/<name>/<name>.add with its .am and .ai.
set -e
here=$(cd "$(dirname "$0")" && pwd)
work=$1
dll=$2
mkdir -p "$work"
rm -rf "$work/dd"
mkdir "$work/dd"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/variants.exe" "$here/variants.c"
cp "$here/variants.txt" "$work/"
cd "$work"
WINEDEBUG=-all wine variants.exe "$dll" "Z:$(echo "$work/dd" | tr / '\\')" variants.txt
