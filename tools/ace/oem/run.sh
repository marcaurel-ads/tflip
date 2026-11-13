#!/bin/sh
# Builds oem.exe and records the original engine's conversions of client text
# against an OEM table, under Wine:
#
#     tools/ace/oem/run.sh <work folder> <acesdk folder, a Unix path>
#
# The second argument is the folder that holds ace32.dll and its runtime files;
# the run uses an "engine" folder of symlinks to them with an adslocal.cfg of
# its own (OEM_CHAR_SET=USA). Writes oem.json to the work folder; copy it to
# fixtures/expr/.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
work=$1
acesdk=$2
mkdir -p "$work"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/oem.exe" "$here/oem.c"
python3 "$here/cases.py" "$work"
win() { echo "Z:$(echo "$1" | tr / '\\')"; }

engine=$work/engine
rm -rf "$engine" "$work/data"
mkdir "$engine" "$work/data" "$work/data/ntx"
for f in "$acesdk"/*; do
    [ "$(basename "$f")" = adslocal.cfg ] || ln -sf "$f" "$engine/"
done
printf '[settings]\r\nANSI_CHAR_SET=\r\nOEM_CHAR_SET=USA\r\n' > "$engine/adslocal.cfg"
for f in "$root"/fixtures/ntx/charset_default/co*; do
    case $f in *.json) ;; *) cp "$f" "$work/data/ntx/" ;; esac
done
(cd "$work" && WINEDEBUG=-all wine "$work/oem.exe" "$(win "$engine/ace32.dll")" \
    "$(win "$work/data")" "$(win "$work/oem.txt")" > "$work/oem.json")
echo "wrote $work/oem.json" >&2
