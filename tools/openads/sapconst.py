"""Rewrite OpenADS header constants to SAP's values.

    python3 -I sapconst.py <SAP ace.h> <header> [<header> ...]

Every `#define NAME <number>` and every enum entry `NAME = <number>,` whose
NAME SAP's ace.h also defines as a number gets SAP's value. Prints what it
changed.
"""
import re
import sys

NUM = r"\(?\s*(-?(?:0x[0-9A-Fa-f]+|\d+))[uUlL]*\s*\)?"
DEFINE = re.compile(r"^(\s*#\s*define\s+((?:ADS|AE)_\w+)\s+)" + NUM + r"(.*)$")
ENUM = re.compile(r"^(\s*((?:ADS|AE)_\w+)\s*=\s*)" + NUM + r"(\s*,?.*)$")


def sap_values(path):
    vals = {}
    with open(path, encoding="latin-1") as f:
        for line in f:
            m = DEFINE.match(line)
            if m:
                vals.setdefault(m.group(2), int(m.group(3), 0))
    return vals


def main():
    sap = sap_values(sys.argv[1])
    for path in sys.argv[2:]:
        with open(path, encoding="latin-1") as f:
            lines = f.readlines()
        out = []
        for line in lines:
            body = line.rstrip("\n")
            for rx in (DEFINE, ENUM):
                m = rx.match(body)
                if m and m.group(2) in sap and int(m.group(3), 0) != sap[m.group(2)]:
                    print(f"{path.rsplit('/', 1)[-1]}: {m.group(2)} {m.group(3)} -> {sap[m.group(2)]}")
                    body = f"{m.group(1)}{sap[m.group(2)]}{m.group(4)}"
                    break
            out.append(body + "\n")
        with open(path, "w", encoding="latin-1") as f:
            f.writelines(out)


main()
