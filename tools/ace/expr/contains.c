/* Records what CONTAINS finds in an Advantage Client Engine's filters and
 * AOFs, for fixtures/expr/contains.json.
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o contains.exe contains.c
 *     cd <empty folder> && wine contains.exe <ace32.dll> <the folder as a Windows path>
 *
 * The original resolves relative table names against the current directory,
 * so the folder must be the current one too. For each table type (NTX, CDX,
 * ADT, VFP) the probe makes the table c<type>_<i> of the fields NOTES C(40)
 * and M Memo, both holding, record by record,
 *
 *     1 "Quick brown fox jumps over the lazy dog"
 *     2 "The lazy dog sleeps in the sun"
 *     3 "Quick fox returns to the den at dusk"
 *     4 "A b c 1,423.99 x-ray"
 *
 * once without a full-text order (i 0) and once with one on NOTES made by
 * AdsCreateFTSIndex in a file of its own, the default lists (i 1; not on
 * NTX, which has none). For each condition below and each field, it gives
 * the rows
 *
 *     {"type", "fts", "field", "condition", "op": "filter"|"aof", "rc",
 *      "count_rc", "count"}
 *
 * of AdsSetFilter or AdsSetAOF (ADS_RESOLVE_DYNAMIC) of
 * CONTAINS(<field>, '<condition>'), then AdsGetRecordCount
 * (ADS_RESPECTFILTERS) with the filter or AOF on and what it returned (0
 * and 777 where the set failed; 777 where the count failed). */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;
static HMODULE ace;
#define F(name) ((void *)GetProcAddress(ace, name))
typedef U32(WINAPI *Fts)(H, U8 *, U8 *, U8 *, U32, U32, U32, U16, U8 *, U16, U8 *, U16, U8 *,
                         U16, U8 *, U8 *, U8 *, U32);

static const char *records[] = {"Quick brown fox jumps over the lazy dog",
                                "The lazy dog sleeps in the sun",
                                "Quick fox returns to the den at dusk", "A b c 1,423.99 x-ray"};
static const char *conditions[] = {
    "quick", "QUICK",          "zzz",         "qu*",       "*ick",    "\"lazy dog\"",
    "\"dog lazy\"", "quick AND lazy", "quick OR sun", "NOT quick", "quick AND NOT lazy",
    "the",   "a",              "b",           "1,423.99",  "x-ray",   "lazy NEAR sun",
    "",      NULL,
};

int main(int argc, char **argv) {
    if (argc != 3 || !argv[2][0]) return 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) return 1;
    H conn = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
        (U8 *)argv[2], 1, NULL, NULL, 0, &conn);
    if (rc) return printf("connect: %lu\n", rc), 1;
    static const struct { const char *name; U16 type; } types[] = {
        {"NTX", 1}, {"CDX", 2}, {"ADT", 3}, {"VFP", 4}};
    static const char *fields[] = {"NOTES", "M"};
    int first = 1;
    printf("[\n");
    for (int t = 0; t < 4; t++) {
        for (int fts = 0; fts < 2; fts++) {
            if (fts && types[t].type == 1) continue;
            char name[16];
            H table = 0;
            snprintf(name, sizeof name, "c%u_%d", types[t].type, fts);
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *))F(
                "AdsCreateTable"))(conn, (U8 *)name, NULL, types[t].type, 1, 0, 1, 0,
                                   (U8 *)"NOTES,C,40,0;M,Memo", &table);
            if (rc) return printf("create %s: %lu\n", name, rc), 1;
            for (int r = 0; r < 4; r++) {
                ((U32(WINAPI *)(H))F("AdsAppendRecord"))(table);
                for (int f = 0; f < 2; f++)
                    ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetString"))(
                        table, (U8 *)fields[f], (U8 *)records[r], (U32)strlen(records[r]));
                ((U32(WINAPI *)(H))F("AdsWriteRecord"))(table);
            }
            if (fts) {
                char file[24];
                U8 empty[2] = {0, 0};
                snprintf(file, sizeof file, "%s_n", name);
                rc = ((Fts)F("AdsCreateFTSIndex"))(table, (U8 *)file, (U8 *)"NOTESFTS",
                                                   (U8 *)"NOTES", 0, 3, 30, 1, empty, 1, empty,
                                                   1, empty, 1, empty, empty, empty, 0);
                if (rc) return printf("fts %s: %lu\n", name, rc), 1;
            }
            for (int f = 0; f < 2; f++) {
                for (int c = 0; conditions[c]; c++) {
                    for (int op = 0; op < 2; op++) {
                        char expr[128];
                        snprintf(expr, sizeof expr, "CONTAINS(%s, '%s')", fields[f], conditions[c]);
                        rc = op ? ((U32(WINAPI *)(H, U8 *, U16))F("AdsSetAOF"))(table, (U8 *)expr,
                                                                                 2)
                                : ((U32(WINAPI *)(H, U8 *))F("AdsSetFilter"))(table, (U8 *)expr);
                        U32 count = 777, count_rc = 0;
                        if (rc == 0) {
                            count_rc = ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(
                                table, 1, &count);
                            if (op) ((U32(WINAPI *)(H))F("AdsClearAOF"))(table);
                            else ((U32(WINAPI *)(H))F("AdsClearFilter"))(table);
                        }
                        printf("%s{\"type\": \"%s\", \"fts\": %d, \"field\": \"%s\", "
                               "\"condition\": \"",
                               first ? "" : ",\n", types[t].name, fts, fields[f]);
                        for (const char *p = conditions[c]; *p; p++)
                            printf(*p == '"' ? "\\\"" : "%c", *p);
                        printf("\", \"op\": \"%s\", \"rc\": %lu, \"count_rc\": %lu, "
                               "\"count\": %lu}",
                               op ? "aof" : "filter", rc, count_rc, count);
                        first = 0;
                    }
                }
            }
            ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
        }
    }
    printf("\n]\n");
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
    return 0;
}
