#!/bin/sh
# Records what the original's sp_SetDDEncryptionType does to a data dictionary
# of every layout, on the AES-capable engine (see LOCAL.md), under Wine:
#
#     tools/ace/crypto/aesconvert.sh <work folder> <folder of the patched 32-bit DLLs> [<pattern>]
#
# It builds aesdict.exe, then makes the six dictionaries aesconvert/a_*.txt
# describe in <work>/data/<letter> (P plain, R RC4, C AES-128 with its records
# in the clear, E AES-128 and F AES-256 enciphering them, and T a plain one with an
# encrypted table, which a conversion refuses; each with the same
# tables, users, groups, grants, comments, a view, a procedure, a trigger, a
# function and holes left by what was dropped), and converts a copy of each,
# in a process of its own, as each aesconvert/c_*.txt does. The files
# kept are fixtures/aes/convert/:
#
#     before/<letter>/d.{add,ai,am}   a dictionary as it was made
#     after/c_<letter>_<type>_<encrypt>/d.*   what the conversion left of its copy
#
#     cp -r <work>/fixtures/before <work>/fixtures/after fixtures/aes/convert/
#
# With a pattern (m_E_*), only the c_ and m_ scripts whose names match it run, on
# the dictionaries fixtures/aes/convert/before already holds (they are not made
# again, as their random material would change; one it does not hold is), and
# only their `after` is left in <work>/fixtures.
#
# Only the documented API is driven; nothing reads the engine's code.
set -e
if [ $# -lt 2 ] || [ $# -gt 3 ] || [ -z "$1" ]; then
    echo "usage: aesconvert.sh <work folder> <patched DLL folder> [<pattern>]" >&2
    exit 2
fi
here=$(cd "$(dirname "$0")" && pwd)
work=$(mkdir -p "$1" && cd "$1" && pwd)
dlls=$(cd "$2" && pwd)
only=${3:-*}
rm -rf "$work/data" "$work/fixtures"
mkdir -p "$work/data" "$work/fixtures/before" "$work/fixtures/after"
for letter in P R C E F T X Y Z W V; do
    mkdir "$work/data/$letter"
done
i686-w64-mingw32-gcc -O1 -Wall -o "$work/aesdict.exe" "$here/aesdict.c"
win() { echo "Z:$1" | tr / '\\'; }
run() {
    script=$1
    cp "$here/aesconvert/$script.txt" "$work/"
    WINEDEBUG=-all wine "$work/aesdict.exe" "$(win "$dlls/ace32.dll")" "$(win "$work/data")" \
        "$(win "$work/$script.txt")" | tr -d '\r' > "$work/$script.json"
}
cd "$dlls"
for letter in P R C E F T X Y Z W V; do
    mkdir "$work/fixtures/before/$letter"
    if [ $# -eq 3 ] && [ -d "$here/../../../fixtures/aes/convert/before/$letter" ]; then
        cp "$here"/../../../fixtures/aes/convert/before/$letter/d.* "$work/fixtures/before/$letter/"
        continue
    fi
    run "a_$letter"
    for ext in add ai am; do
        cp "$work/data/$letter/$letter.$ext" "$work/fixtures/before/$letter/d.$ext"
    done
done
for path in "$here"/aesconvert/[cm]_*.txt; do
    script=$(basename "$path" .txt)
    # shellcheck disable=SC2254
    case "$script" in $only) ;; *) continue ;; esac
    letter=$(echo "$script" | cut -c3)
    mkdir "$work/data/$script"
    for ext in add ai am; do
        cp "$work/fixtures/before/$letter/d.$ext" "$work/data/$script/"
    done
    run "$script"
    mkdir "$work/fixtures/after/$script"
    cp "$work/data/$script"/d.* "$work/fixtures/after/$script/"
done
