#!/bin/sh
# Runs a write scenario (see run.sh) under a static collation: the engine
# reads ANSI_CHAR_SET from the adslocal.cfg in its own folder, so the run
# uses a copy of the acesdk folder (in the work folder) whose adslocal.cfg
# has the ANSI_CHAR_SET given; the original folder is left as it is. The
# scenario's ops.txt says so in a `# ANSI_CHAR_SET=<name>` line, for the
# replay.
#
#     tools/ace/write/run_static.sh <work folder> <acesdk folder, a Unix path> <scenario folder> <ANSI_CHAR_SET>
#
# For fixtures/write/aofs_static (and aofs_static_keys):
#
#     tools/ace/write/run_static.sh /tmp/w ".../Advantage 12.0/acesdk" fixtures/write/aofs_static German
set -e
if [ $# -ne 4 ]; then
    echo "usage: run_static.sh <work folder> <acesdk folder> <scenario folder> <ANSI_CHAR_SET>" >&2
    exit 2
fi
here=$(cd "$(dirname "$0")" && pwd)
work=$1
acesdk=$2
scenario=$3
ansi=$4
mkdir -p "$work"
rm -rf "$work/acesdk"
cp -r "$acesdk" "$work/acesdk"
sed "s/^ANSI_CHAR_SET=.*/ANSI_CHAR_SET=$ansi/" "$acesdk/adslocal.cfg" > "$work/acesdk/adslocal.cfg"
dll="Z:$(cd "$work/acesdk" && pwd | tr / '\\')\\ace32.dll"
"$here/run.sh" "$work" "$dll" "$scenario"
