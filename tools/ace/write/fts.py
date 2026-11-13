#!/usr/bin/env python3
"""Writes the setup.txt and ops.txt of the full-text scenarios for
tools/ace/write/run.sh:

- fts_build: tables with text of many kinds (case, accents, quotes,
  numbers, long words, NULL, a deleted record) and a memo table large enough
  for trees of several levels; the ops create full-text orders on them with
  each option.
- fts_writes: the same kinds of tables with full-text orders made in the
  setup; the ops append, change and delete records, so that the orders'
  leaves split, empty and come back from the free list.
- fts_rebuild: full-text orders made with AdsCreateFTSIndex, in the table's
  index file and in files of their own, then rebuilt by AdsReindexFTS,
  AdsReindex, a pack and a zap.

- fts_binary: full-text orders on Binary, Image and VarBinary fields of an
  ADT table (made, and kept current by writes, deletes and recalls), a
  zero byte among the words, and a table of 300 records written at random.
- fts_recall, fts_recall_binary: records deleted, recalled one by one and
  all at once in tables whose full-text orders are on character, memo,
  Binary and Image fields.
- fts_cdx_create, fts_cdx, fts_cdx_null: full-text orders on FoxPro and VFP
  tables: made by SQL (a tag of the structural .cdx) and by AdsCreateFTSIndex
  (a single-order file of its own), kept current, rebuilt, packed and zapped;
  a VFP field set to NULL.

    tools/ace/write/fts.py fixtures/write
"""
import os
import random
import sys

TRICKY = [
    "The Quick Brown Fox jumps over the lazy dog",
    "O'Malley said \"hello\" to `everyone`",
    "e-mail foo.bar. (paren) end. -dash- _under_ #tag 50% $5",
    "1,423.99 and 3.14 and 1.000,5 and 12:30",
    "Ärger über Straße café naïve ÉCOLE Ôte",
    "Superextraordinarilylongwordthatgoesonandonforever and short",
    "a ab abc abcd ABCDE abcdef",
    "tab\there\nnewline\rreturn\x0bvt\x0cff",
    "MiXeD mixed MIXED Mixed",
    "the and for with noise only",
    "repeat repeat repeat again repeat",
    "x,y;z!w?v",
    "",
]


def words(rng, n):
    vowels = "aeiou"
    consonants = "bcdfghjklmnprstvwz"
    out = []
    for _ in range(n):
        length = rng.randrange(3, 11)
        out.append("".join(rng.choice(consonants if i % 2 == 0 else vowels)
                           for i in range(length)))
    return out


def quote(text):
    return text.replace("'", "''").replace("\\", "\\\\").replace("\t", "\\t") \
        .replace("\n", "\\n").replace("\r", "\\r").replace("\x0b", "\\x0b") \
        .replace("\x0c", "\\x0c")


def tricky_table(name, column):
    lines = ["sql CREATE TABLE %s (id INTEGER, body %s)" % (name, column)]
    for i, text in enumerate(TRICKY, 1):
        lines.append("sql INSERT INTO %s VALUES (%d, '%s')" % (name, i, quote(text)))
    n = len(TRICKY)
    lines.append("sql INSERT INTO %s VALUES (%d, NULL)" % (name, n + 1))
    lines.append("sql INSERT INTO %s VALUES (%d, 'deleted words here')" % (name, n + 2))
    lines.append("sql DELETE FROM %s WHERE id = %d" % (name, n + 2))
    return lines


def memo_table(name, rng, vocabulary, records, per_record):
    lines = ["sql CREATE TABLE %s (id INTEGER, m MEMO)" % name]
    rows = []
    for i in range(1, records + 1):
        text = " ".join(rng.choice(vocabulary) for _ in range(rng.randrange(1, per_record)))
        rows.append("(%d, '%s')" % (i, text))
    for start in range(0, len(rows), 50):
        lines.append("sql INSERT INTO %s VALUES %s" % (name, ", ".join(rows[start:start + 50])))
    return lines


