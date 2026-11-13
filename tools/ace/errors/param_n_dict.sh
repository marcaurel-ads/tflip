#!/bin/bash
# Makes fixtures/errors/param_n/indexed_table.{add,ai,am}: the dictionary
# fixtures/dictionary/new/indexed_table with one object of each kind it has
# none of, added by param_n_dict.exe under Wine.
#
#     tools/ace/errors/param_n_dict.sh <work folder> <ace32.dll as a Windows path>
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
work=$1
dll=$2
new=$root/fixtures/dictionary/new
rm -rf "$work/dict"
mkdir -p "$work/dict"
# The view, the trigger and the article open table t, whose index file is it.adi's.
cp "$new"/indexed_table.{add,ai,am} "$new"/t.adt "$work/dict/"
cp "$new/it.adi" "$work/dict/t.adi"
# The link's target must be a dictionary.
cp "$new/empty.add" "$work/dict/other.add"
cp "$new/empty.ai" "$work/dict/other.ai"
cp "$new/empty.am" "$work/dict/other.am"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/param_n_dict.exe" "$here/param_n_dict.c"
cd "$work"
WINEDEBUG=-all wine param_n_dict.exe "$dll" "$(winepath -w "$work/dict/indexed_table.add")" 2>&1 | grep -v fixme || true
mkdir -p "$root/fixtures/errors/param_n"
cp dict/indexed_table.{add,ai,am} "$root/fixtures/errors/param_n/"
