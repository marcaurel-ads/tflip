/* Records what the original engine answers for handles that are wrong:
 * a value that is no handle, 0, and handles of the wrong kinds, given to
 * calls that take one handle; for fixtures/errors/handles.json.
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o handles.exe handles.c
 *     wine handles.exe <ace32.dll> <folder with all_types.adt as a Windows path>
 *
 * Each call is {"fn": <name>, "handle": <kind given>, "before": <the error
 * provoked just before it>, "rc": <rc>, "last": <AdsGetLastError's code>}:
 * the last error is sticky, so a call that leaves it shows "before". A call
 * is not made with the kind it takes (it would succeed, and some change the
 * table). */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;
static HMODULE ace;
#define F(name) ((void *)GetProcAddress(ace, name))

enum { CONN = 1, TABLE = 2, ORDER = 4, STMT = 8, ZERO = 16 };

static const struct {
    const char *fn;
    int takes;
} calls[] = {
    {"AdsGotoTop", TABLE | ORDER},       {"AdsGotoBottom", TABLE | ORDER},
    {"AdsWriteRecord", TABLE},           {"AdsRefreshRecord", TABLE},
    {"AdsClearFilter", TABLE},           {"AdsClearAOF", TABLE},
    {"AdsClearRelation", TABLE},         {"AdsLockTable", TABLE},
    {"AdsUnlockTable", TABLE},           {"AdsAppendRecord", TABLE},
    {"AdsDeleteRecord", TABLE},          {"AdsRecallRecord", TABLE},
    {"AdsCancelUpdate", TABLE},          {"AdsReindex", TABLE | ORDER},
    {"AdsCloseTable", TABLE},            {"AdsPackTable", TABLE},
    {"AdsBeginTransaction", CONN | ZERO}, {"AdsCommitTransaction", CONN | ZERO},
    {"AdsRollbackTransaction", CONN | ZERO}, {"AdsCloseSQLStatement", STMT},
    {"AdsClearSQLParams", STMT},         {"AdsRefreshAOF", TABLE},
    {"AdsFlushFileBuffers", TABLE},      {"AdsRecallAllRecords", TABLE},
};

int main(int argc, char **argv) {
    if (argc < 3) return fprintf(stderr, "usage: handles.exe <ace32.dll> <folder>\n"), 2;
    setvbuf(stdout, NULL, _IONBF, 0);
    ace = LoadLibraryA(argv[1]);
    if (!ace) return fprintf(stderr, "cannot load %s\n", argv[1]), 1;
    H conn = 0, table = 0, order = 0, stmt = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
        (U8 *)argv[2], 1, NULL, NULL, 0, &conn);
    if (rc) return fprintf(stderr, "connect: %lu\n", rc), 1;
    char path[600];
    snprintf(path, sizeof path, "%s\\all_types.adt", argv[2]);
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
        conn, (U8 *)path, NULL, 3, 1, 1, 1, 2, &table);
    if (rc) return fprintf(stderr, "open: %lu\n", rc), 1;
    U16 n = 1;
    ((U32(WINAPI *)(H, H *, U16 *))F("AdsGetAllIndexes"))(table, &order, &n);
    ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(conn, &stmt);
    const struct {
        const char *name;
        int kind;
        H handle;
    } handles[] = {{"none", 0, 0x12345}, {"zero", ZERO, 0}, {"connection", CONN, conn},
                   {"table", TABLE, table}, {"order", ORDER, order}, {"statement", STMT, stmt}};
    printf("[\n");
    int first = 1;
    for (size_t i = 0; i < sizeof calls / sizeof *calls; i++) {
        for (size_t k = 0; k < sizeof handles / sizeof *handles; k++) {
            if (handles[k].kind & calls[i].takes) continue;
            /* A last error of another kind before the call (a record
             * number that is not there), to see whether the call replaces
             * it, and with what. */
            U32 before = ((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(table, 999999);
            U32 rc = ((U32(WINAPI *)(H))F(calls[i].fn))(handles[k].handle);
            U32 code = 0;
            U8 text[600];
            U16 len = sizeof text;
            ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
            printf("%s{\"fn\": \"%s\", \"handle\": \"%s\", \"before\": %lu, \"rc\": %lu, "
                   "\"last\": %lu}",
                   first ? "" : ",\n", calls[i].fn, handles[k].name, before, rc, code);
            first = 0;
        }
    }
    printf("\n]\n");
    return 0;
}