def build(out):
    rng = random.Random(11)
    vocabulary = words(rng, 400)
    setup = ["# Tables to build full-text orders on."]
    setup += tricky_table("c", "CHAR(80)")
    setup += tricky_table("m", "MEMO")
    setup += tricky_table("o", "MEMO")
    setup += memo_table("big", rng, vocabulary, 900, 14)
    setup += memo_table("ks", rng, vocabulary[:60], 400, 20)
    setup += ["sql CREATE TABLE ex (id INTEGER, body CHAR(40), n INTEGER)",
              "sql INSERT INTO ex VALUES (1, 'alpha bravo', 1), (2, 'charlie delta', 2)",
              "sql CREATE INDEX n ON ex (n)",
              "sql CREATE TABLE e (id INTEGER, body MEMO)",
              "sql INSERT INTO e VALUES (1, 'alpha extra words'), (2, 'Alpha a b c xylophone')"]
    ops = [
        "# The default options, on a character and a memo field.",
        "sql CREATE INDEX c_fts ON c (body) CONTENT",
        "sql CREATE INDEX m_fts ON m (body) CONTENT",
        "# Each option on its own order of one table.",
        "sql CREATE INDEX o_min ON o (body) CONTENT MIN WORD 1 MAX WORD 5",
        "sql CREATE INDEX o_case ON o (body) CONTENT CASESENSITIVE",
        "sql CREATE INDEX o_score ON o (body) CONTENT KEEPSCORE",
        "sql CREATE INDEX o_fixed ON o (body) CONTENT NOTMAINTAINED",
        "sql CREATE INDEX o_delim ON o (body) CONTENT DELIMITERS ',.' PROTECTNUMBERS",
        "sql CREATE INDEX o_newdelim ON o (body) CONTENT NEW DELIMITERS ' -'",
        "sql CREATE INDEX o_noise ON o (body) CONTENT NOISE 'quick brown'",
        "sql CREATE INDEX o_newnoise ON o (body) CONTENT NEW NOISE 'xx repeat'",
        "sql CREATE INDEX o_drop ON o (body) CONTENT DROPCHARS 'z' NEW CONDITIONALS '.'",
        "sql CREATE INDEX o_all ON o (body) CONTENT MIN WORD 2 MAX WORD 12 CASESENSITIVE KEEPSCORE",
        "# Trees of several levels, with and without scores.",
        "sql CREATE INDEX big_fts ON big (m) CONTENT",
        "sql CREATE INDEX ks_fts ON ks (m) CONTENT KEEPSCORE",
        "sql CREATE INDEX ks_short ON ks (m) CONTENT MAX WORD 4",
        "# A full-text order added to a file of 512-byte pages with an order.",
        "sql CREATE INDEX ex_fts ON ex (body) CONTENT",
        "# How the options merge with the defaults, and what is refused.",
        "sql CREATE INDEX e1 ON e (body) CONTENT DELIMITERS ' x'",
        "sql CREATE INDEX e2 ON e (body) CONTENT NOISE 'The about Zebra apple zebra'",
        "sql CREATE INDEX e3 ON e (body) CONTENT DROPCHARS '\"z' CONDITIONALS ',y'",
        "sql CREATE INDEX e4 ON e (body) CONTENT MIN WORD 1 NEW NOISE 'b a c B'",
        "sql CREATE INDEX e5 ON e (body) CONTENT NEW DELIMITERS '' NEW DROPCHARS '' NEW CONDITIONALS ''",
        "sql CREATE INDEX e6 ON e (body) CONTENT NEW NOISE ''",
        "sql CREATE INDEX e7 ON e (body) CONTENT MIN WORD 0 MAX WORD 1",
        "sql CREATE INDEX e8 ON e (body) CONTENT MIN WORD 10 MAX WORD 5",
        "sql CREATE INDEX e9 ON e (body) CONTENT MAX WORD 0",
        "sql CREATE INDEX e10 ON e (body) CONTENT NEW DELIMITERS 'aa' NEW NOISE 'zz  yy'",
    ]
    write(out, "fts_build", setup, ops)


