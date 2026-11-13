#!/bin/sh
# Builds and runs the encryption probes under Wine, then summarises them.
#
#     tools/ace/crypto/run.sh <work folder> <ace32.dll as a Windows path>
#
# Example (substitute your Wine prefix and the Windows path to the SDK DLL):
#
#     WINEPREFIX=<wine prefix> tools/ace/crypto/run.sh /tmp/c 'Z:\path\to\ace32.dll'
#
# It writes crypto.json and dict.json in the work folder and prints what they
# show (analyze.py). Nothing here reads the engine's code; the probes only
# drive the documented API and read the files the engine writes.
set -e
if [ $# -ne 2 ] || [ -z "$1" ]; then
    echo "usage: run.sh <work folder> <ace32.dll>" >&2
    exit 2
fi
here=$(cd "$(dirname "$0")" && pwd)
work=$1
dll=$2
mkdir -p "$work"
rm -rf "$work/data"
mkdir -p "$work/data"
data="Z:$(echo "$work/data" | tr / '\\')"

i686-w64-mingw32-gcc -O1 -Wall -o "$work/crypto.exe" "$here/crypto.c"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/dict.exe" "$here/dict.c"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/stream.exe" "$here/stream.c"
cd "$work"

# Table encryption: the same scenario under several passwords. Same-length
# pairs (A/B, AAAAAAAA/aaaaaaaa, password/passwore) show how the key depends on
# the password; different lengths show whether the key is padded or hashed.
run_crypto() {
    WINEDEBUG=-all wine crypto.exe "$dll" "$data" "$2" "$1" | tr -d '\r'
}
{
    printf '[\n'
    first=1
    for pair in \
        e1:A e2:B e3:AAAA e4:AAAAAAAA e5:aaaaaaaa \
        e6:AAAAAAAAB e7:password e8:passwore e9:0000000000000000; do
        tag=${pair%%:*}
        pw=${pair#*:}
        [ $first -eq 1 ] || printf ',\n'
        first=0
        run_crypto "$tag" "$pw"
    done
    printf '\n]\n'
} > crypto.json

# Table item structure: same password, so keystreams are comparable across runs.
# s1 vs s2 change the record WIDTH (1 vs 2 fields); s3 appends many records to
# scan the record-number -> keystream map for a period. run_stream <tag> <pw>
# <fields> <records>.
run_stream() {
    WINEDEBUG=-all wine stream.exe "$dll" "$data" "$2" "$3" "$4" "$1" | tr -d '\r'
}
{
    printf '[\n'
    run_stream s1 Structure01 1 64; printf ',\n'
    run_stream s2 Structure01 2 64; printf ',\n'
    run_stream s3 Structure01 2 300
    printf '\n]\n'
} > stream.json

# Dictionary properties: where each encrypted byte's variation comes from.
# a,b: same passwords in the same second (random-salt check). c: admin password
# changed. d: user password changed. e,f: same as a but created seconds later
# (so a,e,f isolate creation-time variation). A byte that never varies across
# a,c,d,e,f is encrypted with a fixed keystream -- recoverable with known
# plaintext, no secret needed; a byte that varies only with time is knowable;
# only a byte that varies with neither password nor time (a vs b) would point to
# a random, binary-resident secret.
run_dict() {
    WINEDEBUG=-all wine dict.exe "$dll" "$data" "$2" "$3" "$1" | tr -d '\r'
}
{
    printf '[\n'
    run_dict a SecretPass12 UserPass34; printf ',\n'
    run_dict b SecretPass12 UserPass34; printf ',\n'
    run_dict c XecretPass12 UserPass34; printf ',\n'
    run_dict d SecretPass12 VserPass34; printf ',\n'
    sleep 2; run_dict e SecretPass12 UserPass34; printf ',\n'
    sleep 2; run_dict f SecretPass12 UserPass34
    printf '\n]\n'
} > dict.json

python3 "$here/analyze.py" crypto.json dict.json stream.json
