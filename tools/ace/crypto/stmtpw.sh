#!/bin/sh
# Records what the original does with the table passwords of a statement and
# its AdsStmtEnableEncryption, on the AES-capable engine's folder (see
# LOCAL.md; the RC4 tables need no OpenSSL, but the folder holds the DLLs),
# under Wine:
#
#     tools/ace/crypto/stmtpw.sh <work folder> <folder of the 32-bit DLLs>
#
# It builds stmtpw.exe and runs stmtpw.txt on <work>/data; <work>/stmtpw.json
# is what the steps returned and <work>/data holds the tables it left:
#
#     cp <work>/stmtpw.json fixtures/write/encryption/
#
# Only the documented API is driven; nothing reads the engine's code.
set -e
if [ $# -ne 2 ] || [ -z "$1" ]; then
    echo "usage: stmtpw.sh <work folder> <DLL folder>" >&2
    exit 2
fi
here=$(cd "$(dirname "$0")" && pwd)
work=$(mkdir -p "$1" && cd "$1" && pwd)
dlls=$(cd "$2" && pwd)
rm -rf "$work/data"
mkdir -p "$work/data"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/stmtpw.exe" "$here/stmtpw.c"
win() { echo "Z:$1" | tr / '\\'; }
cd "$dlls"
WINEDEBUG=-all wine "$work/stmtpw.exe" "$(win "$dlls/ace32.dll")" "$(win "$work/data")" \
    "$(win "$here/stmtpw.txt")" | tr -d '\r' > "$work/stmtpw.json"
