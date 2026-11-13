/* Confirms what happens when a table whose structural ADI was built under one
 * static collation is opened under another: AdsOpenTable of the charset_german
 * `ca` table, under the default collation and under German. Built and run like
 * tools/ace/expr/probe.c, with an "engine" folder of symlinks to the acesdk
 * files plus an adslocal.cfg of its own that sets ANSI_CHAR_SET (the local
 * server reads adslocal.cfg from the DLL's folder), as run_case.sh does:
 *
 *     SDK="$HOME/src/ace-sdk/.../acesdk"
 *     i686-w64-mingw32-gcc -O1 -Wall -I "$SDK" -o openmismatch.exe openmismatch.c
 *     # copy fixtures/adt/charset_german/{ca.adt,ca.adi} into a data folder,
 *     # make an engine dir of acesdk symlinks + adslocal.cfg ANSI_CHAR_SET=...,
 *     # then, per collation:
 *     WINEPREFIX=~/src/ace-sdk wine openmismatch.exe '<engine ace32.dll>' 'Z:\<data>'
 *
 * German-built ADI under the default collation gives 5175, under German 0. */
#include <windows.h>
#include <stdio.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;

static HMODULE ace;
#define F(name) GetProcAddress(ace, name)

int main(int argc, char **argv) {
    if (argc < 3) {
        fprintf(stderr, "usage: openmismatch.exe <ace32.dll> <data folder>\n");
        return 2;
    }
    ace = LoadLibraryA(argv[1]);
    if (!ace) { fprintf(stderr, "LoadLibrary failed\n"); return 2; }
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    H conn = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, H *))F("AdsConnect26"))((U8 *)argv[2], 1, &conn);
    if (rc) { fprintf(stderr, "AdsConnect26 = %lu\n", rc); return 2; }
    char path[1024];
    snprintf(path, sizeof path, "%s\\ca.adt", argv[2]);
    H t = 0;
    /* ADS_ADT=3, ANSI=1, proprietary lock=1, check rights=2, exclusive=1 */
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
        conn, (U8 *)path, NULL, 3, 1, 1, 2, 1, &t);
    printf("{\"open\": %lu", rc);
    if (rc == 0) {
        U32 count = 0;
        ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(t, 1, &count);
        printf(", \"count\": %lu", count);
    }
    printf("}\n");
    fflush(stdout);
    return 0;
}
