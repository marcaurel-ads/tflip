/* Records what AdsSetIndexDirection does to an Advantage Client Engine's
 * index orders, for fixtures/seek/direction.json.
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o direction.exe direction.c
 *     cd <empty folder> && wine direction.exe <ace32.dll> <the folder as a Windows path>
 *
 * The original resolves relative table names against the current directory,
 * so the folder must be the current one too. For each table type (ADT, NTX,
 * CDX, VFP) the probe makes the table d<type> of the field C C(4) and the
 * records 1 "BBBB", 2 "AAAA", 3 "CCCC", and for each kind of order below
 * (ascending or ADS_DESCENDING, in a compound file or a file of its own,
 * where the type has both) an order on C in a file of its own. Then, for
 * each value v of the list, it calls AdsSetIndexDirection(order, v) and
 * gives the row
 *
 *     {"type", "kind", "value", "rc", "error", "descending", "walk",
 *      "table_walk", "top_key"}
 *
 * "error" is AdsGetLastError's code after the call, "descending" what
 * AdsIsIndexDescending says after it, "walk" the records from AdsGotoTop of
 * the order skipping 1 at a time through it, "table_walk" the same through
 * the table handle (whose current order a new order does not become), and
 * "top_key" AdsGetKeyNum(ADS_IGNOREFILTERS) at the order's top. The values
 * are applied one after another on the same order: 1, 1, 0, 2, 1, 0. */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;
static HMODULE ace;
#define F(name) ((void *)GetProcAddress(ace, name))

static void walk(H table, H mover) {
    printf("[");
    ((U32(WINAPI *)(H))F("AdsGotoTop"))(mover);
    for (int i = 0; i < 5; i++) {
        U16 eof = 0;
        U32 recno = 0;
        ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(table, &eof);
        if (eof) break;
        ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordNum"))(table, 2, &recno);
        printf("%s%lu", i ? ", " : "", recno);
        ((U32(WINAPI *)(H, U32))F("AdsSkip"))(mover, 1);
    }
    printf("]");
}

int main(int argc, char **argv) {
    if (argc != 3 || !argv[2][0]) return 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) return 1;
    H conn = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
        (U8 *)argv[2], 1, NULL, NULL, 0, &conn);
    if (rc) return printf("connect: %lu\n", rc), 1;
    static const struct { const char *name, *ext; U16 type; } types[] = {
        {"ADT", "adi", 3}, {"NTX", "ntx", 1}, {"CDX", "cdx", 2}, {"VFP", "cdx", 4}};
    static const struct { const char *name; U32 options; int compound; } kinds[] = {
        {"ascending", 0, 0}, {"descending", 8, 0},
        {"compound ascending", 2, 1}, {"compound descending", 10, 1}};
    static const U16 values[] = {1, 1, 0, 2, 1, 0};
    static const char *records[] = {"BBBB", "AAAA", "CCCC"};
    int first = 1;
    printf("[\n");
    for (int t = 0; t < 4; t++) {
        char name[16];
        H table = 0;
        snprintf(name, sizeof name, "d%u", types[t].type);
        rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *))F(
            "AdsCreateTable"))(conn, (U8 *)name, NULL, types[t].type, 1, 0, 1, 0,
                               (U8 *)"C,C,4,0", &table);
        if (rc) return printf("create %s: %lu\n", name, rc), 1;
        for (int r = 0; r < 3; r++) {
            ((U32(WINAPI *)(H))F("AdsAppendRecord"))(table);
            ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetString"))(table, (U8 *)"C",
                                                                    (U8 *)records[r], 4);
            ((U32(WINAPI *)(H))F("AdsWriteRecord"))(table);
        }
        for (int k = 0; k < 4; k++) {
            /* ADT orders are always compound; NTX ones never. */
            if (types[t].type == 3 && !kinds[k].compound) continue;
            if (types[t].type == 1 && kinds[k].compound) continue;
            char bag[32];
            H order = 0;
            snprintf(bag, sizeof bag, "o%u_%d.%s", types[t].type, k, types[t].ext);
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, U32, H *))F(
                "AdsCreateIndex61"))(table, (U8 *)bag, (U8 *)"K", (U8 *)"C", (U8 *)"", (U8 *)"",
                                     kinds[k].options, 512, &order);
            if (rc) {
                printf("%s{\"type\": \"%s\", \"kind\": \"%s\", \"index\": %lu}",
                       first ? "" : ",\n", types[t].name, kinds[k].name, rc);
                first = 0;
                continue;
            }
            for (int v = 0; v < (int)(sizeof values / sizeof values[0]); v++) {
                rc = ((U32(WINAPI *)(H, U16))F("AdsSetIndexDirection"))(order, values[v]);
                U32 error = 0;
                U8 text[600];
                U16 len = sizeof text;
                ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&error, text, &len);
                U16 descending = 7;
                ((U32(WINAPI *)(H, U16 *))F("AdsIsIndexDescending"))(order, &descending);
                printf("%s{\"type\": \"%s\", \"kind\": \"%s\", \"value\": %u, \"rc\": %lu, "
                       "\"error\": %lu, \"descending\": %u, \"walk\": ",
                       first ? "" : ",\n", types[t].name, kinds[k].name, values[v], rc, error,
                       descending);
                first = 0;
                walk(table, order);
                printf(", \"table_walk\": ");
                walk(table, table);
                U32 key = 0;
                ((U32(WINAPI *)(H))F("AdsGotoTop"))(order);
                ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetKeyNum"))(order, 2, &key);
                printf(", \"top_key\": %lu}", key);
            }
            ((U32(WINAPI *)(H))F("AdsCloseIndex"))(order);
        }
        ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
    }
    printf("\n]\n");
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
    return 0;
}
