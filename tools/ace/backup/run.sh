#!/bin/bash
# Runs a script of this folder through the probe of tools/ace/events/ev.c on
# the ADS 12 evaluation server (under Wine, setup in LOCAL.md) and records what it
# answers and every file it leaves in the scenario's folder:
#
#     EVAL_SERVER_PREFIX=<prefix> tools/ace/backup/run.sh <scenario> [local]
#
# The scenario's script is scripts/<scenario>.txt. "@ROOT@" in it is the
# scenario's folder as the server sees it (C:\ads\data\bk\<scenario>), so no
# path may have a component that starts with r, n or t (ev.c reads \r, \n, \t
# there). "@CLIENT@" is the same folder as the client's Wine prefix names it
# (for the "shell" lines that copy files between two statements) and "@UNC@"
# the folder as a remote client names it (\\127.0.0.1:6262\data\bk\<scenario>).
# The answers go to fixtures/backup/<scenario>/result.json, the rows the
# server added to its error log to errlog.txt and the folder to tree.
# With "local" the script runs on the local server (the folder is then under
# data/bk_local of the evaluation server's prefix, which the Wine prefix of
# the SDK reaches as a Z: path) and the fixture is <scenario>_local.
# Lines "#include <file>" of a script are replaced by that file of scripts/;
# "#table" lines are replaced by the output of mk_tables.sh with the same
# arguments.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
pfx=${EVAL_SERVER_PREFIX:?set EVAL_SERVER_PREFIX to the Wine prefix of the evaluation server (LOCAL.md)}
export WINEPREFIX=${WINEPREFIX:-$HOME/src/ace-sdk}
dll='C:\Program Files (x86)\Advantage 12.0\acesdk\ace32.dll'
scen=$1
work=${BK_WORK:-${TMPDIR:-/tmp}/tflip-backup-probe}
if [ "$2" = local ]; then
    data=$pfx/drive_c/ads/data/bk_local/$scen
    out=$root/fixtures/backup/${scen}_local
    folder="Z:${data//\//\\}"
else
    data=$pfx/drive_c/ads/data/bk/$scen
    out=$root/fixtures/backup/$scen
    folder="C:\\ads\\data\\bk\\$scen"
fi
mkdir -p "$work"
awk -v dir="$here" '
    /^#include / { while ((getline l < (dir "/scripts/" $2)) > 0) print l; next }
    { print }' "$here/scripts/$scen.txt" |
folder="$folder" unc="\\\\127.0.0.1:6262\\data\\bk\\$scen" clientfolder="Z:${data//\//\\}" awk -v dir="$here" '
    BEGIN { folder = ENVIRON["folder"]; clientfolder = ENVIRON["clientfolder"]; unc = ENVIRON["unc"] }
    function repl(s, a, b,    r, i) {
        r = ""
        while ((i = index(s, a)) > 0) { r = r substr(s, 1, i - 1) b; s = substr(s, i + length(a)) }
        return r s
    }
    /^#table / { cmd = dir "/mk_tables.sh " $2 " " $3 " " $4 " " $5; while ((cmd | getline l) > 0) print l; close(cmd); next }
    { print repl(repl(repl($0, "@ROOT@", folder), "@CLIENT@", clientfolder), "@UNC@", unc) }' > "$work/$scen.txt"
rm -rf "$data"
mkdir -p "$data"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/ev.exe" "$root/tools/ace/events/ev.c"
iconv -f utf-8 -t cp1252 "$work/$scen.txt" > "$work/$scen.cp"
cd "$work"
# The server logs what it did to its own ads_err.adt.
srvlog=$pfx/drive_c/ads_err.adt
errdump() {
    rm -rf "$work/errlog"
    mkdir -p "$work/errlog"
    cp "$srvlog" "${srvlog%.adt}.adm" "$work/errlog/"
    sh "$here/dump.sh" "Z:${work//\//\\}\\errlog\\ads_err.adt" adt > "$work/errlog/dump.json"
}
if [ "$2" = local ]; then
    EV_SERVER_TYPE=1 WINEDEBUG=-all wine ev.exe "$dll" "$folder" "$scen.cp" > "$scen.json"
else
    errdump
    before=$(python3 "$here/errlog.py" "$work/errlog/dump.json")
    WINEDEBUG=-all wine ev.exe "$dll" "\\\\127.0.0.1:6262\\data\\bk\\$scen" "$scen.cp" > "$scen.json"
fi
rm -rf "$out"
mkdir -p "$out"
cp "$work/$scen.json" "$out/result.json"
if [ "$2" != local ]; then
    errdump
    python3 "$here/errlog.py" "$work/errlog/dump.json" "$before" > "$out/errlog.txt"
fi
cp -a "$data" "$out/tree"
