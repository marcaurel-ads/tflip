#!/bin/sh
# Builds clipboard.exe and runs it on the native Windows VM (LOCAL.md): it is a
# clipboard probe, so it must run on a real Windows window station, not Wine.
#
#     tools/ace/clipboard/run.sh <ace32.dll Windows path> <data folder> [mode]
#
# It creates a one-field ADS_IMAGE table in <data folder>, stores a known BMP
# (or junk, for the error modes), calls AdsImageToClipboard, and dumps every
# clipboard format present. Modes: bmp (default), junk, empty, bmhdr, bmdib.
#
# Typical use copies the .exe to the VM and runs it there against both tflip's
# and the original's ace32.dll (see docs/KNOWN_ISSUES.md, the clipboard entry):
#
#     i686-w64-mingw32-gcc -O2 -Wall -o clipboard.exe clipboard.c -lgdi32
#     scp -i ~/.ssh/winvm -P 2222 clipboard.exe Docker@127.0.0.1:clipboard.exe
#     ssh -i ~/.ssh/winvm -p 2222 Docker@127.0.0.1 \
#         'cmd /c "cd /d C:\acesdk && C:\Users\Docker\clipboard.exe ace32.dll C:\Users\Docker\clip"'
set -e
here=$(cd "$(dirname "$0")" && pwd)
dll=$1
data=$2
mode=$3
out=${TMPDIR:-/tmp}/clipboard.exe
i686-w64-mingw32-gcc -O2 -Wall -o "$out" "$here/clipboard.c" -lgdi32
WINEDEBUG=-all wine "$out" "$dll" "$data" "$mode"
