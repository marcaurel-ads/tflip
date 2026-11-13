#!/usr/bin/env python3
"""Writes fixtures/write/tagdir{,_drop}/{setup,ops}.txt: tag directories of
more than one page (an ADI file's and a CDX file's), for tools/ace/write/run.sh.

    tools/ace/write/tagdir.py fixtures/write

`tagdir` makes the 256 tags a file takes and tries a 257th; `tagdir_drop`
starts with 100 tags in each file (more than a leaf of the directory holds),
opens them, drops them in two sweeps and makes new ones, and keeps writing
records through what is left.
"""
import os
import sys

TYPES = [("adt", "t.adt"), ("cdx", "t.dbf")]


def create(count):
    return "".join("createindex - o%d 2 512 a\n" % i for i in range(1, count + 1))


def write(folder, name, setup, ops):
    os.makedirs(os.path.join(folder, name), exist_ok=True)
    for file, text in (("setup.txt", setup), ("ops.txt", ops)):
        with open(os.path.join(folder, name, file), "w") as f:
            f.write(text)


def tables(body):
    return "".join(
        "tabletype %s\n%s" % (t, body(t, f)) for t, f in TYPES
    )


folder = sys.argv[1]
fields = "a,Character,4;b,Integer"
write(
    folder,
    "tagdir",
    tables(lambda t, f: "createtable %s 0 %s\nclose\n" % (f, fields)),
    tables(lambda t, f: "open %s\n%sclose\n" % (f, create(257))),
)
write(
    folder,
    "tagdir_drop",
    tables(lambda t, f: "createtable %s 0 %s\nopen %s\n%sclose\n" % (f, fields, f, create(100))),
    tables(
        lambda t, f: "open %s\n" % f
        + "".join("order o%d\n" % i for i in (1, 37, 50, 64, 99, 100, 101))
        + "append\nset a string mid\nset b long 1\nwrite\norder o64\ntop\nget a\norder o100\ntop\nget a\n"
        + "".join("deleteindex o%d\n" % i for i in range(1, 101, 3))
        + "createindex - n1 2 512 a\ncreateindex - o4 2 512 a\n"
        + "".join("deleteindex o%d\n" % i for i in range(1, 101) if i % 3 != 1)
        + "createindex - n2 2 512 a\ncreateindex - o1 2 512 a\nclose\n"
        + "open %s\nappend\nset a string abc\nset b long 5\nwrite\nappend\nset a string abd\n"
        % f
        + "set b long 6\nwrite\norder o4\ntop\nget a\nseek 2 abd\nget a\nbottom\nget a\n"
        + "order n2\ntop\nget a\norder o9\nclose\n"
    ),
)
