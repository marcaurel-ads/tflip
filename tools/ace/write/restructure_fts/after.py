import os, sys
# usage: after.py <out folder> [drop|controls|types|keys]
# A dictionary's table whose restructure failed on an order the new types
# cannot build (a full-text order on a field retyped to Integer), then what
# follows: restructures, SQL, opens. The scenarios are kept small because
# the original's dictionary index grows with the objects a dictionary adds
# and removes in ways tflip does not match:
#   (none)    restructure_fts_dict_after: g tables, one case each
#   drop      restructure_fts_dict_drop: a field of an order dropped
#   controls  restructure_fts_dict_after_fields: what changes the file
#   types     restructure_fts_dict_after_types: retypes, then AdsReindex ...
#   keys      restructure_fts_dict_after_keys: leading field, no file, SQL
out = sys.argv[1]
mode = sys.argv[2] if len(sys.argv) > 2 else ""
drop = mode == "drop"
os.makedirs(out, exist_ok=True)
rows = [(1, "alpha beta gamma", "The Quick Brown Fox", "quick memo words"),
        (2, "beta delta epsilon", "jumps over lazy dog", None),
        (3, "gamma gamma zeta", "the and for", "noise gamma"),
        (4, "eta theta", "Fox and Hound", "hound fox")]


def q(v):
    return "NULL" if v is None else "'" + v.replace("'", "''") + "'"


# Each table has the orders id, body (full-text), title (unique) and m
# (full-text), then z; k tables have a field before them, n1 one after z, m
# tables have the full-text order first.
G = M = H = K = F = Q = []
if drop:
    G = ["g6"]
    H = ["h6", "h7"]
elif mode == "controls":
    M = ["ca", "cb", "cc", "cd", "ce", "fa", "fb", "fc", "fd", "fe"]
elif mode == "types":
    H = ["h%d" % i for i in (1, 2, 3, 4, 5, 8, 9, 10, 11, 12)]
elif mode == "keys":
    K = ["k1", "k2", "k3", "k4"]
    F = ["m1", "m2"]
    Q = ["q1", "n1"]
else:
    G = ["g1", "g2", "g3", "g4", "g5", "g7", "g8"]
# zz comes last and does not fail: the memo blocks of the last table's
# orders are at the end of the memo file, which the original treats alone
# (KNOWN_ISSUES).
T = G + M + H + K + F + Q
s = ["ddcreate dd.add"]
for n in T + ["zz"]:
    lead = "pre CHAR(4), " if n in K else ""
    tail = ", z2 CHAR(4)" if n == "n1" else ""
    s.append("sql CREATE TABLE %s (%sid INTEGER, body CHAR(40), title CHAR(30), m MEMO, z CHAR(4)%s)" % (n, lead, tail))
    vals = ", ".join("(%s%d, %s, %s, %s, 'zz'%s)" % ("'pp', " if n in K else "", a, q(b), q(c), q(d), ", 'yy'" if n == "n1" else "") for a, b, c, d in rows)
    s.append("sql INSERT INTO %s VALUES %s" % (n, vals))
    if n in F:
        s.append("sql CREATE INDEX %s_body ON %s (body) CONTENT KEEPSCORE" % (n, n))
        s.append("sql CREATE INDEX %s_id ON %s (id)" % (n, n))
    else:
        s.append("sql CREATE INDEX %s_id ON %s (id)" % (n, n))
        s.append("sql CREATE INDEX %s_body ON %s (body) CONTENT KEEPSCORE" % (n, n))
    s.append("sql CREATE UNIQUE INDEX %s_title ON %s (title)" % (n, n))
    s.append("sql CREATE INDEX %s_m ON %s (m) CONTENT" % (n, n))
if G and not drop:
    s.append("sql CREATE INDEX g7_bp ON g7 (body)")
    s.append("sql CREATE INDEX g8_f ON g8 (body) CONTENT IN FILE words8")
    s.append("sql CREATE INDEX g8_up ON g8 (title) IN FILE up8")
FAIL = "||body,body,Integer,4"
# The original keeps the dictionary's index up to date a record at a time,
# and a failed restructure, which moves the objects' records, leaves the
# keys of equal orders in other places than a rebuilt index has them
# (KNOWN_ISSUES).
o = ["# Not compared: dd.ai", "ddconnect dd.add ADSSYS -", "tabletype database"]
tags = ["id", "body", "title", "m"]


def orders(n):
    r = []
    for t in tags + (["bp"] if n == "g7" else []):
        # The rows of a full-text order are not what this scenario is about.
        r += ["order %s_%s" % (n, t)] + (["rows"] if t == "id" else [])
    return r


def look(n):
    return ["open :%s" % n, "top", "record"] + orders(n) + ["close"]


