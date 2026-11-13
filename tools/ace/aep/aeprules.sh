#!/bin/sh
# Records aeprules.txt on the original's local server, under Wine:
#
#     tools/ace/aep/aeprules.sh > fixtures/sql/aeprules.json
#
# The work folder ($WORK) has data/ae/ holding aeprules.c built as a 32-bit
# DLL that links the SDK's ace32.dll, the one sql.exe loads.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
sdk=${WINEPREFIX:-$HOME/src/ace-sdk}
dll='C:\Program Files (x86)\Advantage 12.0\acesdk\ace32.dll'
work=${WORK:-${TMPDIR:-/tmp}/aeprules-work}
PATH="$PATH:/usr/lib/mingw64-toolchain/bin"
rm -rf "$work"
mkdir -p "$work/data/ae"
i686-w64-mingw32-gcc -O1 -Wall -shared -Wl,--kill-at -o "$work/data/ae/aeprules.aep" "$here/aeprules.c" "$sdk/drive_c/Program Files (x86)/Advantage 12.0/acesdk/ace32.dll"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/sql.exe" "$root/tools/ace/sql/sql.c"
iconv -f utf-8 -t cp1252 "$here/aeprules.txt" > "$work/aeprules.txt"
cd "$work"
WINEPREFIX=$sdk WINEDEBUG=-all wine sql.exe "$dll" "Z:$(echo "$work/data" | tr / '\\')" aeprules.txt
