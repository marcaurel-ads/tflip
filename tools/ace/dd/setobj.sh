#!/bin/sh
# Records what AdsDDSetObjectAccessRights does, with a real Advantage Client
# Engine under Wine: which access-string letter sets which permission bit, how
# the object's type is inferred from its name, and that it replaces rather than
# merges. Clean-room: only the official ACE is called; the derived grammar is
# what tflip's AdsDDSetObjectAccessRights reimplements.
#
#     tools/ace/dd/setobj.sh <work folder> <ace64.dll as a Windows path>
#
# Prints its findings; it builds no golden fixture (the enciphered Permission
# object is the grant path's, verified byte for byte elsewhere, so tflip checks
# this one by a write/read round-trip).
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
work=$1
dll=$2
sdk=${ACE_SDK64:-"$HOME/src/ace-sdk/drive_c/Program Files/Advantage 12.0/acesdk"}
mkdir -p "$work/out"
rm -rf "$work/out"/*
PATH="$PATH:/usr/lib/mingw64-toolchain/bin"
x86_64-w64-mingw32-gcc -O1 -Wall -I "$sdk" -o "$work/setobj.exe" "$here/setobj.c"
cd "$work"
WINEDEBUG=-all wine setobj.exe "$dll" "Z:$(echo "$work/out" | tr / '\\')"