def maintained(out):
    rng = random.Random(23)
    vocabulary = words(rng, 300)
    setup = ["# Tables with full-text orders, to write to."]
    setup += tricky_table("t", "MEMO")
    setup += [
        "sql CREATE INDEX t_fts ON t (body) CONTENT",
        "sql CREATE INDEX t_score ON t (body) CONTENT KEEPSCORE",
        "sql CREATE INDEX t_case ON t (body) CONTENT CASESENSITIVE MAX WORD 6",
        "sql CREATE INDEX t_fixed ON t (body) CONTENT NOTMAINTAINED",
        "sql CREATE INDEX t_id ON t (id)",
    ]
    setup += ["sql CREATE TABLE c (id INTEGER, body CHAR(60))",
              "sql CREATE INDEX c_fts ON c (body) CONTENT"]
    setup += memo_table("big", rng, vocabulary, 300, 12)
    setup += ["sql CREATE INDEX big_fts ON big (m) CONTENT",
              "sql CREATE INDEX big_score ON big (m) CONTENT KEEPSCORE"]
    ops = ["open t.adt shared"]
    n = len(TRICKY) + 2
    for text in ["fresh words appended", "Repeat REPEAT repeat", ""]:
        n += 1
        ops += ["append", "set id long %d" % n]
        if text:
            ops.append("set body string %s" % text)
        ops += ["write", "state"]
    ops += ["goto 1", "set body string The quick red fox", "write", "state",
            "goto 2", "set body string completely different now", "write", "state",
            "goto 11", "set body string repeat once", "write", "state",
            "goto 3", "set body null", "write", "state",
            "goto 14", "set body string revived text", "write", "state",
            "goto 4", "delete", "state",
            "goto 6", "set id long 600", "write", "state"]
    ops += ["open c.adt shared"]
    for i in range(1, 30):
        ops += ["append", "set id long %d" % i,
                "set body string %s" % " ".join(rng.choice(vocabulary) for _ in range(5)),
                "write", "state"]
    ops += ["open big.adt shared"]
    live = list(range(1, 301))
    for step in range(400):
        what = rng.random()
        record = rng.choice(live)
        text = " ".join(rng.choice(vocabulary) for _ in range(rng.randrange(1, 12)))
        if what < 0.4:
            ops += ["append", "set id long %d" % (1000 + step),
                    "set m string %s" % text, "write"]
        elif what < 0.85:
            ops += ["goto %d" % record, "set m string %s" % text, "write"]
        else:
            ops += ["goto %d" % record, "delete"]
            live.remove(record)
    ops.append("state")
    write(out, "fts_writes", setup, ops)


