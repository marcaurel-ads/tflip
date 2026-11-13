#!/bin/sh
# Rebuilds fixtures/dictionary/deploy/ with a real Advantage Client Engine,
# under Wine: what AdsDDDeployDatabase does to a folder of free tables for a
# source dictionary, in the scenarios deploy.c makes. Clean-room: only the
# official ACE is called, and tflip's own output is compared to the result.
#
#     tools/ace/dd/deploy.sh <work folder> <ace32.dll as a Windows path>
#
# With SCENARIOS set to a list of scenario names, only those are made and
# put in the fixtures, beside the ones that are there (the databases' login
# secrets are made anew by each run, so the others' files would all change).
#
# The work folder gets deploy.exe, before/ and after/; each scenario below
# is copied to fixtures/dictionary/deploy/<scenario>/ as before/S (the source
# dictionary and its tables), before/D (the free tables) and after/D (what the
# deployment made of it), the log with its times and folder shown as 00:00:00
# and {D}, and the result of every call is in results.txt.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../../..
work=$1
dll=$2
fixtures=$root/fixtures/dictionary/deploy
scenarios="same_backup missing add_field drop_field change_field add_index idx_other
    memo_missing dbf_same dbf_missing dbf_add_field obj_view backup_exists backup_misc
    constraint_nv dest_dict doc_same drop_index dbf_extra_index
    obj_user obj_group x_users x_sqlusers x_grant x_trigger x_ri x_link x_pub x_desc x_long
    x_big x_dropped x_dbset sub_a subm_a subm_b constraint constraint_backup fail_exists
    v_max v_both v_expr v_str v_two v_sub v_none v_users o1 o3 o5 o8 o10
    src_pw_given src_pw_none src_pw_wrong src_pw_users src_pw_grant src_pw_dropped src_pw_link
    x_pubv x_pubp x_pubf x_pubk x_publ x_pubb x_pubu x_pubg x_pubd x_pubm x_pubt0 x_pubv0 x_pubt x_pubtv x_pubt2 x_pubtbig x_pubtg x_pubttr x_pubtpub x_pubtri x_pubtlink x_pubtproc x_pubtpkg x_pubtuser x_pubtgrp x_pubtri3 x_pubtri3b x_pubtpk x_pubtdbf x_pubtmemo x_pubtplain x_pubtri3c x_pubtpw x_pubtdbset x_pubtdbset2"
scenarios=${SCENARIOS:-$scenarios}
mkdir -p "$work"
rm -rf "$work/out" "$work/before" "$work/after"
mkdir "$work/out"
i686-w64-mingw32-gcc -O1 -Wall -o "$work/deploy.exe" "$here/deploy.c"
cd "$work"
winout="Z:$(echo "$work/out" | tr / '\\')"
WINEDEBUG=-all wine deploy.exe "$dll" "$winout" prepare
cp -r out before
WINEDEBUG=-all wine deploy.exe "$dll" "$winout" run | grep '^RESULT|' | sort >results.txt
cp -r out after
if [ -z "$SCENARIOS" ]; then
    rm -rf "$fixtures"
    mkdir -p "$fixtures"
    : >"$fixtures/results.txt"
fi
for s in $scenarios; do
    rm -rf "$fixtures/$s"
    sed -i "/^RESULT|$s|/d" "$fixtures/results.txt"
    mkdir -p "$fixtures/$s/before" "$fixtures/$s/after"
    cp -r "before/$s/S" "before/$s/D" "$fixtures/$s/before/"
    cp -r "after/$s/D" "$fixtures/$s/after/"
    log="$fixtures/$s/after/D/adsdeploy.log"
    if [ -f "$log" ]; then
        sed -i -e 's/^[0-9][0-9]:[0-9][0-9]:[0-9][0-9]/00:00:00/' \
            -e "s|Z:[^ ]*\\\\$s\\\\D|{D}|g" "$log"
    fi
    grep "^RESULT|$s|" results.txt >>"$fixtures/results.txt"
done
