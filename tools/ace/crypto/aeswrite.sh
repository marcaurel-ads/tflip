#!/bin/sh
# Records what the original writes to AES data dictionaries, on the
# AES-capable engine (see LOCAL.md), under Wine:
#
#     tools/ace/crypto/aeswrite.sh <work folder> <folder of the patched 32-bit DLLs> [<prefix>]
#
# It builds aesdict.exe and runs the scripts of aeswrite/ in order, each in a
# process of its own, on <work>/data; after each, <work>/snap/<script> is a
# copy of data/ and <work>/<script>.json what the steps returned. The work
# folder is fixtures/aes/dictwrite/:
#
#     cp -r <work>/snap <work>/*.json fixtures/aes/dictwrite/
#
# With a prefix (40_), only the scripts that start with it run, on an empty data folder
# (a script that starts from nothing), leaving the other snapshots alone.
#
# Only the documented API is driven; nothing reads the engine's code.
set -e
if [ $# -lt 2 ] || [ $# -gt 3 ] || [ -z "$1" ]; then
    echo "usage: aeswrite.sh <work folder> <patched DLL folder> [<prefix>]" >&2
    exit 2
fi
here=$(cd "$(dirname "$0")" && pwd)
work=$(mkdir -p "$1" && cd "$1" && pwd)
dlls=$(cd "$2" && pwd)
only=${3:-}
rm -rf "$work/data"
[ -z "$only" ] && rm -rf "$work/snap"
mkdir -p "$work/data/c" "$work/data/e" "$work/data/f" "$work/data/h" "$work/data/i" \
    "$work/data/j" "$work/data/k" "$work/data/m" "$work/data/n" "$work/data/p" \
    "$work/data/q" "$work/data/r" "$work/data/s" "$work/data/t" "$work/data/u" \
    "$work/data/v" "$work/data/w" "$work/data/x" "$work/data/y" "$work/data/z" "$work/snap"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/aesdict.exe" "$here/aesdict.c"
win() { echo "Z:$1" | tr / '\\'; }
cd "$dlls"
for path in "$here"/aeswrite/"$only"*.txt; do
    script=$(basename "$path" .txt)
    cp "$path" "$work/"
    WINEDEBUG=-all wine "$work/aesdict.exe" "$(win "$dlls/ace32.dll")" "$(win "$work/data")" \
        "$(win "$work/$script.txt")" | tr -d '\r' > "$work/$script.json"
    rm -rf "$work/snap/$script"
    cp -r "$work/data" "$work/snap/$script"
done
