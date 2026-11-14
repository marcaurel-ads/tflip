#!/bin/sh
# Runs OpenADS's ACE-level unit tests against tflip's libtflip_ace.so:
#
#     tools/openads/run.sh [--sap] <OpenADS checkout> <work folder>
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
# With --sap, the same tests are also built for Windows (x86_64-w64-mingw32-g++,
# linked against the original's ace64.dll from the SDK in $WINEPREFIX, by
# default ~/src/ace-sdk) and run under Wine, so that each failure can be
# checked against the original. Also writes:
#
#     sap/results.txt, sap/logs/   the same, from the original
#     compare.txt   per file, the checks that fail on one engine only or with
#                   other values on each (compare.py says how to read it)
#
# The tests encode what OpenADS takes the engine to do. Where that differs
# from what the original does, as tflip's fixtures recorded it, the tests
# are edited in a copy, <OpenADS checkout>/tests/unit_tflip (the tests find
# their fixtures relative to their own path, so the copy sits next to
# tests/unit; it is removed and remade on each run):
#
#   1. Literal zeros where ace.h defines no 0 (filter options, seek and key
#      data types, the AOF resolve option, character types, rights options)
#      become ace.h's value with the meaning OpenADS gives 0. Not an edit of
#      the sources but wrappers linked in, zeros.cpp. The original rejects
#      them (filter option 0 with 5013: fixtures/errors/record_count.json).
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
#   6. The DBF headers the tests write themselves (hdr[0] = 0x03, ...) get
#      the date 2024-01-01: the tests leave it 00/00/00, and the original
#      opens no DBF table whose month or day is 0 (7016,
#      fixtures/dbf/header_dates.json), nor does tflip since.
#
# Needs g++ (C++20), python3 and cargo, and for --sap MinGW-w64 and Wine; the
# collation files come from TFLIP_COLLATION_DIR or the SDK, as for tflip's
# own tests.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
sap=
if [ "$1" = --sap ]; then sap=1; shift; fi
O=$(cd "$1" && pwd)
mkdir -p "$2"
W=$(cd "$2" && pwd)
export WINEPREFIX=${WINEPREFIX:-$HOME/src/ace-sdk}
sdk="$WINEPREFIX/drive_c/Program Files/Advantage 12.0/acesdk"
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
rm -rf "$P" "$W/obj" "$W/logs" "$W/cwd" "$W/include" "$W/sap" "$W/compare.txt"
mkdir -p "$P" "$W/obj" "$W/logs" "$W/cwd" "$W/include"
cp "$O"/tests/unit/*.cpp "$O"/tests/unit/*.h "$P"/
for f in $(cat "$list"); do
    sed -i -E \
        -e 's/\b(FROM|JOIN|INTO|UPDATE|TABLE) ([A-Za-z_][A-Za-z0-9_]*)\.(dbf|adt|DBF|ADT)\b/\1 \\"\2.\3\\"/g' \
        -e 's/^([[:space:]]*hdr\[0\] *= *0x(03|30|31|32);)/\1 hdr[1] = 124; hdr[2] = 1; hdr[3] = 1;/' \
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
wrap=-Wl$(sed -n 's/^U32 __wrap_\([A-Za-z]*\).*/,--wrap=\1/p; s/^FILTER_OPTION(\(.*\))$/,--wrap=\1/p' \
    "$here/zeros.cpp" | tr -d '\n')
g++ -std=c++20 -O1 -c "$here/zeros.cpp" -o zeros.o
g++ -o xtest obj/*.o zeros.o -L"$L" -ltflip_ace -Wl,-rpath,"$L" -lpthread "$wrap"

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

if [ -n "$sap" ]; then
    mkdir -p sap/obj sap/logs sap/cwd
    for f in doctest_main.cpp $(cat "$list"); do
        echo "$f"
    done | xargs -P "$(nproc)" -I{} sh -c 'x86_64-w64-mingw32-g++ -std=c++20 -O1 -w -c \
        -I"$1/include" -I"$2/include" -I"$2/tests" -I"$2/third_party/doctest" -I"$2/src" \
        "$3/{}" -o "sap/obj/$(basename {} .cpp).o"' sh "$W" "$O" "$P"
    x86_64-w64-mingw32-g++ -std=c++20 -O1 -c "$here/zeros.cpp" -o sap/zeros.o
    x86_64-w64-mingw32-g++ -std=c++20 -O1 -c "$here/sapcwd.cpp" -o sap/sapcwd.o
    cp "$sdk"/*64.dll sap/
    x86_64-w64-mingw32-g++ -static -o sap/xtest.exe sap/obj/*.o sap/zeros.o sap/sapcwd.o \
        sap/ace64.dll "$wrap",--wrap=AdsConnect60
    for f in $(cat "$list"); do
        n=${f%.cpp}
        rc=0
        (cd sap/cwd && WINEDEBUG=-all timeout 300 wine ../xtest.exe -sf="*$n.cpp" -nv \
            > "../logs/$n.log" 2>&1) || rc=$?
        echo "$n $rc"
    done > sap/results.txt
    wineserver -k 2>/dev/null || true
    python3 -I "$here/compare.py" "$W" > compare.txt
fi

awk '{print $2}' results.txt | sort | uniq -c |
    awk '{print $2 == 0 ? "passed" : $2 == 1 ? "failed" : "crashed (" $2 ")", $1}'
cat logs/*.log | sed -n 's/^\[doctest\] test cases: *\([0-9]*\) *| *\([0-9]*\) passed | *\([0-9]*\) failed.*/\2 \3/p' |
    awk '{p += $1; f += $2} END {print "test cases: " p " passed, " f " failed"}'
if [ -n "$sap" ]; then
    echo "the original:"
    awk '{print $2}' sap/results.txt | sort | uniq -c |
        awk '{print "  " ($2 == 0 ? "passed" : $2 == 1 ? "failed" : "crashed (" $2 ")"), $1}'
    tail -1 compare.txt
fi
