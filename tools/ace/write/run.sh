#!/bin/sh
# Runs a write scenario under Wine: its setup.txt makes the tables in an
# empty folder, which is kept as before/; its ops.txt then writes to them,
# and the folder is kept as after/ with what each command returned in
# result.json.
#
#     tools/ace/write/run.sh <work folder> <ace32.dll as a Windows path> <scenario folder>
#
# With the original engine's ace32.dll this makes the scenario's fixture
# (see fixtures/README.md):
#
#     tools/ace/write/run.sh /tmp/w 'C:\...\acesdk\ace32.dll' fixtures/write/records
#     cp -r /tmp/w/before /tmp/w/after /tmp/w/result.json fixtures/write/records/
#
# The ops run twice, with the stack filled with different bytes before each
# call (see write.c): the bytes of after/ that differ between the two runs are
# ones the engine leaves uninitialised, and go to volatile.json (file name ->
# list of [offset, length]), which the replay does not compare. Copy it into
# the scenario too when it is not empty.
#
# A script's snapshots (`snapshot <name>`) go to snaps/<name> (and
# snaps-again/<name> on the second run); copy snaps/ into the scenario too.
#
# With WRITE_SERVER_DIR and WRITE_SERVER_PATH set, the scenario runs on a
# remote server (the evaluation server under Wine, setup in LOCAL.md) in that
# folder, connected over TCP:
#
#     WRITE_SERVER_DIR=$EVAL_SERVER_PREFIX/drive_c/ads/data/w \
#     WRITE_SERVER_PATH='\\127.0.0.1:6262\data\w' tools/ace/write/run.sh ...
#
# The work folder gets write.exe; data/, before/, after/, again/ and the
# snapshots are rebuilt on every run.
set -e
if [ $# -ne 3 ] || [ -z "$1" ]; then
    echo "usage: run.sh <work folder> <ace32.dll> <scenario folder>" >&2
    exit 2
fi
here=$(cd "$(dirname "$0")" && pwd)
work=$1
dll=$2
scenario=$(cd "$3" && pwd)
mkdir -p "$work"
for d in data before after again snaps snaps-again; do
    if [ -d "$work/$d" ]; then
        find "$work/$d" -mindepth 1 -delete
        rmdir "$work/$d"
    fi
done
# The data folder: in the work folder for the local server; with
# WRITE_SERVER_DIR (a folder a remote server serves) and WRITE_SERVER_PATH
# (its path for clients, \\host:port\share\folder) on that server.
if [ -n "${WRITE_SERVER_DIR:-}" ]; then
    datadir=$WRITE_SERVER_DIR
    rm -rf "$datadir"
    mkdir -p "$datadir"
    data=$WRITE_SERVER_PATH
    export WRITE_CONNECT="Data Source=$data;ServerType=REMOTE;CommType=TCP_IP"
else
    datadir=$work/data
    mkdir "$datadir"
    data="Z:$(echo "$datadir" | tr / '\\')"
fi
export WRITE_FILES="Z:$(echo "$datadir" | tr / '\\')"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/write.exe" "$here/write.c"
iconv -f utf-8 -t cp1252 "$scenario/setup.txt" > "$work/setup.txt"
iconv -f utf-8 -t cp1252 "$scenario/ops.txt" > "$work/ops.txt"
cd "$work"
snaps="Z:$(echo "$work/snaps" | tr / '\\')"
WINEDEBUG=-all wine write.exe "$dll" "$data" setup.txt > setup.json
cp -r "$datadir" before
WRITE_SNAPS=$snaps WRITE_STACK_FILL=0x5A WINEDEBUG=-all wine write.exe "$dll" "$data" ops.txt | tr -d '\r' > result.json
cp -r "$datadir" after
find "$datadir" -mindepth 1 -delete
cp -r before/. "$datadir"/
WRITE_SNAPS=$snaps-again WRITE_STACK_FILL=0xA5 WINEDEBUG=-all wine write.exe "$dll" "$data" ops.txt | tr -d '\r' > again.json
cp -r "$datadir" again
python3 "$here/volatile.py" after again > volatile.json
# The snapshots' bytes that differ between the two runs, keyed
# <snapshot>/<file>; copy snaps-volatile.json into the scenario too.
if [ -d snaps ]; then
    python3 "$here/volatile.py" --snaps snaps snaps-again > snaps-volatile.json
fi
