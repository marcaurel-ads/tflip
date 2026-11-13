#!/bin/sh
# Builds seek.exe and records the original engine's seeks, seek-lasts and
# scopes for the seek goldens, under Wine:
#
#     tools/ace/seek/run.sh <work folder> <acesdk folder, a Unix path>
#
# The second argument is the folder that holds ace32.dll and its runtime files
# (ansi.chr, extend.chr, icudt40l.dat, ...). The local server reads
# adslocal.cfg from the DLL's folder, so each run uses an "engine" folder of
# symlinks to those files with an adslocal.cfg of its own. Writes
# collation.json (the charset_german `ca` tables, ADT, NTX and CDX, under
# ANSI_CHAR_SET=German, the OEM `co` ones under OEM_CHAR_SET=GERMAN) and
# oem.json (the same OEM seeks in the charset_default tables, under the
# default collations) and nchar.json (the adi_keys table's NChar and NVarChar
# orders) to the work folder; copy them to fixtures/seek/.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
work=$1
acesdk=$2
mkdir -p "$work"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/seek.exe" "$here/seek.c"
python3 "$here/cases.py" "$work"
win() { echo "Z:$(echo "$1" | tr / '\\')"; }

# <name> <ANSI_CHAR_SET> <OEM_CHAR_SET> <cases> <fixture folders...>
run() {
    name=$1
    ansi=$2
    oem=$3
    cases=$4
    shift 4
    engine=$work/engine-$name
    rm -rf "$engine" "$work/data-$name"
    mkdir "$engine" "$work/data-$name"
    for f in "$acesdk"/*; do
        [ "$(basename "$f")" = adslocal.cfg ] || ln -sf "$f" "$engine/"
    done
    printf '[settings]\r\nANSI_CHAR_SET=%s\r\nOEM_CHAR_SET=%s\r\n' "$ansi" "$oem" > "$engine/adslocal.cfg"
    for dir in "$@"; do
        kind=${dir%%/*}
        mkdir "$work/data-$name/$kind"
        for f in "$root/fixtures/$dir"/*; do
            case $f in *.json) ;; *) cp "$f" "$work/data-$name/$kind/" ;; esac
        done
    done
    (cd "$work" && WINEDEBUG=-all wine "$work/seek.exe" "$(win "$engine/ace32.dll")" \
        "$(win "$work/data-$name")" "$(win "$work/$cases.txt")" > "$work/$cases.json")
}

run german German GERMAN collation adt/charset_german ntx/charset_german cdx/charset_german
run default "" USA nchar adt/adi_keys
run oem "" USA oem ntx/charset_default cdx/charset_default
echo "wrote $work/collation.json, $work/oem.json and $work/nchar.json" >&2
