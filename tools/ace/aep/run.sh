#!/bin/sh
# Records aep.txt on the original's remote server (under Wine, setup in
# LOCAL.md; it serves C:\ads\data of its prefix as the share "data"):
#
#     tools/ace/aep/run.sh > fixtures/sql/aep.json
#
# The work folder is aep/ in that share, its data/ rebuilt with ae/ holding
# aep.c built as a 64-bit DLL (the server is) that links the server's ace64.dll.
# Another script, its C source and the library's name can be given:
#
#     tools/ace/aep/run.sh aeptrig.txt aeptrig.c aeptrig.aep > fixtures/sql/aeptrig.json
set -e
script=${1:-aep.txt}
source=${2:-aep.c}
library=${3:-aeptest.aep}
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
pfx=${EVAL_SERVER_PREFIX:?set EVAL_SERVER_PREFIX to the Wine prefix of the evaluation server (LOCAL.md)}
served=$pfx/drive_c/ads/data
server="$pfx/drive_c/Program Files/Advantage 12.0/Server"
dll='C:\Program Files (x86)\Advantage 12.0\acesdk\ace32.dll'
work=$served/${script%.txt}
PATH="$PATH:/usr/lib/mingw64-toolchain/bin"
rm -rf "$work"
mkdir -p "$work/data/ae"
x86_64-w64-mingw32-gcc -O1 -Wall -shared -o "$work/data/ae/$library" "$here/$source" "$server/ace64.dll"
x86_64-w64-mingw32-gcc -O1 -Wall -DPLAIN -shared -o "$work/data/ae/plain_$library" "$here/$source" "$server/ace64.dll"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/sql.exe" "$root/tools/ace/sql/sql.c"
iconv -f utf-8 -t cp1252 "$here/$script" > "$work/$script"
cd "$work"
SQL_SERVER_TYPE=2 WINEPREFIX=${WINEPREFIX:-$HOME/src/ace-sdk} WINEDEBUG=-all \
    wine sql.exe "$dll" "\\\\127.0.0.1:6262\\data\\${script%.txt}\\data" "$script"
