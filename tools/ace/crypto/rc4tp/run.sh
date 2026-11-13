#!/bin/sh
# Runs rc4tp/<script>.txt on <work>/data/<script> with the probe driver
# (aesdict.c; api.c, which adds the AdsDD* and table calls, for the scripts
# named x*, y* and z*):
#
#     tools/ace/crypto/rc4tp/run.sh <work> <patched 32-bit DLL folder> <script>...
#
# <work>/<script>.json holds what the steps returned and <work>/data/<script>
# the dictionary (and tables) the original left. Under Wine; WINEPREFIX is
# ~/src/ace-sdk (see LOCAL.md).
set -e
here=$(cd "$(dirname "$0")" && pwd)
work=$(mkdir -p "$1" && cd "$1" && pwd)
dlls=$(cd "$2" && pwd)
shift 2
[ -x "$work/aesdict.exe" ] || i686-w64-mingw32-gcc -O1 -Wall -o "$work/aesdict.exe" "$here/../aesdict.c"
[ -x "$work/api.exe" ] || i686-w64-mingw32-gcc -O1 -Wall -o "$work/api.exe" "$here/api.c"
win() { echo "Z:$1" | tr / '\\'; }
cd "$dlls"
for script in "$@"; do
    rm -rf "$work/data/$script"
    mkdir -p "$work/data/$script"
    case $script in v5|v10|v11|y1) mkdir "$work/data/$script/free" ;; esac
    cp "$here/$script.txt" "$work/"
    case $script in v*|x*|y*|z*) driver=api ;; *) driver=aesdict ;; esac
    WINEDEBUG=-all wine "$work/$driver.exe" "$(win "$dlls/ace32.dll")" "$(win "$work/data")" \
        "$(win "$work/$script.txt")" | tr -d '\r' > "$work/$script.json"
done