def rebuilt(out):
    rng = random.Random(31)
    vocabulary = words(rng, 120)
    setup = ["# Tables whose full-text orders the API makes and rebuilds."]
    setup += ["sql CREATE TABLE a (id INTEGER, body CHAR(60), m MEMO, v VARCHAR(40), ci CICHAR(20))"]
    rows = []
    for i, text in enumerate(TRICKY[:12], 1):
        rows.append("(%d, '%s', '%s', '%s', '%s')" % (
            i, quote(text[:60]), quote(text[::-1]), quote(text[:40]), quote(text[:20])))
    setup.append("sql INSERT INTO a VALUES %s" % ", ".join(rows))
    setup += memo_table("r", rng, vocabulary, 300, 10)
    setup += ["sql CREATE INDEX r_fts ON r (m) CONTENT KEEPSCORE",
              "sql CREATE INDEX r_plain ON r (m) CONTENT",
              "sql CREATE INDEX r_id ON r (id)"]
    setup += memo_table("z", rng, vocabulary, 50, 6)
    setup += ["sql CREATE INDEX z_fts ON z (m) CONTENT"]
    ops = [
        "open a.adt",
        "# AdsCreateFTSIndex: the defaults with and without lists, none of them.",
        "createfts - a1 body 0 3 30 0 1 \"\" 1 \"\" 1 \"\" 1 \"\"",
        "createfts - a2 body 0 3 30 0 1 - 1 - 1 - 1 -",
        "# One that keeps scores, in a file of its own: rebuilt beside other full-text",
        "# orders, the original counts some words wrong (docs/ADS_BUGS.md).",
        "createfts scored.adi a3 m 0 2 10 0x180 0 \\x20 0 apple\\x20Pie\\x20pie 0 ' 0 .",
        "createfts - a4 body 0 3 30 0x220 1 ,. 1 extra 1 z 1 y",
        "# Word lengths and options at their edges, and what is refused.",
        "createfts - a5 body 0 0 0 0 1 - 1 - 1 - 1 -",
        "createfts - a6 body 0 5 5 0 1 - 1 - 1 - 1 -",
        "createfts - a7 body 0 3 30 0 0 \"\" 1 - 1 - 1 -",
        "createfts - a8 id 0 3 30 0 1 - 1 - 1 - 1 -",
        "createfts - a1 body 0 3 30 0 1 - 1 - 1 - 1 -",
        "createfts - a9 v 0 3 30 0 1 - 1 - 1 - 1 -",
        "createfts - a10 ci 0 3 30 0 1 - 1 - 1 - 1 -",
        "createfts - a11 body 0 3 30 0x40 1 - 1 - 1 - 1 -",
        "# Index files of their own: the default page size, and one given.",
        "createfts words.adi w1 body 0 3 30 0 1 - 1 - 1 - 1 -",
        "createfts words.adi w2 m 512 3 30 0 1 - 1 - 1 - 1 -",
        "createfts big.adi b1 m 8192 3 30 0x100 1 - 1 - 1 - 1 -",
        "createfts small.adi s1 m 512 3 30 0 1 - 1 - 1 - 1 -",
        "deleteindex a2",
        "# Records written, then the full-text orders rebuilt.",
        "append", "set id long 100", "set body string brand new words here",
        "set m string memo words too", "write",
        "goto 2", "set body string replaced text", "write",
        "goto 5", "delete",
        "reindexfts 0",
        "open r.adt",
    ]
    deleted = rng.sample(range(1, 301), 60)
    for record in deleted:
        ops += ["goto %d" % record, "delete"]
    for record in rng.sample([r for r in range(1, 301) if r not in deleted], 40):
        ops += ["goto %d" % record,
                "set m string %s" % " ".join(rng.choice(vocabulary) for _ in range(8)),
                "write"]
    ops += ["reindexfts 0", "reindex", "state", "pack", "state", "reindex61 2048", "state",
            "append", "set id long 999", "set m string after the rebuild", "write", "state",
            # A zap right after a full-text reindex leaves the memo file's blocks on its
            # free list (docs/ADS_BUGS.md); this one follows a write.
            "open z.adt", "goto 3", "set m string changed again", "write",
            "zap", "state",
            "append", "set id long 1", "set m string first words after zap", "write", "state"]
    write(out, "fts_rebuild", setup, ops)


