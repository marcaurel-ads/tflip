#!/bin/sh
# Records what opening a table does when its ADI index was built under another
# collation than the connection's, with the ADS_REINDEX_ON_COLLATION_MISMATCH
# and ADS_IGNORE_COLLATION_MISMATCH open options (mismatch.c), under Wine:
#
#     tools/ace/collation/run_mismatch.sh <work folder> <acesdk folder, a Unix path>
#
# The second argument is the folder that holds ace32.dll and its runtime files
# (ansi.chr, ...); the local server reads adslocal.cfg from the DLL's folder,
# so each run uses an "engine" folder of symlinks to those files with an
# adslocal.cfg of its own. Writes mismatch.json, custom/ (the custom-order
# table, made under ANSI_CHAR_SET=German) and ca_reindexed.adi (the index file
# of fixtures/adt/charset_german/ca.adt after the reindex on opening) to the
# work folder; copy them to fixtures/collation/mismatch*.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
work=$1
acesdk=$2
mkdir -p "$work"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/mismatch.exe" "$here/mismatch.c"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/mismatch_custom.exe" "$here/mismatch_custom.c"
win() { echo "Z:$(echo "$1" | tr / '\\')"; }

engine() {
    name=$1
    ansi=$2
    dir=$work/engine-$name
    rm -rf "$dir"
    mkdir "$dir"
    for f in "$acesdk"/*; do
        [ "$(basename "$f")" = adslocal.cfg ] || ln -sf "$f" "$dir/"
    done
    printf '[settings]\r\nANSI_CHAR_SET=%s\r\nOEM_CHAR_SET=USA\r\n' "$ansi" > "$dir/adslocal.cfg"
}
engine default ""
engine german German

# The table with a custom order, built under German.
rm -rf "$work/custom"
mkdir "$work/custom"
WINEDEBUG=-all wine "$work/mismatch_custom.exe" "$(win "$work/engine-german/ace32.dll")" \
    "$(win "$work/custom")"

# <name> <fixture folder> <table> <index file> <options> <other user> [<collation>]
run() {
    fixture=$1
    table=$2
    index=$3
    options=$4
    other=$5
    collation=$6
    rm -rf "$work/data"
    mkdir "$work/data"
    cp "$fixture"/* "$work/data/"
    label=${fixture#$root/fixtures/}
    case $label in /*) label=collation/mismatch/custom ;; esac
    printf '%s{"fixture": "%s", "table": "%s", "options": "%s", "other_user": %s, "collation": "%s", "result": ' \
        "$sep" "$label" "$table" "$options" "$([ "$other" = 1 ] && echo true || echo false)" \
        "$collation"
    WINEDEBUG=-all wine "$work/mismatch.exe" "$(win "$work/engine-default/ace32.dll")" \
        "$(win "$work/data")" "$table" "$index" 3 "$options" "$other" $collation | tr -d '\r\n'
    printf '}'
    sep=",
"
    if [ "$options" = 101 ] && [ "$table" = ca.adt ] && [ "$other" = 0 ]; then
        cp "$work/data/ca.adi" "$work/ca_reindexed.adi"
    fi
}

sep=""
{
    echo "["
    german=$root/fixtures/adt/charset_german
    for options in 1 2 4 101 102 103 104 201 204 301 5; do
        run "$german" ca.adt ca.adi $options 0
    done
    run "$german" ca.adt ca.adi 104 1
    run "$german" ca.adt ca.adi 101 1
    run "$german" ca.adt ca.adi 204 1
    for options in 1 101 201; do
        run "$work/custom" cu.adt cu.adi $options 0
    done
    dynamic=$root/fixtures/adt/collation_dynamic
    for collation in "" GERMAN_VFP_CI_AS_1252 CZECH_VFP_CI_AS_1250; do
        for options in 1 101 201; do
            run "$dynamic" cze.adt cze.adi $options 0 $collation
        done
    done
    echo "
]"
} > "$work/mismatch.json"
echo "wrote $work/mismatch.json" >&2
