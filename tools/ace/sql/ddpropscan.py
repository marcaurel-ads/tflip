#!/usr/bin/env python3
"""Writes the script that finds which property ids each AdsDDSet*Property of an
Advantage Client Engine knows: every id up to 1800 and some above, on one
object of each kind, and the ids it answers 5126 to (5042 for a user group) are
the ones it does not know. The lists in `known_properties`
(crates/tflip-engine/src/dictionary_write/properties.rs) are the ids left.

    ddpropscan.py > ddpropscan.txt
    run.sh <work> <ace32.dll> ddpropscan.txt > ddpropscan.json

The script is far too long to keep; the ids that are known may change the
object they are tried on, which does not matter here.
"""

ids = list(range(0, 1800)) + [2000, 2047, 2048, 4096, 8192, 16384, 32767, 32768, 40000, 65534, 65535]
print(
    """# Which property ids each AdsDDSet*Property answers 5126 (or, for a user group,
# 5042) to, written by ddpropscan.py. Run in the default data/ on a dictionary
# made here.
ddcreate dict\\scan.add
stmt
direct CREATE TABLE t0 (id INTEGER, pid INTEGER, PRIMARY KEY (id))
direct CREATE TABLE t0c (id INTEGER, pid INTEGER, PRIMARY KEY (id))
direct CREATE INDEX t0i ON t0 (pid)
direct CREATE VIEW v0 AS SELECT id FROM t0
direct CREATE PROCEDURE p0() BEGIN SELECT 1 FROM system.iota; END
direct EXECUTE PROCEDURE sp_CreateUser('x0', 'pw', '')
direct EXECUTE PROCEDURE sp_CreateGroup('y0', '')
ddpub q0 - 0
ddart q0 t0c id 0
ddsub s0 q0 target0 usr pw q0.adt 0 - 0"""
)
for n in ids:
    print(f"ddapi AdsDDSetTableProperty t0 #{n} c1 #L #0 -")
    print(f"ddapi AdsDDSetFieldProperty t0 id #{n} c2 #L #0 -")
    print(f"ddapi AdsDDSetIndexProperty t0 t0i #{n} c3 #L")
    print(f"ddapi AdsDDSetViewProperty v0 #{n} c4 #L")
    print(f"ddapi AdsDDSetProcedureProperty p0 #{n} c5 #L")
    print(f"ddapi AdsDDSetUserProperty x0 #{n} c6 #L")
    print(f"ddapi AdsDDSetUserGroupProperty y0 #{n} c7 #L")
    print(f"ddapi AdsDDSetPublicationProperty q0 #{n} c8 #L")
    print(f"ddapi AdsDDSetArticleProperty q0 t0c #{n} c9 #2")
    print(f"ddapi AdsDDSetSubscriptionProperty s0 #{n} c11 #L")
    print(f"dddbprop {n} -1 c10")
print("disconnect")
