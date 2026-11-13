#!/bin/sh
# Runs the scripts of aesapi/ on the AES-capable engine (see LOCAL.md), under
# Wine, each in a process of its own on a fresh data folder:
#
#     tools/ace/crypto/aesapi.sh <work folder> <folder of the patched 32-bit DLLs> [<prefix>]
#
# <work>/<script>.json is what a script returned, and <work>/snap/<script> a
# copy of the folder it left (the fixtures are fixtures/aes/api/).
#
# Only the documented API is driven; nothing reads the engine's code.
set -e
if [ $# -lt 2 ] || [ $# -gt 3 ] || [ -z "$1" ]; then
    echo "usage: aesapi.sh <work folder> <patched DLL folder> [<prefix>]" >&2
    exit 2
fi
here=$(cd "$(dirname "$0")" && pwd)
work=$(mkdir -p "$1" && cd "$1" && pwd)
dlls=$(cd "$2" && pwd)
only=${3:-}
mkdir -p "$work/snap"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/aesapi.exe" "$here/aesapi.c"
win() { echo "Z:$1" | tr / '\\'; }
cd "$dlls"
for path in "$here"/aesapi/"$only"*.txt; do
    script=$(basename "$path" .txt)
    rm -rf "$work/data"
    mkdir -p "$work/data"
    awk -v dir="$here/aesapi" '/^include /{ while ((getline l < (dir "/" $2)) > 0) print l; next } { print }' "$path" > "$work/$script.txt"
    WINEDEBUG=-all wine "$work/aesapi.exe" "$(win "$dlls/ace32.dll")" "$(win "$work/data")" \
        "$(win "$work/$script.txt")" | tr -d '\r' > "$work/$script.json"
    rm -rf "$work/snap/$script"
    cp -r "$work/data" "$work/snap/$script"
done
