#!/bin/sh
# Runs tflip's probes against another client engine's 64-bit DLL, under
# Wine, and compares its answers with the original's:
#
#     tools/openads/probes.sh <DLL, a Unix path> <work folder> [<probe>...]
#
# Made for OpenADS's openace64.dll (README.md says how to build it), but
# takes any ace64-like DLL. The probes are the ones in probes.txt (all of
# them, or those named). Each is built for x86_64 rather than i686: a
# compiler shim named i686-w64-mingw32-gcc on PATH runs
# x86_64-w64-mingw32-gcc with probe64.h included, so the probe scripts run
# unchanged. Each probe runs twice, against the DLL and against the
# original's ace64.dll ($SAP_ACE64, a Windows path in $WINEPREFIX, by
# default the SDK's in ~/src/ace-sdk). The original's answers are kept in
# <work folder>/sap64/ and reused; delete them to record again. The x64
# original is the reference: its answers are the golden's but for a few
# lines (the golden comes from ace32.dll), counted in the "x64" column.
#
# Writes, per probe, <work folder>/dll/<probe>.json, the diff against the
# original's in diffs/<probe>.diff, the Ads functions its sources look up
# that the DLL does not export in missing/<probe>.txt (probe64.h makes
# those answer 99999), and prints a table:
#
#     probe  golden lines  x64  same  differ  missing
#
# "x64": lines in which the x64 original differs from the golden; "same":
# lines of the original's answers the DLL gives too; "differ": lines in
# which they differ (diff's < and > lines); "missing": functions not
# exported. A probe that crashes or times out (600 s) leaves what it printed.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
dll=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
mkdir -p "$2"
W=$(cd "$2" && pwd)
shift 2
export WINEPREFIX=${WINEPREFIX:-$HOME/src/ace-sdk}
export WINEDLLOVERRIDES="winedbg.exe=d${WINEDLLOVERRIDES:+;$WINEDLLOVERRIDES}"
SAP_ACE64=${SAP_ACE64:-'C:\Program Files\Advantage 12.0\acesdk\ace64.dll'}

mkdir -p "$W/shim" "$W/sap64" "$W/dll" "$W/diffs" "$W/missing" "$W/logs" "$W/runs"
printf '#!/bin/sh\nexec x86_64-w64-mingw32-gcc -include "%s" "$@"\n' "$here/probe64.h" \
    > "$W/shim/i686-w64-mingw32-gcc"
chmod +x "$W/shim/i686-w64-mingw32-gcc"
x86_64-w64-mingw32-objdump -p "$dll" | sed -n '/Ordinal\/Name Pointer/,/^$/p' |
    awk '{print $NF}' | grep '^Ads' | sort -u > "$W/exports.txt"
windll=$(winepath -w "$dll" 2>/dev/null)

# <probe> <script> <DLL as a Windows path> <output> [args...]
run() {
    name=$1 script=$2 target=$3 out=$4
    shift 4
    rm -rf "$W/runs/$name"
    PATH="$W/shim:$PATH" timeout 600 sh "$root/tools/ace/$script" \
        "$W/runs/$name" "$target" "$@" > "$out" 2> "$W/logs/$name.$(basename "$(dirname "$out")").err" || true
    wineserver -k 2>/dev/null || true
}

printf '%-16s %8s %6s %8s %7s %8s\n' probe golden x64 same differ missing
grep -v '^#' "$here/probes.txt" | grep . | while read -r name script golden args; do
    if [ $# -gt 0 ]; then
        case " $* " in *" $name "*) ;; *) continue ;; esac
    fi
    gold=$root/fixtures/$golden
    [ -s "$W/sap64/$name.json" ] || run "$name" "$script" "$SAP_ACE64" "$W/sap64/$name.json" $args
    run "$name" "$script" "$windll" "$W/dll/$name.json" $args
    cat "$root/tools/ace/$(dirname "$script")"/*.c 2>/dev/null |
        grep -o '"Ads[A-Za-z0-9_]*"' | tr -d '"' | sort -u |
        comm -23 - "$W/exports.txt" > "$W/missing/$name.txt"
    diff "$W/sap64/$name.json" "$W/dll/$name.json" > "$W/diffs/$name.diff" || true
    x64=$(diff "$gold" "$W/sap64/$name.json" | grep -c '^[<>]' || true)
    total=$(grep -c "" "$W/sap64/$name.json" || true)
    gone=$(grep -c '^<' "$W/diffs/$name.diff" || true)
    differ=$(grep -c '^[<>]' "$W/diffs/$name.diff" || true)
    printf '%-16s %8s %6s %8s %7s %8s\n' "$name" "$(grep -c "" "$gold")" "$x64" \
        "$((total - gone))/$total" "$differ" "$(wc -l < "$W/missing/$name.txt")"
done