def binary(out):
    rng = random.Random(5)
    V = ["".join(rng.choice("bcdfghjklmnprstvwz" if i%2==0 else "aeiou") for i in range(rng.randrange(3,9))) for _ in range(80)]
    T = ["The Quick Brown Fox jumps over the lazy dog","O'Malley said hello","repeat repeat repeat again","Ärger über Straße café",
         "bin\x00ary wor\x01ds inside\xff\xfe junk","a ab abc","x,y;z!w?v e-mail 1,423.99","MiXeD mixed MIXED",""]
    def h(s): return s.encode("cp1252", "surrogateescape").hex()
    def words(n): return " ".join(rng.choice(V) for _ in range(n))
    setup=["createtable b.adt 0 id,Integer;bl,Binary;im,Image;vb,VarBinary,40;ch,Character,40"]
    for i,t in enumerate(T,1):
        setup+=["append","set id long %d"%i,"set bl binary %s"%h(t),"set im image %s"%h(t[::-1]),"set vb binary %s"%h(t[:40]),"write"]
    setup+=["append","set id long 20","write"]
    setup+=["append","set id long 21","set bl empty","write"]
    for n in range(30):
        setup+=["append","set id long %d"%(100+n),"set bl binary %s"%h(words(rng.randrange(1,10))),"write"]
    setup+=["createtable w.adt 0 id,Integer;bl,Binary;im,Image"]
    for n in range(300):
        setup+=["append","set id long %d"%(100+n),"set bl binary %s"%h(words(rng.randrange(1,12))),"set im image %s"%h(words(4)),"write"]
    ops=["# created in SQL and through the API",
    "sql CREATE INDEX b_bl ON b (bl) CONTENT",
    "sql CREATE INDEX b_im ON b (im) CONTENT KEEPSCORE",
    "sql CREATE INDEX b_vb ON b (vb) CONTENT CASESENSITIVE MAX WORD 6",
    "sql CREATE INDEX b_ch ON b (ch) CONTENT",
    "sql CREATE INDEX b_fixed ON b (bl) CONTENT NOTMAINTAINED",
    "sql CREATE INDEX b_id ON b (id)",
    "sql CREATE INDEX w_bl ON w (bl) CONTENT",
    "sql CREATE INDEX w_im ON w (im) CONTENT",
    "open b.adt",
    "createfts - a1 bl 0 3 30 0 1 - 1 - 1 - 1 -",
    "createfts - a2 im 0 2 20 0x100 1 - 1 - 1 - 1 -",
    "# writes",
    "state"]
    for t in ["fresh words appended","Repeat REPEAT repeat","",None]:
        ops+=["append","set id long %d"%(500+len(ops))]
        if t is not None: ops+=["set bl binary "+h(t),"set im image "+h(t),"set vb binary "+h(t)]
        ops+=["write","state"]
    ops+=["goto 1","set bl binary "+h("The quick red fox"),"write","state",
          "goto 2","set bl null","write","state",
          "goto 3","set im image "+h("completely different"),"write","state",
          "goto 4","delete","state","recall","state",
          "goto 5","delete","state","recall","state",
          "goto 21","set bl binary "+h("was empty"),"write","state",
          "goto 6","delete","goto 7","delete","recall","state"]
    ops+=["open w.adt"]
    live=list(range(1,301))
    for step in range(300):
        r=rng.random(); rec=rng.choice(live)
        if r<.35: ops+=["append","set id long %d"%(1000+step),"set bl binary "+h(words(rng.randrange(1,12))),"set im image "+h(words(3)),"write"]
        elif r<.8: ops+=["goto %d"%rec,"set bl binary "+h(words(rng.randrange(1,12))),"write"]
        elif r<.9: ops+=["goto %d"%rec,"delete"]; live.remove(rec)
        else: ops+=["goto %d"%rec,"set im image "+h(words(3)),"write"]
    ops+=["state","reindex","state","reindexfts 0","state","pack","state"]
    write(out, "fts_binary", setup, ops)


def recalled(out):
    setup = []
    for name, column in [("c", "c CHAR(40)"), ("m", "m MEMO"), ("k", "m MEMO")]:
        setup.append("sql CREATE TABLE %s (id INTEGER, %s)" % (name, column))
        for i, text in enumerate(["alpha bravo", "charlie delta", "echo foxtrot",
                                  "golf hotel", "india juliet"], 1):
            setup.append("sql INSERT INTO %s VALUES (%d, '%s')" % (name, i, text))
    setup += ["sql CREATE INDEX c_fts ON c (c) CONTENT KEEPSCORE",
              "sql CREATE INDEX m_fts ON m (m) CONTENT",
              "sql CREATE INDEX k_fts ON k (m) CONTENT KEEPSCORE",
              "sql CREATE INDEX k_id ON k (id)"]
    ops = []
    for table in "cmk":
        ops += ["open %s.adt" % table, "goto 2", "delete", "goto 4", "delete", "state",
                "recallall", "state", "close"]
    write(out, "fts_recall", setup, ops)
    setup = ["createtable b.adt 0 id,Integer;bl,Binary;im,Image"]
    for i, text in enumerate(["alpha bravo", "charlie delta", "echo foxtrot",
                              "golf hotel", "india juliet"], 1):
        setup += ["append", "set id long %d" % i, "set bl binary " + text.encode().hex(),
                  "set im image " + text.encode().hex(), "write"]
    ops = ["sql CREATE INDEX b_bl ON b (bl) CONTENT KEEPSCORE",
           "sql CREATE INDEX b_im ON b (im) CONTENT",
           "open b.adt", "goto 2", "delete", "goto 4", "delete", "state", "recall", "state",
           "recallall", "state", "goto 3", "delete", "recallall", "state"]
    write(out, "fts_recall_binary", setup, ops)


