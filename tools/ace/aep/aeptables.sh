#!/bin/sh
# Records aeptables.txt on the original's local server, under Wine:
#
#     tools/ace/aep/aeptables.sh <work folder> > fixtures/sql/aeptables.json
#
# The work folder gets a copy of the SDK folder (its ace32.dll must be the one
# both sql.exe and the library load), sql.exe, and data/ with ae/ holding
# aeptables.c built as a 32-bit DLL that links that ace32.dll.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
work=$(cd "$1" && pwd)
sdk=${SDK:-$HOME/src/ace-sdk/drive_c/Program Files (x86)/Advantage 12.0/acesdk}
PATH="$PATH:/usr/lib/mingw64-toolchain/bin"
rm -rf "$work/data" "$work/sdk"
mkdir -p "$work/data/ae" "$work/sdk"
cp -r "$sdk"/. "$work/sdk/"
i686-w64-mingw32-gcc -O1 -Wall -shared -static-libgcc -Wl,--enable-stdcall-fixup,--kill-at -o "$work/data/ae/aeptables.aep" "$here/aeptables.c" "$work/sdk/ace32.dll"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/sdk/sql.exe" "$root/tools/ace/sql/sql.c"
iconv -f utf-8 -t cp1252 "$here/aeptables.txt" > "$work/aeptables.txt"
cd "$work/sdk"
WINEPREFIX=${WINEPREFIX:-$HOME/src/ace-sdk} WINEDEBUG=-all \
    wine sql.exe ace32.dll "Z:$(echo "$work/data" | tr / '\\')" ../aeptables.txt |
    sed "s#\"$(date +%m/%d/%Y)\"#\"<today>\"#g"
