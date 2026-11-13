#!/bin/sh
# Runs dbf.exe on fresh copies of the DBF fixtures, under Wine.
#
#     tools/ace/dbf/run.sh <work folder> <ace32.dll as a Windows path>  > getters.json
#
# The work folder gets dbf.exe and data/, rebuilt on every run. Each table
# is copied with its memo file and indexes into data/<type>/.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
work=$1
dll=$2
mkdir -p "$work"
rm -rf "$work/data"
mkdir -p "$work/data/ntx" "$work/data/cdx" "$work/data/vfp"
tables=""
# <type folder> <ACE table type> <char type> <fixture>/<table>
while read -r kind type char table; do
    base=${table%.dbf}
    cp "$root/fixtures/$kind/$base".* "$work/data/$kind/"
    rm -f "$work/data/$kind/${base##*/}.json"
    tables="$tables $type $char $kind/${base##*/}.dbf"
done <<LIST
ntx 1 1 dbf_types/dbf_types.dbf
ntx 1 1 dbf_ext/dbf_ext.dbf
ntx 1 1 dbf_ext/dbf_api.dbf
ntx 1 1 dbf_keys/dbf_keys.dbf
ntx 1 2 charset_default/co.dbf
cdx 2 1 dbf_types/dbf_types.dbf
cdx 2 1 dbf_ext/dbf_ext.dbf
cdx 2 1 cdx_types/cdx_types.dbf
cdx 2 1 dbf_keys/dbf_keys.dbf
vfp 4 1 dbf_types/dbf_types.dbf
vfp 4 1 vfp_types/vfp_types.dbf
vfp 4 1 vfp_more/vfp_more.dbf
vfp 4 1 vfp_keys/vfp_keys.dbf
vfp 4 1 dbf_keys/dbf_keys.dbf
LIST
i686-w64-mingw32-gcc -O1 -Wall -o "$work/dbf.exe" "$here/dbf.c"
cd "$work"
# shellcheck disable=SC2086
WINEDEBUG=-all wine dbf.exe "$dll" "Z:$(echo "$work/data" | tr / '\\')" $tables
