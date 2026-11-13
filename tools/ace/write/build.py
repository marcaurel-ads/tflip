#!/usr/bin/env python3
"""Writes fixtures/write/build/setup.txt and ops.txt: tables of 1 to 3,000
records made by the original engine, then orders of many kinds built on
them (CREATE INDEX and sp_CreateIndex90 on tables that have records).

    tools/ace/write/build.py fixtures/write/build
"""
import random
import sys

out = sys.argv[1]
rng = random.Random(7)
words = ["alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel",
         "india", "juliet", "kilo", "lima", "mike", "november", "oscar", "papa"]

setup = ["# Tables of 1 to 3,000 records, some deleted."]
for name, n in [("b1", 1), ("b10", 10), ("b120", 120), ("b500", 500), ("b3k", 3000)]:
    setup.append(f"sql CREATE TABLE {name} (id INTEGER, word CHAR(12), grp SHORT, "
                 "amount NUMERIC(8, 2), d DATE, l LOGICAL, m MEMO)")
    rows = []
    for i in range(1, n + 1):
        word = rng.choice(words) + str(rng.randrange(1000))
        grp = rng.randrange(-5, 20)
        amount = f"{rng.randrange(-100000, 100000) / 100:.2f}"
        day = f"20{rng.randrange(0, 30):02d}-{rng.randrange(1, 13):02d}-{rng.randrange(1, 29):02d}"
        flag = rng.choice(["TRUE", "FALSE"])
        # Some NULLs.
        if i % 37 == 0:
            rows.append(f"({i}, NULL, NULL, NULL, NULL, NULL, NULL)")
        else:
            rows.append(f"({i}, '{word}', {grp}, {amount}, '{day}', {flag}, 'm{i}')")
    for at in range(0, len(rows), 100):
        setup.append(f"sql INSERT INTO {name} VALUES " + ", ".join(rows[at:at + 100]))
setup.append("sql DELETE FROM b500 WHERE id % 7 = 3")
setup.append("sql DELETE FROM b3k WHERE id % 11 = 5")

ops = ["# Orders built on tables with records."]
for name in ["b1", "b10", "b120", "b500", "b3k"]:
    ops += [
        f"sql CREATE INDEX id ON {name} (id)",
        f"sql CREATE INDEX word ON {name} (word)",
        f"sql CREATE INDEX grp_desc ON {name} (grp DESC)",
        f"sql CREATE INDEX word_id ON {name} (word, id)",
        f"sql CREATE UNIQUE INDEX uid ON {name} (id)",
        f"sql CREATE INDEX amount ON {name} (amount)",
        f"sql CREATE INDEX d ON {name} (d)",
        f"sql CREATE INDEX l ON {name} (l)",
        f"sql EXECUTE PROCEDURE sp_CreateIndex90 ('{name}', '{name}', 'up', 'UPPER(word)', NULL, 2, 512, NULL)",
        f"sql EXECUTE PROCEDURE sp_CreateIndex90 ('{name}', '{name}', 'cond', 'amount', 'grp > 3', 2, 512, NULL)",
        f"sql EXECUTE PROCEDURE sp_CreateIndex90 ('{name}', '{name}', 'cust', 'word', NULL, 6, 512, NULL)",
        f"sql EXECUTE PROCEDURE sp_CreateIndex90 ('{name}', '{name}', 'del', 'DELETED()', NULL, 4098, 512, NULL)",
        f"sql EXECUTE PROCEDURE sp_CreateIndex90 ('{name}', 'k{name}', 'w1k', 'word', NULL, 2, 1024, NULL)",
        f"sql EXECUTE PROCEDURE sp_CreateIndex90 ('{name}', 'e{name}', 'w8k', 'word', NULL, 2, 8192, NULL)",
    ]
# A unique order refused by duplicate keys.
ops.append("sql CREATE UNIQUE INDEX ugrp ON b500 (grp)")
# Records added to built orders.
ops.append("sql INSERT INTO b3k (id, word, grp, amount, d, l) VALUES (5000, 'zulu', 3, 1.5, '2020-01-01', TRUE)")
ops.append("sql INSERT INTO b120 (id, word, grp, amount, d, l) VALUES (5000, 'aaa', 3, 1.5, '2020-01-01', TRUE)")

with open(f"{out}/setup.txt", "w") as f:
    f.write("\n".join(setup) + "\n")
with open(f"{out}/ops.txt", "w") as f:
    f.write("\n".join(ops) + "\n")
