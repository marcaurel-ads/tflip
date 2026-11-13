#!/usr/bin/env python3
"""Writes the cases files seek.exe runs (see seek.c), from the readable lists
below: text is written as hex of its Windows-1252 bytes (UTF-16LE for the
`w` commands).

    tools/ace/seek/cases.py <out folder>

writes `collation.txt` (seeks and scopes in the charset_german `ca` tables,
ADT, NTX and CDX, under ANSI_CHAR_SET=German, and in its OEM `co` tables),
`oem.txt` (the OEM seeks alone, for the charset_default tables) and
`nchar.txt` (seeks in the adi_keys `keys` table's NChar and NVarChar orders)."""
import os
import sys


def hx(text, wide=False):
    if text == "":
        return "-"
    return text.encode("utf-16-le" if wide else "cp1252").hex()


WORDS = ["", " ", "a", "A", "apfel", "Apfel", "APFEL", "äpfel", "Äpfel", "ä", "Ä", "å",
         "ap", "aq", "b", "n", "ñ", "o", "ö", "öre", "p", "resum", "resume", "resumé",
         "resumes", "resumf", "s", "ss", "ß", "st", "t", "z", "Z", "zebra", "ÿ", "þ"]
CHARS = ["a", "A", "ä", "Ä", "b", "z", "Z", "[", "ß", "s", "t", "ü", "ÿ", "0", "~"]
SCOPES = [("a", "b"), ("ä", "ö"), ("Ä", "z"), ("n", "s"), ("ss", "ß"), ("A", "Z")]


def collation_cases(out):
    for label, opens, cw, cc in [
        ("ADT", "open 3 1 adt\\ca.adt", "CA_W", "CA_C"),
        ("NTX", "open 1 1 ntx\\ca.dbf ntx\\ca_c.ntx ntx\\ca_w.ntx", "ca_w", "ca_c"),
        ("CDX", "open 2 1 cdx\\ca.dbf", "CA_W", "CA_C"),
    ]:
        out.append(f"# {label}")
        out.append(opens)
        for word in WORDS:
            for how in ("soft", "hard", "after"):
                out.append(f"seek {cw} {how} {hx(word)}")
            out.append(f"last {cw} {hx(word)}")
        for char in CHARS:
            for how in ("soft", "hard", "after"):
                out.append(f"seek {cc} {how} {hx(char)}")
            out.append(f"last {cc} {hx(char)}")
        for top, bottom in SCOPES:
            out.append(f"scope {cw} top {hx(top)}")
            out.append(f"scope {cw} bottom {hx(bottom)}")
            out.append(f"walk {cw}")
            for word in ("a", "ä", "n", "s", "ß", "z"):
                out.append(f"seek {cw} soft {hx(word)}")
            out.append(f"clear {cw} top")
            out.append(f"clear {cw} bottom")
        for top, bottom in [("a", "b"), ("ä", "ü"), ("A", "Z")]:
            out.append(f"scope {cc} top {hx(top)}")
            out.append(f"scope {cc} bottom {hx(bottom)}")
            out.append(f"walk {cc}")
            out.append(f"clear {cc} top")
            out.append(f"clear {cc} bottom")
    oem_cases(out)


def oem_cases(out):
    """OEM tables: keys in the OEM code page (weighed by OEM_CHAR_SET)."""
    for label, opens, cw in [
        ("NTX OEM", "open 1 2 ntx\\co.dbf ntx\\co_c.ntx ntx\\co_w.ntx", "co_w"),
        ("CDX OEM", "open 2 2 cdx\\co.dbf", "CO_W"),
    ]:
        out.append(f"# {label}")
        out.append(opens)
        for word in ["", "a", "A", "apfel", "Apfel", "ä", "Ä", "Äpfel", "n", "ö", "öre",
                     "resum", "resumé", "s", "ss", "ß", "z", "Z"]:
            for how in ("soft", "hard", "after"):
                out.append(f"seek {cw} {how} {hx(word)}")
            out.append(f"last {cw} {hx(word)}")
        for top, bottom in [("a", "b"), ("ä", "ö"), ("n", "s")]:
            out.append(f"scope {cw} top {hx(top)}")
            out.append(f"scope {cw} bottom {hx(bottom)}")
            out.append(f"walk {cw}")
            out.append(f"clear {cw} top")
            out.append(f"clear {cw} bottom")


NTEXT = ["", " ", "a", "A", "ab", "abc", "abd", "ABC", "b", "z", "Z", "zz", "Zz", "ZZ",
         "Ω", "Ωж", "Ωж€", "ω", "ж", "€", "ä", "aa", "a b", "  ", "   ",
         "a ", "a  ", "ab ", "abc ", "abcd", "äöü", "Ω ", "\t", "Zz "]


def nchar_cases(out):
    out.append("open 3 1 adt\\keys.adt")
    for order in ("I_NCHAR", "I_NVARCHAR"):
        out.append(f"walk {order}")
        for record in range(1, 7):
            out.append(f"key {order} {record}")
        for text in NTEXT:
            for how in ("soft", "hard", "after"):
                try:
                    out.append(f"seek {order} {how} {hx(text)}")
                except UnicodeEncodeError:
                    pass  # not in Windows-1252
                out.append(f"wseek {order} {how} {hx(text, True)}")
            out.append(f"wlast {order} {hx(text, True)}")
        for top, bottom in [("a", "b"), ("ab", "zz"), ("Z", "Ω"), ("", "a")]:
            out.append(f"wscope {order} top {hx(top, True)}")
            out.append(f"wscope {order} bottom {hx(bottom, True)}")
            out.append(f"walk {order}")
            out.append(f"clear {order} top")
            out.append(f"clear {order} bottom")


def main():
    folder = sys.argv[1]
    for name, make in [("collation.txt", collation_cases), ("nchar.txt", nchar_cases),
                       ("oem.txt", oem_cases)]:
        out = []
        make(out)
        with open(os.path.join(folder, name), "w") as f:
            f.write("\n".join(out) + "\n")


main()
