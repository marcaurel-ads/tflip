#!/bin/sh
# Makes AES data dictionaries with known secrets on the AES-capable engine
# (see LOCAL.md), under Wine:
#
#     tools/ace/crypto/aesdict.sh <work folder> <folder of the patched 32-bit DLLs>
#
# It builds aesdict.exe and runs aesdict.txt, aesdict2.txt and aesdict3.txt
# in turn, each in a process of its own, on <work>/data, and writes
# aesdict.json, aesdict2.json and aesdict3.json in the work folder. The
# dictionaries in data/ (a0 to a4, e3; a2, a3, a4 and e3 are kept in fixtures/aes/dict/):
#
#     cp -r <work>/data/a2 <work>/data/a3 <work>/data/a4 <work>/data/e3 fixtures/aes/dict/
#     cp <work>/aesdict*.json fixtures/aes/dict/
#
# Only the documented API is driven; nothing reads the engine's code.
set -e
if [ $# -ne 2 ] || [ -z "$1" ]; then
    echo "usage: aesdict.sh <work folder> <patched DLL folder>" >&2
    exit 2
fi
here=$(cd "$(dirname "$0")" && pwd)
work=$(mkdir -p "$1" && cd "$1" && pwd)
dlls=$(cd "$2" && pwd)
rm -rf "$work/data"
mkdir -p "$work/data/a0" "$work/data/a1" "$work/data/a2" "$work/data/a3" \
    "$work/data/a4" "$work/data/r3" "$work/data/e3"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/aesdict.exe" "$here/aesdict.c"
win() { echo "Z:$1" | tr / '\\'; }
cd "$dlls"
for script in aesdict aesdict2 aesdict3; do
    cp "$here/$script.txt" "$work/"
    WINEDEBUG=-all wine "$work/aesdict.exe" "$(win "$dlls/ace32.dll")" "$(win "$work/data")" \
        "$(win "$work/$script.txt")" | tr -d '\r' > "$work/$script.json"
done
