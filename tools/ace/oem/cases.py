"""Writes oem.txt, the script for oem.c (see there), into the folder given as
the first argument. Client text is Windows-1252 bytes or UTF-16 or UTF-8 as
the commands take it. The OEM tables hold the characters 0x20 to 0xFF of
code page 437 in their C field (record n holds 0x1F + n), and in record 226
"resum" and 0x82 in W."""
import sys


def a(s):
    return s.encode("cp1252").hex()


def w(s):
    return s.encode("utf-16-le").hex() + "0000"


def u(s):
    return s.encode("utf-8").hex()


def q(s):
    return "'" + s + "'"


lines = ["open 1 ntx/co.dbf"]
for ch in "äöüßéÄÖÜáñ¿ªº«»¢£¥ƒ°±²µ·€žŒœ™":
    lines.append('clear')
    lines.append(f"locate {a('C=' + q(ch))}")
    lines.append(f"filter {a('C=' + q(ch))}")
lines.append(f"filter {a('C>' + q('z') + '.AND.C<' + q('ä'))}")
lines.append(f"filter {a('C $ ' + q('xäx'))}")
lines.append(f"filter {a('W=' + q('resumé'))}")
lines.append(f"filter {a('W=' + q('resum') + '+CHR(130)')}")
lines.append(f"filter {a('C=CHR(132)')}")
lines.append(f"filter {a('UPPER(C)=' + q('Ä'))}")
lines.append(f"filter {a('LEN(' + q('ä') + ')=1')}")
lines.append("clear")
lines.append(f"aof {a('C=' + q('ä'))}")
lines.append("getaof")
lines.append("inaof 197")
lines.append("inaof 197")
lines.append("clear")
lines.append(f"filter {a('C=' + q('ä'))}")
lines.append("getfilter")
lines.append("clear")
for ch in "äß░─│α±€中éẞ":
    lines.append(f"wfilter {w('C=' + q(ch))}")
for ch in "äß░":
    lines.append(f"ufilter {u('C=' + q(ch))}")
lines.append("clear")
for ch in "ä░€ö":
    lines.append("goto 197")
    lines.append(f"weval {w('C=' + q(ch))}")
lines.append("goto 197")
lines.append("eval " + a("C='ä'"))
lines.append("eval " + a("C='ö'"))
lines.append("eval " + b"C='\x84'".hex())
lines.append("eval " + a("C=CHR(228)"))
lines.append("eval " + a("C=CHR(132)"))
lines.append("str " + a("C"))
lines.append("goto 226")
lines.append("str " + a("W"))
lines.append("str " + a("W+'ä'"))
lines.append("str " + a("'ä'"))
lines.append("str " + a("'äöüßé€'"))
lines.append("str " + a("UPPER('äé')"))
lines.append("str " + a("UPPER(W)"))
lines.append("str " + a("LOWER('ÄÉ')"))
lines.append("str " + a("CHR(132)"))
lines.append("str " + a("SUBSTR(W,6,1)"))
lines.append("goto 197")
lines.append("str " + a("C+'ä'"))
lines.append("valid " + a("C='ä'"))
for expr in ["'░'='?'", "'░'=CHR(0)", "'░'=' '", "'░'=''", "LEN('░')=1", "'░'<'a'", "'░'>'a'", "'░'<CHR(1)"]:
    lines.append(f"weval {w(expr)}")
for expr in ["'€'='€'", "'€'='?'", "'a€'='a?'", "LEN('a€b')=3"]:
    lines.append(f"weval {w(expr)}")
for ch, n in [("░", 176), ("α", 224), ("é", 130), ("─", 196), ("│", 179), ("ß", 225), ("±", 241)]:
    for byte in (n, n + 1):
        lines.append(f"weval {w(f'{q(ch)}=CHR({byte})')}")
lines.append(f"wfilter {w('C=' + q('ä'))}")
lines.append("getfilter")
lines.append(f"wfilter {w('C=' + q('░'))}")
lines.append("getfilter")
lines.append("clear")
lines.append(f"ufilter {u('C=' + q('ä'))}")
lines.append("getfilter")
lines.append("clear")
lines.append(f"aof {a('C=' + q('€'))}")
lines.append("getaof")
lines.append("clear")
lines.append("goto 197")
lines.append(f"index ixa.ntx {a('C+' + q('ä'))} -")
lines.append(f"index ixb.ntx {a('C')} {a('C>' + q('ä'))}")
lines.append(f"index ixc.ntx {a('UPPER(W)+' + q('éß'))} -")
open(sys.argv[1] + "/oem.txt", "w").write("\n".join(lines) + "\n")