# Every table fails once, as g1 to g6 do on their full-text order of body;
# g7's plain order on body fails on a Memo instead; the controls (ca to ce)
# do not fail.
for n in T:
    if mode == "controls" and n.startswith("c"):
        continue
    if n == "g7":
        o.append("restructure :g7 0 ||body,body,Memo,10")
    else:
        o.append("restructure :%s 0 %s" % (n, FAIL))
o.append("snapshot failed")


def step(n, what):
    x = what
    o.extend([("sql " + x[4:]) if x.startswith("SQL ") else "restructure :%s 0 %s" % (n, x), "snapshot %s" % n])
    o.extend(look(n))


if drop:
    # The field of an order dropped: the orders are made from the Key
    # objects, and the first one on it fails, in SQL or not.
    step("g6", "SQL ALTER TABLE g6 DROP COLUMN body")
    step("h6", "|title|")
    step("h7", "SQL ALTER TABLE h7 DROP COLUMN title")
elif G:
    # g1: a restructure that adds a field leaves the index file as it is
    step("g1", "extra,Char,4||")
    # g2: what else touches the table: SQL, with no table open on it, joins
    # the file's orders; a table open on it keeps SQL out
    o += ["open :g2", "close", "sql CREATE INDEX g2_x ON g2 (id)", "sql EXECUTE PROCEDURE sp_Reindex('g2', 0)",
          "sql INSERT INTO g2 (id) VALUES (9)", "snapshot g2"]
    # g3: Char to Memo and back
    step("g3", "||body,body,Memo,10")
    step("g3", "||body,body,Char,40")
    # g4: a second failure, then a good one
    o += ["restructure :g4 0 %s" % FAIL, "snapshot g4a"]
    step("g4", "extra,Char,4||")
    # g5: by SQL
    step("g5", "SQL ALTER TABLE g5 ADD COLUMN extra CHAR(4)")
    step("g7", "extra,Char,4||")
    step("g8", "extra,Char,4||")
if M:
    # What changes an order's field rebuilds the file (c: before the
    # failure; f: after it); what does not, leaves it.
    MX = {"a": "extra,Char,4||", "b": "||z,z,Char,8", "c": "||id,id,Double,8", "d": "|z|", "e": "||title,title2,Char,30"}
    for n in M:
        step(n, MX[n[1]])
if H and not drop:
    HX = {
        "h1": "||title,title,Char,40",
        "h2": "||id,id,ShortInt,2",
        "h3": "extra,Char,4||z,z,Memo,10",
        "h4": "||body,body,CICharacter,40",
        "h5": "||body,body,Char,50",
        "h8": "SQL ALTER TABLE h8 ALTER COLUMN id id SHORT",
        "h9": "SQL ALTER TABLE h9 ALTER COLUMN body body MEMO",
    }
    for n in H:
        if n in HX:
            step(n, HX[n])
    # AdsReindex, AdsCreateIndex61, AdsPackTable and an append on the table
    # that failed: the file's orders only; the orders made join the Key
    # objects the next restructure builds from.
    o += ["open :h10", "reindex", "close", "snapshot h10a", "open :h10", "createindex - h10_x 0 512 z", "close", "snapshot h10b"]
    o += ["open :h10", "createindex - h10_y 0 512 title", "close", "snapshot h10c"] + look("h10")
    o += ["restructure :h10 0 ||id,id,Double,8", "snapshot h10d"] + look("h10") + ["open :h10", "order h10_x", "order h10_y", "close"]
    o += ["open :h11", "pack", "close", "snapshot h11"] + look("h11")
    o += ["open :h12", "append", "set id long 7", "write", "close", "snapshot h12"] + look("h12")
if K or F or Q:
    KX = {
        "k1": "|pre|",
        "k2": "||pre,pre,Char,8",
        "k3": "||title,title2,Char,40",
        "k4": "|z|title,title,Char,40",
        "n1": "|z|",
        "m1": "extra,Char,4||",
        "m2": "||body,body,Memo,10",
    }
    for n in K + ["n1"]:
        step(n, KX[n])
    # The table that cannot open its index file (its first order failed):
    # nothing opens it, and a restructure fails before it changes anything.
    for n in F:
        o += ["restructure :%s 0 %s" % (n, KX[n]), "snapshot %s" % n, "open :%s" % n]
    # SQL on a table that failed, with no table open on it
    o += ["sql CREATE INDEX q1_x ON q1 (z)", "sql EXECUTE PROCEDURE sp_Reindex('q1', 0)",
          "sql INSERT INTO q1 (id) VALUES (9)", "sql UPDATE q1 SET id = 8 WHERE id = 9", "sql DELETE FROM q1 WHERE id = 8",
          "snapshot q1"]
open(os.path.join(out, "setup.txt"), "w").write("\n".join(s) + "\n")
# Snapshots (SNAPSHOTS=1) are for looking at the files between the steps;
# the fixture keeps the last state only.
if not os.environ.get("SNAPSHOTS"):
    o = [line for line in o if not line.startswith("snapshot ")]
open(os.path.join(out, "ops.txt"), "w").write("\n".join(o) + "\n")
