#!/usr/bin/env python3
"""Writes the setup.txt and ops.txt of the scenarios of full-text orders in
transactions for tools/ace/write/run.sh, which ran on the remote server (see
that script):

- fts_tx_rollback, fts_tx_commit: tables with words in a memo and a
  character field and full-text orders on both, written in a transaction
  that is rolled back or committed (one transaction per table, as the
  original's memo files hold the blocks a transaction frees apart from the
  next one's writes): appended, changed and deleted by the API, by SQL, with
  a savepoint, and with changes pending when it ends; what CONTAINS finds
  on the connection that writes and on another meanwhile.
- fts_tx_big_rollback, fts_tx_big_commit: a table of character fields large
  enough for the orders' leaves to split, written at random.
- fts_tx_memo_rollback, fts_tx_memo_commit: the same with a memo field and
  fewer writes.
- fts_tx_cdx_*, fts_tx_vfp_*: the first scenario on FoxPro and Visual FoxPro
  tables (a DBF record's words are taken once, at the commit, from its
  original to its final image); the files only, no CONTAINS.

    tools/ace/write/fts_tx.py fixtures/write
"""
import os
import random
import sys


def write(out, name, setup, ops):
    folder = os.path.join(out, name)
    os.makedirs(folder, exist_ok=True)
    with open(os.path.join(folder, "setup.txt"), "w") as f:
        f.write("\n".join(setup) + "\n")
    with open(os.path.join(folder, "ops.txt"), "w") as f:
        f.write("\n".join(ops) + "\n")


class Script:
    """The lines of an ops.txt, counting the tables the original numbers."""

    def __init__(self, kind=None):
        self.lines = []
        self.count = 0
        self.here = 0
        self.kind = kind

    def add(self, *lines):
        self.lines += lines

    def open(self, table):
        """Opens the table on both connections and makes the first current."""
        if self.kind:
            # One connection: a DBF memo value goes to a new block when another
            # connection's handle stands on its record, in a transaction or not.
            self.add("open %s.dbf shared" % table)
            self.here = self.count + 1
            self.count += 1
            return
        self.add("open %s shared" % table, "open2 %s shared" % table)
        self.here = self.count + 1
        self.count += 2
        self.add("use %d" % self.here)

    def query(self, table, words):
        """What CONTAINS finds for each word, on the first connection and the
        second (not on a FoxPro or VFP table: tflip's SQL finds nothing there
        yet, so those scenarios compare the files)."""
        if self.kind:
            return
        for field, word in words:
            select = "SELECT id FROM %s WHERE CONTAINS(%s, '%s') ORDER BY id" % (table, field, word)
            for cursor in ["cursor", "cursor2"]:
                self.add("%s %s" % (cursor, select), "column id", "closecursor")
                self.count += 1
                self.add("use %d" % self.here)


def table(name):
    return [
        "sql CREATE TABLE %s (id INTEGER, body MEMO, c CHAR(40))" % name,
        "sql INSERT INTO %s VALUES (1, 'The quick brown fox jumps', 'fox den')" % name,
        "sql INSERT INTO %s VALUES (2, 'lazy dog sleeps by the river', 'dog house')" % name,
        "sql INSERT INTO %s VALUES (3, 'fox and dog together again', 'fox dog')" % name,
        "sql INSERT INTO %s VALUES (4, 'repeat repeat repeat', NULL)" % name,
        "sql INSERT INTO %s VALUES (5, NULL, 'only char')" % name,
        "sql CREATE INDEX %s_fts ON %s (body) CONTENT" % (name, name),
        "sql CREATE INDEX %s_score ON %s (body) CONTENT KEEPSCORE" % (name, name),
        "sql CREATE INDEX %s_c ON %s (c) CONTENT" % (name, name),
        "sql CREATE INDEX %s_id ON %s (id)" % (name, name),
    ]


