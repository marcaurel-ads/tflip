#!/usr/bin/env python3
"""Runs sysdefault.exe under Wine once per locale, with ANSI_CHAR_SET and
OEM_CHAR_SET empty, and prints what the engine did as JSON
(fixtures/collation/sysdefault.json).

    tools/ace/collation/sysdefault.py <acesdk folder> <work folder> <locale>... > sysdefault.json

The acesdk folder holds ace32.dll and its runtime files (adsloc32.dll,
aicu32.dll, ansi.chr, ...). The local server reads adslocal.cfg from the DLL's
folder, so the run copies those files into an "engine" folder of its own with
an adslocal.cfg that leaves both settings empty. Each locale (a Unix one such
as de_DE.UTF-8, which Wine turns into the Windows locale) runs with LC_ALL set
to it, in the Wine prefix of $WINEPREFIX. A locale written NAME=VALUE (e.g.
en_US.UTF-8:ANSI_CHAR_SET=German) sets that adslocal.cfg value too.
"""
import json, os, shutil, subprocess, sys

here = os.path.dirname(os.path.abspath(__file__))
acesdk, work = sys.argv[1], sys.argv[2]
os.makedirs(work, exist_ok=True)
exe = os.path.join(work, "sysdefault.exe")
subprocess.run(["i686-w64-mingw32-gcc", "-O1", "-Wall", "-o", exe, os.path.join(here, "sysdefault.c")],
               check=True)
engine = os.path.join(work, "engine")
shutil.rmtree(engine, ignore_errors=True)
os.makedirs(engine)
for f in os.listdir(acesdk):
    if f.lower() != "adslocal.cfg" and os.path.isfile(os.path.join(acesdk, f)):
        shutil.copy(os.path.join(acesdk, f), engine)


def win(path):
    return "Z:" + os.path.abspath(path).replace("/", "\\")


runs = []
for spec in sys.argv[3:]:
    locale, _, setting = spec.partition(":")
    settings = {"ANSI_CHAR_SET": "", "OEM_CHAR_SET": ""}
    if setting:
        key, _, value = setting.partition("=")
        settings[key] = value
    with open(os.path.join(engine, "adslocal.cfg"), "w", newline="") as cfg:
        cfg.write("[SETTINGS]\r\n" + "".join("%s=%s\r\n" % kv for kv in settings.items()))
    data = os.path.join(work, "data")
    shutil.rmtree(data, ignore_errors=True)
    os.makedirs(data)
    env = dict(os.environ, LC_ALL=locale, WINEDEBUG="-all")
    out = subprocess.run(["wine", exe, win(os.path.join(engine, "ace32.dll")), win(data)], env=env,
                         capture_output=True, text=True, timeout=120).stdout
    lines = dict(line.split(" ", 1) for line in out.replace("\r", "").splitlines() if " " in line)
    run = {"locale": locale, "settings": settings}
    run["windows"] = dict(kv.split("=") for kv in lines["lcid"].split())
    for key in ("ansi", "oem", "weights", "upper", "lower"):
        run[key] = lines[key]
    run["crc"] = int(lines["crc"])
    runs.append(run)
    print(locale, run["windows"]["user"], file=sys.stderr)

json.dump({
    "note": "Static collations the engine picks with ANSI_CHAR_SET and OEM_CHAR_SET empty, per host "
            "locale (tools/ace/collation/sysdefault.py, under Wine). Bytes 1..255, as hex: ansi and oem "
            "are the bytes in an NTX order's order (a DBF opened ANSI and OEM), weights each byte's ADI "
            "key, upper and lower what UPPER(c)/LOWER(c) evaluate to on an ADT table, crc the collation "
            "CRC the ADI file records at 0x14.",
    "runs": runs,
}, sys.stdout, indent=1)
print()