def xbase(out):
    rng = random.Random(9)
    V = ["".join(rng.choice("bcdfghjklmnprstvwz" if i%2==0 else "aeiou") for i in range(rng.randrange(3,9))) for _ in range(80)]
    T = ["The Quick Brown Fox jumps over the lazy dog","O'Malley said hello","repeat repeat repeat again","Ärger über Straße café",
         "a ab abc","x,y;z!w?v e-mail 1,423.99","MiXeD mixed MIXED",""]
    def words(n): return " ".join(rng.choice(V) for _ in range(n))
    def q(s): return s.replace("'", "''")
    setup = []
    for kind in ("cdx", "vfp"):
        p = kind[0]
        setup += ["tabletype " + kind]
        setup += ["sql CREATE TABLE %st (id INTEGER, body CHAR(60)%s, m MEMO%s, bl BLOB%s)" % ((p,) + (("", "", "") if p == "c" else (" NULL",) * 3))]
        for i, t in enumerate(T, 1):
            setup += ["sql INSERT INTO %st VALUES (%d, '%s', '%s', NULL)" % (p, i, q(t), q(t[::-1]))]
        setup += ["sql INSERT INTO %st VALUES (%d, NULL, NULL, NULL)" % (p, len(T) + 1),
                  "sql INSERT INTO %st VALUES (%d, 'deleted words', 'deleted memo', NULL)" % (p, len(T) + 2),
                  "sql DELETE FROM %st WHERE id = %d" % (p, len(T) + 2),
                  "sql CREATE INDEX %st_id ON %st (id)" % (p, p)]
        setup += ["sql CREATE TABLE %sbig (id INTEGER, m MEMO)" % p]
        for start in range(0, 300, 50):
            setup += ["sql INSERT INTO %sbig VALUES %s" % (p, ", ".join("(%d, '%s')" % (start + j, words(rng.randrange(1, 12))) for j in range(50)))]
    ops = []
    for kind in ("cdx", "vfp"):
        p = kind[0]
        ops += ["tabletype " + kind,
                "sql CREATE INDEX %st_fts ON %st (body) CONTENT" % (p, p),
                "sql CREATE INDEX %st_m ON %st (m) CONTENT KEEPSCORE" % (p, p),
                "sql CREATE INDEX %st_case ON %st (m) CONTENT CASESENSITIVE MAX WORD 6" % (p, p),
                "sql CREATE INDEX %st_fixed ON %st (body) CONTENT NOTMAINTAINED" % (p, p),
                "sql CREATE INDEX %st_bl ON %st (bl) CONTENT" % (p, p),
                "sql CREATE INDEX %sbig_fts ON %sbig (m) CONTENT" % (p, p),
                "sql CREATE INDEX %sbig_sc ON %sbig (m) CONTENT KEEPSCORE" % (p, p),
                "open %st.dbf" % p,
                "createfts - a1 body 0 3 30 0 1 - 1 - 1 - 1 -",
                "createfts own.cdx a2 m 0 3 30 0 1 - 1 - 1 - 1 -",
                "createfts - a3 id 0 3 30 0 1 - 1 - 1 - 1 -",
                "state"]
        for text in ["fresh words appended", "Repeat REPEAT repeat", ""]:
            ops += ["append", "set id long %d" % (50 + len(ops)), "set body string %s" % text, "set m string %s" % text, "write", "state"]
        ops += ["goto 1", "set body string The quick red fox", "set m string memo changed", "write", "state",
                "goto 2", "set body null", "write", "state",
                "goto 3", "set m string completely different", "write", "state",
                "goto 4", "delete", "state", "recall", "state",
                "goto 5", "delete", "state",
                "goto 6", "set id long 600", "write", "state",
                "recall", "state",
                "close"]
        ops += ["open %sbig.dbf" % p]
        live = list(range(1, 301))
        for step in range(300):
            r = rng.random()
            rec = rng.choice(live)
            if r < .35:
                ops += ["append", "set id long %d" % (1000 + step), "set m string %s" % words(rng.randrange(1, 12)), "write"]
            elif r < .8:
                ops += ["goto %d" % rec, "set m string %s" % words(rng.randrange(1, 12)), "write"]
            else:
                ops += ["goto %d" % rec, "delete"]
                live.remove(rec)
        ops += ["state", "reindex", "state", "reindexfts 0", "state", "pack", "state", "zap", "state",
                "append", "set id long 1", "set m string after zap", "write", "state", "close"]
    write(out, "fts_cdx", setup, ops)
    # The smallest tables of them, for the bytes of their files.
    write(out, "fts_cdx_create", [
        "tabletype cdx",
        "sql CREATE TABLE c1 (id INTEGER, body CHAR(40))",
        "sql INSERT INTO c1 VALUES (1, 'alpha beta gamma'), (2, 'beta delta')",
        "sql CREATE TABLE c2 (id INTEGER, body CHAR(40))",
        "sql INSERT INTO c2 VALUES (1, 'alpha beta gamma'), (2, 'beta delta')",
        "sql CREATE TABLE c3 (id INTEGER, body CHAR(40))",
        "sql CREATE TABLE c4 (id INTEGER, body CHAR(40))",
        "sql INSERT INTO c4 VALUES (1, 'alpha beta gamma'), (2, 'beta delta')",
        "sql CREATE INDEX c4_id ON c4 (id)",
        "sql CREATE TABLE c5 (id INTEGER, body CHAR(40))",
        "sql INSERT INTO c5 VALUES (1, 'alpha beta gamma'), (2, 'beta delta')",
    ], [
        "tabletype cdx",
        "sql CREATE INDEX f1 ON c1 (body) CONTENT",
        "sql CREATE INDEX f2 ON c2 (body) CONTENT KEEPSCORE",
        "sql CREATE INDEX f3 ON c3 (body) CONTENT",
        "sql CREATE INDEX f4 ON c4 (body) CONTENT",
        "open c5.dbf",
        "createfts - g1 body 0 3 30 0 1 - 1 - 1 - 1 -",
        "createfts own.cdx g2 body 0 3 30 0 1 - 1 - 1 - 1 -",
    ])
    # A VFP field set to NULL keeps its bytes, and the order its words.
    setup = ["tabletype vfp",
             "sql CREATE TABLE v (id INTEGER, body CHAR(60) NULL)"]
    for i, t in enumerate(["alpha bravo charlie delta echo", "foxtrot golf hotel", "india juliet kilo lima mike november",
                           "oscar papa quebec", "romeo sierra tango", "uniform victor whiskey"], 1):
        setup.append("sql INSERT INTO v VALUES (%d, '%s')" % (i, t))
    setup.append("sql CREATE INDEX v_fts ON v (body) CONTENT")
    setup.append("sql CREATE INDEX v_sc ON v (body) CONTENT KEEPSCORE")
    ops = ["tabletype vfp", "open v.dbf",
           "goto 1", "set body null", "write", "state",
           "goto 2", "set body string zzz", "write",
           "goto 3", "set body empty", "write",
           "goto 4", "delete", "state",
           "goto 5", "set id long 55", "write",
           "goto 6", "set body null", "write",
           "goto 6", "set body string back again", "write"]
    write(out, "fts_cdx_null", setup, ops)


def write(out, name, setup, ops):
    folder = os.path.join(out, name)
    os.makedirs(folder, exist_ok=True)
    with open(os.path.join(folder, "setup.txt"), "w") as f:
        f.write("\n".join(setup) + "\n")
    with open(os.path.join(folder, "ops.txt"), "w") as f:
        f.write("\n".join(ops) + "\n")


if __name__ == "__main__":
    build(sys.argv[1])
    maintained(sys.argv[1])
    rebuilt(sys.argv[1])
    binary(sys.argv[1])
    recalled(sys.argv[1])
    xbase(sys.argv[1])