def small(out, end, kind=None):
    """The scenario on ADT tables, or on FoxPro (`cdx`) or Visual FoxPro
    (`vfp`) ones."""
    setup = ["tabletype " + kind] if kind else []
    for name in ["ta", "tq", "tp", "tn"]:
        setup += table(name)
    watch = [("body", "fox"), ("body", "zebra"), ("body", "newword"), ("c", "zebra"),
             ("c", "fox"), ("body", "repeat"), ("body", "dog")]
    s = Script(kind)
    if kind:
        s.add("tabletype " + kind)
    s.add("# Full-text orders kept current in a transaction: what CONTAINS finds",
          "# on the connection that writes and on another, and the files after the",
          "# %s." % end)
    s.add("# An append, a change of each field and a delete, by the API.")
    s.open("ta")
    s.query("ta", watch)
    s.add("begin",
          "append",
          "set id long 6", "set body string zebra and fox", "set c string zebra stripes",
          "write",
          "flush", "snapshot appended")
    s.query("ta", watch)
    s.add("goto 1", "set body string newword here", "write",
          "goto 3", "set c string zebra", "write",
          "flush", "snapshot changed")
    s.query("ta", watch)
    s.add("goto 2", "delete", "write",
          "goto 4", "set body string newword", "write",
          "flush", "snapshot deleted")
    s.query("ta", watch)
    s.add(end, "flush", "snapshot " + end)
    s.query("ta", watch)
    s.add("state", "rows")
    s.add("# SQL's statements.")
    s.open("tq")
    s.add("begin",
          "sql INSERT INTO tq VALUES (10, 'zebra crossing', 'zebra')",
          "sql UPDATE tq SET body = 'fox and zebra' WHERE id = 5",
          "sql DELETE FROM tq WHERE id = 3",
          "flush", "snapshot sql")
    s.query("tq", watch)
    s.add(end, "flush", "snapshot sql-" + end)
    s.query("tq", watch)
    s.add("# A savepoint.")
    s.open("tp")
    saved = [("body", "savepoint"), ("body", "one"), ("body", "two"), ("body", "river")]
    s.add("begin",
          "goto 1", "set body string savepoint one", "write",
          "savepoint s",
          "goto 2", "set body string savepoint two", "write",
          "goto 5", "delete", "write",
          "rollbackto s",
          "flush", "snapshot savepoint-rolled")
    s.query("tp", saved)
    s.add(end, "flush", "snapshot savepoint-" + end)
    s.query("tp", saved)
    s.add("# A change pending when the transaction ends.")
    s.open("tn")
    s.add("begin", "goto 4", "set body string pending words", end,
          "flush", "snapshot pending-" + end)
    s.query("tn", [("body", "pending"), ("body", "repeat")])
    write(out, "fts_tx_%s%s" % (kind + "_" if kind else "", end), setup, s.lines)


def big(out, end, memo):
    """Records written at random: in character fields alone (`fts_tx_big_*`,
    enough for the orders' leaves to split), or with a memo field and fewer
    changes (`fts_tx_memo_*`)."""
    rng = random.Random(31)
    vowels, consonants = "aeiou", "bcdfghjklmnprstvwz"
    vocabulary = ["".join(rng.choice(consonants if i % 2 == 0 else vowels)
                          for i in range(rng.randrange(3, 9))) for _ in range(150)]

    def text(long=False):
        return " ".join(rng.choice(vocabulary) for _ in range(rng.randrange(1, 12)))[:55].strip()

    def long_text():
        return " ".join(rng.choice(vocabulary) for _ in range(rng.randrange(1, 12)))

    first = "body" if memo else "d"
    kind = "MEMO" if memo else "CHAR(80)"
    value = long_text if memo else text
    records = 40 if memo else 160
    appends, firsts, seconds, deletes = (12, 8, 6, 5) if memo else (60, 40, 30, 20)
    setup = ["sql CREATE TABLE t (id INTEGER, %s %s, c CHAR(60))" % (first, kind)]
    rows = ["(%d, '%s', '%s')" % (i, value(), text()) for i in range(1, records + 1)]
    for start in range(0, len(rows), 40):
        setup.append("sql INSERT INTO t VALUES " + ", ".join(rows[start:start + 40]))
    setup += ["sql CREATE INDEX t_fts ON t (%s) CONTENT" % first,
              "sql CREATE INDEX t_score ON t (%s) CONTENT KEEPSCORE" % first,
              "sql CREATE INDEX c_fts ON t (c) CONTENT",
              "sql CREATE INDEX t_id ON t (id)"]
    s = Script()
    s.open("t")
    s.add("# Appends, changes and deletes at random, then a %s." % end, "begin")
    for i in range(appends):
        s.add("append", "set id long %d" % (1000 + i), "set %s string %s" % (first, value()),
              "set c string " + text(), "write")
    for _ in range(firsts):
        s.add("goto %d" % rng.randrange(1, records + 1), "set %s string %s" % (first, value()),
              "write")
    for _ in range(seconds):
        s.add("goto %d" % rng.randrange(1, records + 1), "set c string " + text(), "write")
    # Each record at most once, so that none is deleted twice.
    for record in rng.sample(range(1, records + 1), deletes):
        s.add("goto %d" % record, "delete", "write")
    watch = [(first, vocabulary[0]), ("c", vocabulary[1])]
    s.add("flush", "snapshot written")
    s.query("t", watch)
    s.add(end, "flush", "snapshot " + end)
    s.query("t", watch)
    write(out, "fts_tx_%s_%s" % ("memo" if memo else "big", end), setup, s.lines)


if __name__ == "__main__":
    out = sys.argv[1]
    for end in ["rollback", "commit"]:
        small(out, end)
        big(out, end, False)
        big(out, end, True)
        small(out, end, "cdx")
        small(out, end, "vfp")
