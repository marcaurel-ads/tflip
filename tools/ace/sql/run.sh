#!/bin/sh
# Runs sql.exe on a fresh copy of the fixtures, under Wine.
#
#     tools/ace/sql/run.sh <work folder> <ace32.dll as a Windows path> [script [share]]  > statements.json
#
# The script is statements.txt unless named (select.txt makes select.json,
# plans.txt plans.json, dml.txt dml.json with fixtures/sql/dml/ copied too,
# dbf.txt dbf.json, groupconcat.txt groupconcat.json, funcs.txt funcs.json and
# percent.txt percent.json, modulo.txt modulo.json, scripts.txt scripts.json and temptables.txt
# temptables.json in an empty data/, as they make their own tables or
# read none, links.txt links.json and linklist.txt linklist.json in one with
# the dictionaries alone, dbfdict.txt dbfdict.json in one with a copy of
# fixtures/dictionary/dbftables in dbftables/, ddbf.txt ddbf.json in an empty
# one with a cb/ folder for its dictionary; updfrom.txt updfrom.json in an empty one; fts.txt
# fts_plans.json on a copy of fixtures/sql/fts/ alone, and fts_tables.txt the
# fx, px and qx tables of fixtures/sql/fts/ in an empty data/; rowfuncs.txt
# rowfuncs.json, and rowid.txt and rowid2.txt, which give the ROWIDs of
# rowid_paths.json, in an empty data/ too); views.txt
# makes views.json (in a dictionary of its own, vw.add) and orderby.txt
# orderby.json in data/ as it is.
# ddfuncs.txt makes ddfuncs.json in an empty data/ too (its dictionary is df/df.add).
# The work folder gets sql.exe, the script in Windows-1252 and data/, a copy
# of the adt fixtures the scripts use and of fixtures/sql/tables/ (two
# dictionaries in data/props/ and data/dict/, and a copy of the props one in
# data/views/ for spview.txt, which changes it); data/ is rebuilt on every run.
#
# cursors.txt and trycatch.txt make cursors.json and trycatch.json in an empty data/ too.
# cursorlive.txt, cursormore.txt, cursorprocs.txt, cursortrig.txt and temptables_types.txt make
# cursorlive.json, cursormore.json, cursorprocs.json, cursortrig.json and temptables_types.json in
# an empty data/ too (the cursorprocs and cursortrig dictionaries in cp/), and
# so do cursormore2.txt to cursormore5.txt, which make cursormore2.json to cursormore5.json.
# props39.txt makes props39.json in an empty data/ too.
# spcat.txt, spcoll.txt and spcoll2.txt make spcat.json, spcoll.json and
# spcoll2.json in an empty data/ (the catalog and collation procedures);
# spddcat.txt makes spddcat.json in the de/ of the spdd* scripts.
# ddrepl.txt and ddrepl2.txt make ddrepl.json and ddrepl2.json there too.
# ddalter.txt makes ddalter.json in an empty data/ with a folder ct/ for its dictionary.
# ddenc.txt makes ddenc.json on a copy of fixtures/adt/ddenc/ alone (spddencmake.txt made it).
# spql.txt, spql2.txt, spql3.txt and spql6.txt (query logging) and spmg.txt make their .json in an empty
# data/; spddql.txt to spddql4.txt and spddmg.txt in the de/ of the spdd* scripts.
# ddalterdbf.txt makes ddalterdbf.json in an empty data/ with a folder cd/ for its dictionary,
# ddalterfail.txt ddalterfail.json with cf/, and ddalterfree.txt ddalterfree.json on free tables.
# ddslots.txt makes ddslots.json in an empty data/ with a folder ds/ for its dictionary,
# ddrights.txt makes ddrights.json with a folder dr/, ddfieldlen.txt ddfieldlen.json with fl/,
# ddvisible.txt ddvisible.json with vx/, ddlogins.txt ddlogins.json with lg/, ddtrigrights.txt ddtrigrights.json with gq/,
# ddcolrights.txt ddcolrights.json with cq/, ddgrantopt.txt ddgrantopt.json with go/, ddgrantopt2.txt ddgrantopt2.json with g2/, ddfuncrights.txt ddfuncrights.json with fr/, ddadminpk.txt ddadminpk.json with ap/, ddinsertonly.txt ddinsertonly.json with io/, ddinsertonly2.txt ddinsertonly2.json with i2/, ddpublicgrant.txt ddpublicgrant.json with pg/, ddfuncgroup.txt ddfuncgroup.json with fg/, ddbuiltin.txt ddbuiltin.json with bg/, ddapirights.txt ddapirights.json with ar/, ddrenamerights.txt ddrenamerights.json with ar/, ddcreatedbf.txt ddcreatedbf.json with cd2/.
# wherefilter.txt to wherefilter6.txt make wherefilter.json to wherefilter6.json in an empty data/ too.
# ddprocs2.txt makes ddprocs2.json in an empty data/ (its dictionary is dpf/dpf.add);
# params3.txt, guid.txt and planupd.txt make params3.json, guid.json and planupd.json in an empty data/ too (like gram_cast*.txt)
# and spddri2.txt makes spddri2.json in the de/ of the spdd* scripts.
# indexsel.txt, indexsel2.txt, joinorder.txt and joinsize.txt make their .json in an empty data/ too;
# indexsel3.txt runs with run_collation_dynamic.sh (its ger and gen tables).
#
# trigerr.txt makes trigerr.json on a copy of fixtures/adt/psm/ alone, on the
# local server (see its header); trigdb.txt, trigdbfire.txt, trigdbopen.txt and trigdblog*.txt, which make
# their dictionary dbt/dbt.add, run in an empty data/.
#
# triggers.txt makes triggers.json on a copy of fixtures/adt/psm/ alone,
# recorded on the remote server: SQL_SERVER_TYPE=2 (see sql.c) and a fourth
# argument, the server's path to the work folder's data/ (the work folder in
# a folder the server shares).
#
# The transaction scripts are recorded on the remote server the same way
# (SQL_SERVER_TYPE=2 and the share): txscripts.txt, txddl.txt, txlimit.txt and
# txremote.txt (free tables, in an empty data/), txprocs.txt, txtrig.txt (each
# makes its own dictionary with CREATE DATABASE, in a folder of data/ the server
# creates; the script's first lines say which), and txtrig2.txt (a trigger's
# savepoints).
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
work=$1
dll=$2
script=${3:-statements.txt}
share=$4
mkdir -p "$work"
rm -rf "$work/data"
mkdir "$work/data"
cp "$root"/fixtures/adt/all_types/all_types.ad[tm] "$root"/fixtures/adt/adi_keys/keys.ad[ti] \
   "$root"/fixtures/adt/indexes/*.ad[ti] "$root"/fixtures/sql/tables/* "$work/data/"
# The DBF, GROUP_CONCAT, scalar-function and TOP PERCENT scripts run on an
# empty folder, the links one on the dictionaries' subfolders alone.
if [ "$script" = dbf.txt ] || [ "$script" = groupconcat.txt ] || [ "$script" = funcs.txt ] ||
   [ "$script" = percent.txt ] || [ "$script" = modulo.txt ] || [ "$script" = links.txt ] || [ "$script" = linklist.txt ] ||
   [ "$script" = dbfdict.txt ] || [ "$script" = ddtables.txt ] || [ "$script" = ddbf.txt ] ||
   [ "$script" = dbfpk.txt ] || [ "$script" = scripts.txt ] || [ "$script" = sysvars.txt ] || [ "$script" = declare.txt ] || [ "$script" = temptables.txt ] || [ "$script" = temptables_api.txt ] || [ "$script" = temptables_types.txt ] || [ "$script" = temptables_into.txt ] || [ "$script" = temptables_into2.txt ] || [ "$script" = temptables_into3.txt ] || [ "$script" = temptables_into4.txt ] || [ "$script" = temptables_api2.txt ] || [ "$script" = temptables_api3.txt ] ||
   [ "$script" = selinto.txt ] || [ "$script" = rowfuncs.txt ] || [ "$script" = rowid.txt ] || [ "$script" = rowid2.txt ] ||
   [ "$script" = ddopts.txt ] || [ "$script" = ddoptsdict.txt ] || [ "$script" = ddalter.txt ] || [ "$script" = ddalterdbf.txt ] || [ "$script" = ddalterfree.txt ] || [ "$script" = ddalterfail.txt ] ||
   [ "$script" = merge.txt ] || [ "$script" = updfrom.txt ] || [ "$script" = mergedd.txt ] || [ "$script" = ddviews.txt ] ||
   [ "$script" = ddprocs.txt ] || [ "$script" = ddprocs2.txt ] || [ "$script" = ddproclong.txt ] || [ "$script" = limits_procs.txt ] || [ "$script" = trigbig.txt ] || [ "$script" = trignn.txt ] || [ "$script" = dddatabase.txt ] || [ "$script" = refuse.txt ] ||
   [ "$script" = cursors.txt ] || [ "$script" = cursorlive.txt ] || [ "$script" = cursormore.txt ] || [ "$script" = cursorprocs.txt ] || [ "$script" = cursortrig.txt ] || [ "$script" = trycatch.txt ] || [ "$script" = loops.txt ] || [ "$script" = likeorder.txt ] || [ "$script" = statics.txt ] || [ "$script" = derived.txt ] ||
   [ "$script" = udfdict.txt ] || [ "$script" = ddfuncs.txt ] || [ "$script" = props39.txt ] || [ "$script" = procvars.txt ] || [ "$script" = subq.txt ] || [ "$script" = literals.txt ] || [ "$script" = limits.txt ] || [ "$script" = trigdb.txt ] || [ "$script" = trigdbfire.txt ] || [ "$script" = trigdbopen.txt ] || [ "$script" = trigdblog.txt ] || [ "$script" = trigdblog2.txt ] || [ "$script" = trigdblog3.txt ] || [ "$script" = trigself.txt ] || [ "$script" = ddslots.txt ] || [ "$script" = ddrights.txt ] || [ "$script" = ddgrantopt.txt ] || [ "$script" = ddgrantopt2.txt ] || [ "$script" = ddfuncgroup.txt ] || [ "$script" = ddinsertonly.txt ] || [ "$script" = ddinsertonly2.txt ] || [ "$script" = ddinsertonly3.txt ] || [ "$script" = ddpublicgrant.txt ] || [ "$script" = ddpublicbad_make.txt ] || [ "$script" = ddeffperm.txt ] || [ "$script" = ddadminpk.txt ] || [ "$script" = ddfuncrights.txt ] || [ "$script" = ddbuiltin.txt ] || [ "$script" = ddapirights.txt ] || [ "$script" = ddapirights2.txt ] || [ "$script" = ddrenamerights.txt ] || [ "$script" = ddcreatedbf.txt ] || [ "$script" = ddvisible.txt ] || [ "$script" = ddlogins.txt ] || [ "$script" = ddtrigrights.txt ] || [ "$script" = ddcolrights.txt ] || [ "$script" = ddfieldlen.txt ]; then
    rm -rf "$work/data"/*
fi
if [ "$script" = ddtables.txt ] || [ "$script" = ddoptsdict.txt ] || [ "$script" = ddalter.txt ] || [ "$script" = mergedd.txt ]; then
    mkdir "$work/data/ct"
fi
if [ "$script" = ddalterdbf.txt ]; then
    mkdir "$work/data/cd"
fi
if [ "$script" = ddalterfail.txt ]; then
    mkdir "$work/data/cf"
fi
if [ "$script" = selinto.txt ]; then
    mkdir "$work/data/dd"
fi
if [ "$script" = udfdict.txt ]; then
    mkdir "$work/data/uf"
fi
if [ "$script" = ddfuncs.txt ]; then
    mkdir "$work/data/df"
fi
if [ "$script" = procvars.txt ]; then
    mkdir "$work/data/pv"
fi
if [ "$script" = likedict.txt ]; then
    rm -rf "$work/data"/*
    mkdir "$work/data/ld"
fi
if [ "$script" = ddviews.txt ]; then
    mkdir "$work/data/dv"
fi
if [ "$script" = ddproclong.txt ]; then
    mkdir "$work/data/dpl"
fi
if [ "$script" = ddprocs.txt ]; then
    mkdir "$work/data/dp"
fi
if [ "$script" = ddslots.txt ]; then
    mkdir "$work/data/ds"
fi
if [ "$script" = ddrenamerights.txt ]; then
    mkdir "$work/data/ar"
fi
if [ "$script" = ddcreatedbf.txt ]; then
    mkdir "$work/data/cd2"
fi
if [ "$script" = ddapirights.txt ]; then
    mkdir "$work/data/ar"
fi
if [ "$script" = ddapirights2.txt ]; then
    mkdir "$work/data/sr"
fi
if [ "$script" = ddbuiltin.txt ]; then
    mkdir "$work/data/bg"
fi
if [ "$script" = ddgrantopt.txt ]; then
    mkdir "$work/data/go"
fi
if [ "$script" = ddfuncrights.txt ]; then
    mkdir "$work/data/fr"
fi
if [ "$script" = ddadminpk.txt ]; then
    mkdir "$work/data/ap"
fi
if [ "$script" = ddinsertonly.txt ]; then
    mkdir "$work/data/io"
fi
if [ "$script" = ddinsertonly2.txt ]; then
    mkdir "$work/data/i2"
fi
if [ "$script" = ddinsertonly3.txt ]; then
    mkdir "$work/data/i3"
fi
# The dictionaries whose DB:Public slot does not check are made once, see
# ddpublicbad_make.txt; the script runs on a copy of the kept one.
if [ "$script" = ddpublicbad.txt ] || [ "$script" = ddpublicbad2.txt ] || [ "$script" = ddpublicbad3.txt ] || [ "$script" = ddpublicbad4.txt ] || [ "$script" = ddpublicbad5.txt ] || [ "$script" = ddpublicbad6.txt ] || [ "$script" = ddpublicbad7.txt ] || [ "$script" = ddpublicbad8.txt ] || [ "$script" = ddpublicbad9.txt ] || [ "$script" = ddpublicbad10.txt ] || [ "$script" = ddexecrights.txt ]; then
    rm -rf "$work/data"/*
    cp "$root/fixtures/sql/${script%.txt}"/* "$work/data/"
fi
if [ "$script" = ddpublicgrant.txt ]; then
    mkdir "$work/data/pg"
fi
if [ "$script" = ddfuncgroup.txt ]; then
    mkdir "$work/data/fg"
fi
if [ "$script" = ddgrantopt2.txt ]; then
    mkdir "$work/data/g2"
fi
if [ "$script" = ddrights.txt ]; then
    mkdir "$work/data/dr"
fi
if [ "$script" = ddfieldlen.txt ]; then
    mkdir "$work/data/fl"
fi
if [ "$script" = ddvisible.txt ]; then
    mkdir "$work/data/vx"
fi
if [ "$script" = ddlogins.txt ]; then
    mkdir "$work/data/lg"
fi
if [ "$script" = ddtrigrights.txt ]; then
    mkdir "$work/data/gq"
fi
if [ "$script" = ddcolrights.txt ]; then
    mkdir "$work/data/cq"
fi
if [ "$script" = ddprocs2.txt ]; then
    mkdir "$work/data/dpf"
fi
if [ "$script" = trigdb.txt ] || [ "$script" = trigdbfire.txt ] || [ "$script" = trigdbopen.txt ] || [ "$script" = trigdblog.txt ] || [ "$script" = trigdblog2.txt ] || [ "$script" = trigdblog3.txt ] || [ "$script" = trigself.txt ]; then
    mkdir "$work/data/dbt"
fi
if [ "$script" = trigbig.txt ]; then
    mkdir "$work/data/bgd"
fi
if [ "$script" = limits_procs.txt ]; then
    mkdir "$work/data/lp"
fi
if [ "$script" = txtrig3.txt ]; then
    mkdir "$work/data/zr"
fi
if [ "$script" = trignn.txt ]; then
    mkdir "$work/data/zq"
fi
if [ "$script" = dddatabase.txt ]; then
    mkdir "$work/data/dc"
fi
if [ "$script" = gram_script.txt ]; then
    mkdir "$work/data/gs"
fi
if [ "$script" = ddbf.txt ]; then
    mkdir "$work/data/cb"
fi
if [ "$script" = cursorprocs.txt ] || [ "$script" = cursortrig.txt ] || [ "$script" = loops.txt ]; then
    mkdir "$work/data/cp"
fi
# The system-procedure scripts (spt*, spm*, spcat, spcoll*), iif.txt and the
# grammar ones (gram_*) make their own tables.
case "$script" in claims*.txt | spt*.txt | spm*.txt | spcat.txt | spcoll*.txt | spql*.txt | iif.txt | gram_*.txt | wherefilter*.txt | castchar2.txt | tsarith.txt | tsarith2.txt | staticorder.txt | indexsel.txt | indexsel2.txt | joinorder.txt | joinsize.txt | txscripts.txt | txddl.txt | txlimit.txt | txremote.txt | txprocs.txt | txtrig.txt | params3.txt | guid.txt | planupd.txt | txtrig2.txt | txtrig3.txt) rm -rf "$work/data"/* ;; esac
# The DBF dictionary script reads a copy of fixtures/dictionary/dbftables.
if [ "$script" = dbfdict.txt ]; then
    mkdir "$work/data/dbftables"
    cp "$root"/fixtures/dictionary/dbftables/*.* "$work/data/dbftables/"
fi
# The DML script changes a table of its own.
if [ "$script" = dml.txt ]; then
    cp "$root"/fixtures/sql/dml/* "$work/data/"
fi
# The full-text script reads the full-text tables alone; the script that made
# the ones of them it adds runs on an empty folder.
if [ "$script" = oem.txt ] || [ "$script" = oemrows.txt ]; then
    rm -rf "$work/data"/*
    cp "$root"/fixtures/ntx/charset_default/co.dbf "$work/data/"
fi
if [ "$script" = fts.txt ] || [ "$script" = fts_tables.txt ]; then
    rm -rf "$work/data"/*
fi
if [ "$script" = fts.txt ]; then
    cp "$root"/fixtures/sql/fts/* "$work/data/"
fi
# The user-defined functions script reads and changes a copy of the dictionary
# udfdict.txt made.
if [ "$script" = udf.txt ]; then
    rm -rf "$work/data"/*
    cp "$root"/fixtures/sql/udf/* "$work/data/"
fi
# The dictionary encryption script reads copies of the dictionaries
# spddencmake.txt made.
if [ "$script" = ddenc.txt ]; then
    rm -rf "$work/data"/*
    cp "$root"/fixtures/adt/ddenc/* "$work/data/"
fi
# The triggers script changes a copy of the psm dictionary.
if [ "$script" = triggers.txt ] || [ "$script" = execproc.txt ] || [ "$script" = trigvars.txt ] || [ "$script" = trigtyped.txt ] || [ "$script" = trigerr.txt ] || [ "$script" = trigproc.txt ] || [ "$script" = trigproc2.txt ] || [ "$script" = trigorder.txt ] || [ "$script" = trigorder2.txt ]; then
    rm -rf "$work/data"/*
    cp "$root"/fixtures/adt/psm/*.a[dim]* "$work/data/"
fi
# The other live-cursor scripts (cursormore2.txt to cursormore5.txt) make their own tables too.
if [ "$script" = cursormore2.txt ] || [ "$script" = cursormore3.txt ] || [ "$script" = cursormore4.txt ] || [ "$script" = cursormore5.txt ]; then
    rm -rf "$work/data"/*
fi
mkdir "$work/data/props" "$work/data/dict" "$work/data/views"
cp "$root"/fixtures/adt/dictionary_props/*.a[dim]* "$work/data/props/"
cp "$root"/fixtures/adt/dictionary_props/*.a[dim]* "$work/data/views/"
cp "$root"/fixtures/adt/dictionary/*.a[dim]* "$work/data/dict/"
# The system procedures of the dictionary objects (spdd*.txt) run in an empty
# folder with a folder de/ for the dictionary the script creates, and the
# folders below it that files are moved to.
case "$script" in spdd*.txt|ddrepl.txt|ddrepl2.txt)
    rm -rf "$work/data"/*
    mkdir "$work/data/de"
    mkdir "$work/data/de/sub" "$work/data/de/sub2" "$work/data/de/sub/deep"
    ;;
esac
i686-w64-mingw32-gcc -O1 -Wall -o "$work/sql.exe" "$here/sql.c"
iconv -f utf-8 -t cp1252 "$here/$script" > "$work/$script"
cd "$work"
# Today's date (CURDATE() in dml.txt) is written as <today>, which the
# replay puts in for its own day.
WINEDEBUG=-all wine sql.exe "$dll" "${share:-Z:$(echo "$work/data" | tr / '\\')}" "$script" |
    sed "s#\"$(date +%m/%d/%Y)\"#\"<today>\"#g"
