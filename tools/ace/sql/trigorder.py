#!/usr/bin/env python3
"""Writes trigorder.txt, the script of fixtures/sql/trigorder.json:

    python3 trigorder.py > trigorder.txt
"""
# writes trigorder.txt
out = []
w = out.append
w("# The order triggers of one timing and priority fire in, for creation orders,")
w("# names, drops and re-creations, ALTERs and priorities, on a copy of the psm")
w("# dictionary (fixtures/adt/psm) alone, whose own triggers it drops first.")
w("# Written by trigorder.py; recorded on the local server:")
w("#")
w("#     run.sh <work> <ace32.dll> trigorder.txt > ../../../fixtures/sql/trigorder.json")
w("#")
w("cachecursors 0")
w("connect psm.add ADSSYS adssyspw")
w("stmt")
for t in ["items_bi", "items_ai", "items_au", "items_iod", "items_conflict"]:
    w("direct DROP TRIGGER items.%s" % t)
w("direct DELETE FROM items")
w("direct DELETE FROM audit")
live = []

def create(name, timing="BEFORE", prio=None, event="INSERT"):
    p = "" if prio is None else " PRIORITY %d" % prio
    w("direct CREATE TRIGGER %s ON items %s %s BEGIN INSERT INTO audit (msg) VALUES ('%s'); END%s" % (name, timing, event, name, p))
    live.append(name)

def drop(name):
    w("direct DROP TRIGGER items.%s" % name)
    live.remove(name)

def fire(label, timing_check=None):
    w("# --- " + label)
    w("query SELECT TRIM(name) FROM system.triggers")
    w("direct INSERT INTO items VALUES (1, 'a', 0, 'm')")
    w("query SELECT TRIM(msg) FROM audit ORDER BY seq")
    w("direct DELETE FROM items")
    w("direct DELETE FROM audit")

def clear():
    for n in list(live):
        drop(n)

def case(label, names, timing="BEFORE", prio=None, after=None):
    for n in names:
        create(n, timing, prio)
    fire(label)
    clear()

case("create b, a, c", ["b1", "a1", "c1"])
case("create c, b, a", ["c2", "b2", "a2"])
case("create a, b, c", ["a3", "b3", "c3"])
case("names of different lengths and digits", ["t10", "t2", "t1", "t20", "t3"])
case("case of names", ["Zb", "aa", "Ab", "zz"])
case("underscore and letters", ["x_b", "xb", "x_a", "xa", "x1"])
case("AFTER, create b, a, c", ["b4", "a4", "c4"], timing="AFTER")
case("AFTER, create c, a, b", ["c5", "a5", "b5"], timing="AFTER")
# drop and re-create
w("# --- drop the middle one and create it again")
for n in ["d1", "d2", "d3"]:
    create(n)
drop("d2")
create("d2")
fire("after drop and re-create of d2")
clear()
w("# --- drop the first and create it again")
for n in ["e1", "e2", "e3"]:
    create(n)
drop("e1")
create("e1")
fire("after drop and re-create of e1")
clear()
w("# --- drop two, create new ones in between")
for n in ["f1", "f2", "f3", "f4"]:
    create(n)
drop("f1"); drop("f3")
create("f9"); create("f0")
fire("after drops of f1, f3 and creates of f9, f0")
clear()
# many, shuffled
import random
random.seed(7)
names = ["m%02d" % i for i in range(12)]
random.shuffle(names)
case("twelve, shuffled: " + " ".join(names), names)
names = ["n%02d" % i for i in range(12)]
random.shuffle(names)
case("twelve, shuffled again: " + " ".join(names), names)
# priorities
w("# --- priorities: equal ones keep the rule, others sort")
for n, p in [("p_c", 2), ("p_b", 1), ("p_a", 2), ("p_e", 1), ("p_d", 3)]:
    create(n, "BEFORE", p)
fire("priorities c2 b1 a2 e1 d3")
clear()
# ALTER
w("# --- ALTER TRIGGER of the first created")
for n in ["g1", "g2", "g3"]:
    create(n)
w("direct ALTER TRIGGER g1 ON items BEFORE INSERT BEGIN INSERT INTO audit (msg) VALUES ('g1 altered'); END")
fire("after ALTER of g1")
clear()
# different tables/events interleaved creation
w("# --- triggers of other events and timings created in between")
create("h1"); create("h2", "AFTER"); create("h3", "BEFORE", None, "UPDATE"); create("h4"); create("h0", "AFTER")
fire("h1 h2(after) h3(update) h4 h0(after)")
clear()
w("# --- the same names after the table's triggers were all dropped, in reverse")
case("create k3 k2 k1", ["k3", "k2", "k1"])
print("\n".join(out))
