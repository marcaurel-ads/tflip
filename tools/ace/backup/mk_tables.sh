#!/bin/sh
# Prints the ev.c script lines that make one table in connection 1 (a
# dictionary) or 0 (free tables) with fixed contents:
#
#     mk_tables.sh <connection> <table> <ev.c table type, 0 = default> <memo text length>
#
# Ten rows, rows 3 and 6 deleted, row 2's memo grown afterwards so the memo
# file holds a dead block; two orders on the name and on the date.
c=$1 t=$2 ty=$3 ml=$4
memo=$(printf 'm%.0s' $(seq 1 "$ml"))
echo "ttype $ty"
echo "sql $c CREATE TABLE $t (id INTEGER, name CHAR(20), qty NUMERIC(10,2), d DATE, flag LOGICAL, note MEMO)"
echo "ttype 0"
i=1
while [ $i -le 10 ]; do
    echo "sql $c INSERT INTO $t VALUES ($i, 'name$((11 - i))', $i.25, '2020-01-0$((i % 9 + 1))', $((i % 2)), 'memo $i $memo')"
    i=$((i + 1))
done
echo "sql $c CREATE INDEX ${t}_name ON $t (name)"
echo "sql $c CREATE INDEX ${t}_d ON $t (d)"
echo "sql $c DELETE FROM $t WHERE id IN (3, 6)"
echo "sql $c UPDATE $t SET note = 'grown $memo$memo$memo$memo' WHERE id = 2"
