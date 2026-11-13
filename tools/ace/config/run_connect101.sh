#!/bin/sh
# Records what the options of AdsConnect101's connect string do and what
# AdsOpenTable101 takes from them (connect101.c), under Wine:
#
#     tools/ace/config/run_connect101.sh <work folder> <acesdk folder, a Unix path>
#
# The tables are copies of the fixtures: indexed.adt (ADT), c.dbf (a FoxPro
# table with a memo), n.dbf (a Clipper one) and v.dbf (a Visual FoxPro one),
# each with its second record deleted. Writes connect101.json (a list of
# {"string": <the options after ServerType=LOCAL;>, "result": ...}) to the work
# folder; copy it to fixtures/config/connect101.json. Also connect101_values.json,
# the same for the options of connect101_values.txt (what values each option
# takes), with no tables opened.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
work=$1
acesdk=$2
mkdir -p "$work"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/connect101.exe" "$here/connect101.c"
win() { echo "Z:$(echo "$1" | tr / '\\')"; }

engine=$work/engine
rm -rf "$engine"
mkdir "$engine"
for f in "$acesdk"/*; do
    [ "$(basename "$f")" = adslocal.cfg ] || ln -sf "$f" "$engine/"
done
printf '[settings]\r\nANSI_CHAR_SET=\r\nOEM_CHAR_SET=\r\n' > "$engine/adslocal.cfg"
dll=$(win "$engine/ace32.dll")

prepared=$work/prepared
rm -rf "$prepared"
mkdir "$prepared"
cp "$root"/fixtures/adt/indexes/indexed.adt "$root"/fixtures/adt/indexes/indexed.adi "$prepared/"
cp "$root"/fixtures/cdx/dbf_types/dbf_types.dbf "$prepared/c.dbf"
cp "$root"/fixtures/cdx/dbf_types/dbf_types.fpt "$prepared/c.fpt"
cp "$root"/fixtures/ntx/dbf_types/dbf_types.dbf "$prepared/n.dbf"
cp "$root"/fixtures/ntx/dbf_types/dbf_types.dbt "$prepared/n.dbt"
cp "$root"/fixtures/vfp/dbf_types/dbf_types.dbf "$prepared/v.dbf"
cp "$root"/fixtures/vfp/dbf_types/dbf_types.fpt "$prepared/v.fpt"
tables="indexed.adt c.dbf n.dbf v.dbf"
PREP=1 WINEDEBUG=-all wine "$work/connect101.exe" "$dll" "$(win "$prepared")" "" $tables

# <connect string options> [<tables>]
run() {
    rm -rf "$work/data"
    cp -r "$prepared" "$work/data"
    printf '%s{"string": "%s", "result": ' "$sep" "$1"
    WINEDEBUG=-all wine "$work/connect101.exe" "$dll" "$(win "$work/data")" "$1" $2 \
        | tr -d '\r\n'
    printf '}'
    sep=",
"
}

sep=""
{
    echo "["
    while IFS= read -r options; do
        run "$options" "$tables"
    done < "$here/connect101.txt"
    echo "
]"
} > "$work/connect101.json"
sep=""
{
    echo "["
    while IFS= read -r options; do
        run "$options"
    done < "$here/connect101_values.txt"
    echo "
]"
} > "$work/connect101_values.json"
echo "wrote $work/connect101.json and connect101_values.json" >&2
