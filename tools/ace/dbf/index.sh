#!/bin/sh
# Runs index.exe on fresh copies of the DBF fixtures' index files (NTX,
# CDX, IDX), under Wine.
#
#     tools/ace/dbf/index.sh <work folder> <ace32.dll as a Windows path>  > indexes.json
#
# The work folder gets index.exe and data/, rebuilt on every run. Each table
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
# <type folder> <ACE table type> <char type> <fixture>/<table> <index files>
while read -r kind type char table indexes; do
    base=${table%.dbf}
    dir=$(dirname "$base")
    name=${base##*/}
    cp "$root/fixtures/$kind/$base".* "$work/data/$kind/"
    for index in $(echo "$indexes" | tr , ' '); do
        cp "$root/fixtures/$kind/$dir/$index" "$work/data/$kind/"
    done
    rm -f "$work/data/$kind/$name.json"
    tables="$tables $type $char $kind/$name.dbf $indexes"
done <<LIST
ntx 1 1 dbf_keys/dbf_keys.dbf i_char.ntx,i_num.ntx,i_date.ntx,i_log.ntx,i_int.ntx,i_dbl.ntx,i_desc.ntx,i_multi.ntx
ntx 1 1 ntx_options/opts.dbf x_upper.ntx,x_mix.ntx,cond.ntx,uniq.ntx,desc.ntx
ntx 1 1 ntx_index/ntx_index.dbf name.ntx
ntx 1 1 charset_default/ca.dbf ca_c.ntx,ca_w.ntx
cdx 2 1 dbf_keys/dbf_keys.dbf dbf_keys.cdx
cdx 2 1 cdx_options/opts.dbf opts.cdx,second.cdx,single.idx
cdx 2 1 cdx_types/cdx_types.dbf cdx_types.cdx
cdx 2 1 charset_default/ca.dbf ca.cdx
vfp 4 1 dbf_keys/dbf_keys.dbf dbf_keys.cdx
vfp 4 1 vfp_keys/vfp_keys.dbf vfp_keys.cdx
vfp 4 1 cdx_options/opts.dbf opts.cdx,second.cdx,single.idx
LIST
i686-w64-mingw32-gcc -O1 -Wall -o "$work/index.exe" "$here/index.c"
cd "$work"
# shellcheck disable=SC2086
WINEDEBUG=-all wine index.exe "$dll" "Z:$(echo "$work/data" | tr / '\\')" $tables
