#!/bin/sh
# Works out crates/tflip-storage/src/icu_root.txt, the collation elements tflip
# builds NChar and NVarChar keys from, from the original engine's keys, and
# checks the result against the keys of random strings, under Wine:
#
#     tools/ace/seek/run_nkeys.sh <work folder> <acesdk folder, a Unix path> <allkeys.txt> [seed]
#
# allkeys.txt is the Default Unicode Collation Element Table of UCA 5.1.0
# (https://www.unicode.org/Public/UCA/5.1.0/allkeys.txt), the version of ICU
# 4.0, the engine's; only its list of contractions is used.
#
# Writes icu_root.txt and random.keys (the engine's keys of 20,000 random
# strings in an NChar(12) field, which the test fixture fixtures/seek/nkeys.txt
# is made of) to the work folder, and checks the table against every BMP and
# plane 1 character, the contractions and the random strings. See nkeys.py.
set -e
here=$(cd "$(dirname "$0")" && pwd)
work=$1
acesdk=$2
allkeys=$3
seed=${4:-1}
mkdir -p "$work"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/nkeys.exe" "$here/nkeys.c"
win() { echo "Z:$(echo "$1" | tr / '\\')"; }

# <strings> <field length> <keys>: the engine's key of each string.
keys() {
    rm -rf "$work/data"
    mkdir "$work/data"
    (cd "$work" && WINEDEBUG=-all wine "$work/nkeys.exe" "$(win "$acesdk/ace32.dll")" \
        "$(win "$work/data")" "$(win "$work/$1")" "$2" > "$work/$3")
}

nk() { python3 "$here/nkeys.py" "$@"; }

nk singles "$work/singles.txt"
keys singles.txt 4 singles.keys
nk astral "$work/astral.txt"
keys astral.txt 4 astral.keys
nk contractions "$allkeys" "$work/contractions.txt"
keys contractions.txt 8 contractions.keys
all="$work/singles.keys $work/astral.keys $work/contractions.keys"
nk pairs "$work/pairs.txt" $all
keys pairs.txt 4 pairs.keys
nk follow "$work/follow.txt" $all
keys follow.txt 8 follow.keys
nk table "$work/pairs.keys" "$work/follow.keys" $all > "$work/icu_root.txt"
nk check "$work/icu_root.txt" "$work/singles.keys" 4
nk check "$work/icu_root.txt" "$work/astral.keys" 4
nk check "$work/icu_root.txt" "$work/contractions.keys" 8
nk random "$work/icu_root.txt" "$seed" 20000 "$work/random.txt"
keys random.txt 12 random.keys
nk check "$work/icu_root.txt" "$work/random.keys" 12
