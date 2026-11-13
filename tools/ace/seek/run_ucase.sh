#!/bin/sh
# Records the keys of UPPER(n) and LOWER(n) of every BMP character and of the
# first supplementary plane's under Wine, and works out
# crates/tflip-storage/src/icu_case.txt from them:
#
#     tools/ace/seek/run_ucase.sh <work folder> <acesdk folder, a Unix path>
#
# Needs icu_root.txt (the table run_nkeys.sh works out) in tflip's source.
# Writes icu_case.txt to the work folder.
set -e
here=$(cd "$(dirname "$0")" && pwd)
work=$1
acesdk=$2
mkdir -p "$work"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/ucase.exe" "$here/ucase.c"
win() { echo "Z:$(echo "$1" | tr / '\\')"; }

# <strings> <keys>
keys() {
    rm -rf "$work/data"
    mkdir "$work/data"
    (cd "$work" && WINEDEBUG=-all wine "$work/ucase.exe" "$(win "$acesdk/ace32.dll")" \
        "$(win "$work/data")" "$(win "$work/$1")" 4 > "$work/$2")
}

root="$here/../../../crates/tflip-storage/src/icu_root.txt"
python3 "$here/ucase.py" strings "$work/bmp.txt" 1 FFFF
keys bmp.txt bmp.keys
python3 "$here/ucase.py" strings "$work/astral.txt" 10000 1FFFF
keys astral.txt astral.keys
(cd "$here" && python3 ucase.py table "$root" "$work/bmp.keys" "$work/astral.keys" \
    > "$work/icu_case.txt")
