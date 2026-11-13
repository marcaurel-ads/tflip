#!/bin/sh
# Dumps a table with adsdump.exe ($ADSDUMP, see LOCAL.md) under Wine:
#
#     tools/ace/backup/dump.sh <Windows path of the table> <adt|cdx|vfp|ntx> [dict.add [user [password]]]
#
# The error log of the evaluation server is C:\ads_err.adt.
export WINEPREFIX=${WINEPREFIX:-$HOME/src/ace-sdk}
export WINEDEBUG=-all
exec wine "${ADSDUMP:?set ADSDUMP to adsdump.exe (LOCAL.md)}" "$@"
