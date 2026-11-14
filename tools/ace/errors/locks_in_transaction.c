/* Records what an Advantage Client Engine allows of unlocking and closing
 * locked tables inside a transaction, for
 * fixtures/errors/locks_in_transaction.json.
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o locks_in_transaction.exe locks_in_transaction.c
 *     cd <empty folder> && wine locks_in_transaction.exe <ace32.dll> <the folder as a Windows path>
 *
 * The original resolves relative table names against the current directory,
 * so the folder must be the current one too. For each table type (NTX, CDX,
 * ADT, VFP) and each script below, the probe makes the table l<type>_<n> of
 * TAG C(5) with the records HELLO and THERE, closes it, opens it shared on
 * a new connection and runs the script's steps, each a call on that table,
 * its connection or a record of it:
 *
 *   B begin (AdsBeginTransaction)   C commit      R rollback
 *   L AdsLockTable                  U AdsUnlockTable
 *   l AdsLockRecord(1)              u AdsUnlockRecord(1)
 *   X AdsCloseTable                 N AdsGetRecordCount (is it still open?)
 *   W AdsGotoTop, AdsSetString(TAG, "WORLD"), AdsWriteRecord
 *
 * Each row is {"type", "script", "rc": [one per step]}; the connection is
 * closed after the script (the table with it). */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;
static HMODULE ace;
#define F(name) ((void *)GetProcAddress(ace, name))

static U32 call(char step, H conn, H table) {
    U32 count = 0;
    switch (step) {
    case 'B': return ((U32(WINAPI *)(H))F("AdsBeginTransaction"))(conn);
    case 'C': return ((U32(WINAPI *)(H))F("AdsCommitTransaction"))(conn);
    case 'R': return ((U32(WINAPI *)(H))F("AdsRollbackTransaction"))(conn);
    case 'L': return ((U32(WINAPI *)(H))F("AdsLockTable"))(table);
    case 'U': return ((U32(WINAPI *)(H))F("AdsUnlockTable"))(table);
    case 'l': return ((U32(WINAPI *)(H, U32))F("AdsLockRecord"))(table, 1);
    case 'u': return ((U32(WINAPI *)(H, U32))F("AdsUnlockRecord"))(table, 1);
    case 'X': return ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
    case 'N': return ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(table, 1, &count);
    case 'W': {
        ((U32(WINAPI *)(H))F("AdsGotoTop"))(table);
        U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetString"))(table, (U8 *)"TAG",
                                                                       (U8 *)"WORLD", 5);
        return rc ? rc : ((U32(WINAPI *)(H))F("AdsWriteRecord"))(table);
    }
    }
    return 99999;
}

int main(int argc, char **argv) {
    if (argc != 3 || !argv[2][0]) return 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) return 1;
    static const struct { const char *name; U16 type; } types[] = {
        {"NTX", 1}, {"CDX", 2}, {"ADT", 3}, {"VFP", 4}};
    static const char *scripts[] = {
        "BXN",   "BLXNRN", "BLUXNRN", "BLRUXN", "BLCUXN", "LBXNRN", "LBUNRUXN",
        "BlXNRN", "BluXN", "lBuXN", "BLWXNRN", "BWXN", "BLRXN", "BlCuXN", NULL};
    int first = 1;
    printf("[\n");
    for (int t = 0; t < 4; t++) {
        for (int s = 0; scripts[s]; s++) {
            char name[16];
            H conn = 0, table = 0;
            snprintf(name, sizeof name, "l%u_%d", types[t].type, s);
            U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
                (U8 *)argv[2], 1, NULL, NULL, 0, &conn);
            if (rc) return printf("connect: %lu\n", rc), 1;
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *))F(
                "AdsCreateTable"))(conn, (U8 *)name, NULL, types[t].type, 1, 0, 1, 0,
                                   (U8 *)"TAG,C,5,0", &table);
            if (rc) return printf("create %s: %lu\n", name, rc), 1;
            for (int r = 0; r < 2; r++) {
                ((U32(WINAPI *)(H))F("AdsAppendRecord"))(table);
                ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetString"))(
                    table, (U8 *)"TAG", (U8 *)(r ? "THERE" : "HELLO"), 5);
                ((U32(WINAPI *)(H))F("AdsWriteRecord"))(table);
            }
            ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
            ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
            conn = table = 0;
            ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
                (U8 *)argv[2], 1, NULL, NULL, 0, &conn);
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
                conn, (U8 *)name, NULL, types[t].type, 1, 0, 1, 0, &table);
            if (rc) return printf("open %s: %lu\n", name, rc), 1;
            printf("%s{\"type\": \"%s\", \"script\": \"%s\", \"rc\": [", first ? "" : ",\n",
                   types[t].name, scripts[s]);
            first = 0;
            for (const char *step = scripts[s]; *step; step++)
                printf("%s%lu", step == scripts[s] ? "" : ", ", call(*step, conn, table));
            printf("]}");
            ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
        }
    }
    printf("\n]\n");
    return 0;
}
