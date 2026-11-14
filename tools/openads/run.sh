#!/bin/sh
# Runs OpenADS's ACE-level unit tests against tflip's libtflip_ace.so:
#
#     tools/openads/run.sh <OpenADS checkout> <work folder>
#
# OpenADS (https://github.com/FiveTechSoft/OpenADS) is another open
# replacement for the Advantage Client Engine. Its doctest unit tests in
# tests/unit/ that talk to the engine through the ACE API alone (the list is
# tests.txt) are built against tflip's client library instead of OpenADS's,
# one doctest binary, and each file runs in a process of its own, so that a
# crash takes only that file down. Writes to the work folder:
#
#     results.txt   each test file and its exit code (0 passed, 1 failed)
#     logs/         doctest's output for each file
#     summary.txt   each failing file's first failed check and its source line
#     constants.txt the constants rewritten to SAP's values (5. below)
#
# The tests encode what OpenADS takes the engine to do. Where that differs
# from what the original does, as tflip's fixtures recorded it, the tests
# are edited in a copy, <OpenADS checkout>/tests/unit_tflip (the tests find
# their fixtures relative to their own path, so the copy sits next to
# tests/unit; it is removed and remade on each run):
#
#   1. AdsGetRecordCount(h, 0, ...) gets ADS_RESPECTFILTERS: the original
#      rejects option 0 with 5013 (fixtures/errors/record_count.json).
#   2. FROM/JOIN/INTO/UPDATE/TABLE name.dbf (or .adt) is quoted: unquoted,
#      the original reads link.table, 5041 on a free connection
#      (fixtures/sql/links.json, docs/ADS_BUGS.md).
#   3. In the files whose SQL names .dbf tables, AdsStmtSetTableType(stmt,
#      ADS_CDX) follows AdsCreateSQLStatement: a statement's table type is
#      ADS_ADT until set.
#   4. ADSHANDLE is 64 bits, as in ace.h under x64; OpenADS's ace.h makes it
#      32 bits everywhere, and tflip's 8-byte handles then overwrite the
#      tests' stack and heap. The tests build against a copy of the header.
#   5. Constants: OpenADS's ace.h and error.h give about 70 of the constants
#      they share with ace.h other values (ADS_IGNOREFILTERS 3, ADS_STRINGKEY
#      1, AE_LOCK_FAILED 5013, ...); the copies get ace.h's values
#      (sapconst.py). ace.h is $SAP_ACE_H, by default the SDK's in the Wine
#      prefix ~/src/ace-sdk.
#
# Literal zeros where ace.h defines no 0 (a seek type, an AOF resolve
# option) stay as they are. Needs g++ (C++20), python3 and cargo; the
# collation files come from TFLIP_COLLATION_DIR or the SDK, as for tflip's
# own tests.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
O=$(cd "$1" && pwd)
mkdir -p "$2"
W=$(cd "$2" && pwd)
sdk="$HOME/src/ace-sdk/drive_c/Program Files/Advantage 12.0/acesdk"
SAP_ACE_H=${SAP_ACE_H:-$sdk/ace.h}
[ -f "$SAP_ACE_H" ] || { echo "no ace.h at $SAP_ACE_H (set SAP_ACE_H)" >&2; exit 1; }
if [ -z "$TFLIP_COLLATION_DIR" ] && [ -f "$sdk/ansi.chr" ]; then
    export TFLIP_COLLATION_DIR="$sdk"
fi

(cd "$root" && cargo build --release -p tflip-ace)
L=$root/target/release

list=$W/tests.txt
grep -v '^#' "$here/tests.txt" | grep . > "$list"
P=$O/tests/unit_tflip
rm -rf "$P" "$W/obj" "$W/logs" "$W/cwd" "$W/include"
mkdir -p "$P" "$W/obj" "$W/logs" "$W/cwd" "$W/include"
cp "$O"/tests/unit/*.cpp "$O"/tests/unit/*.h "$P"/
for f in $(cat "$list"); do
    sed -i -E \
        -e 's/AdsGetRecordCount\(([^,()]+), *0 *,/AdsGetRecordCount(\1, ADS_RESPECTFILTERS,/g' \
        -e 's/\b(FROM|JOIN|INTO|UPDATE|TABLE) ([A-Za-z_][A-Za-z0-9_]*)\.(dbf|adt|DBF|ADT)\b/\1 \\"\2.\3\\"/g' \
        "$P/$f"
    if grep -q '\.dbf\\"' "$P/$f"; then
        sed -i -E 's/^([[:space:]]*)(REQUIRE|CHECK)\(AdsCreateSQLStatement\([^,]+, *&([A-Za-z_][A-Za-z0-9_]*)\) *== *(0|AE_SUCCESS)\);/&\n\1AdsStmtSetTableType(\3, ADS_CDX);/' "$P/$f"
    fi
done

cp -r "$O/include/openads" "$W/include/"
sed -i 's/^typedef uint32_t ADSHANDLE;/typedef uint64_t ADSHANDLE;/' "$W/include/openads/ace.h"
grep -q '^typedef uint64_t ADSHANDLE;' "$W/include/openads/ace.h"
python3 -I "$here/sapconst.py" "$SAP_ACE_H" \
    "$W/include/openads/ace.h" "$W/include/openads/error.h" > "$W/constants.txt"

cd "$W"
for f in doctest_main.cpp $(cat "$list"); do
    g++ -std=c++20 -O1 -w -c -I"$W/include" -I"$O/include" -I"$O/tests" \
        -I"$O/third_party/doctest" -I"$O/src" "$P/$f" -o "obj/${f%.cpp}.o"
done
g++ -o xtest obj/*.o -L"$L" -ltflip_ace -Wl,-rpath,"$L" -lpthread

for f in $(cat "$list"); do
    n=${f%.cpp}
    rc=0
    (cd cwd && timeout 120 ../xtest -sf="*/$n.cpp" -nv > "../logs/$n.log" 2>&1) || rc=$?
    echo "$n $rc"
done > results.txt

for f in logs/*.log; do
    loc=$(grep -m1 'ERROR' "$f" | grep -o '[a-z_0-9]*\.cpp:[0-9]*') || continue
    file=${loc%%:*}
    line=${loc##*:}
    printf '%s  %s  %s\n' "$loc" \
        "$(grep -m1 -A1 'ERROR' "$f" | sed -n 's/.*values: //p')" \
        "$(sed -n "${line}p" "$P/$file" | sed 's/^ *//')"
done > summary.txt

awk '{print $2}' results.txt | sort | uniq -c |
    awk '{print $2 == 0 ? "passed" : $2 == 1 ? "failed" : "crashed (" $2 ")", $1}'
cat logs/*.log | sed -n 's/^\[doctest\] test cases: *\([0-9]*\) *| *\([0-9]*\) passed | *\([0-9]*\) failed.*/\2 \3/p' |
    awk '{p += $1; f += $2} END {print "test cases: " p " passed, " f " failed"}'
