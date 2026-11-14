/* Records what AdsGetRecordCount of an Advantage Client Engine counts on a
 * table handle and on an index order's, for fixtures/errors/order_counts.json.
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o order_counts.exe order_counts.c
 *     cd <empty folder> && wine order_counts.exe <ace32.dll> <the folder as a Windows path>
 *
 * The original resolves relative table names against the current directory,
 * so the folder must be the current one too. For each table type (NTX, CDX,
 * ADT, VFP) and each case below, the probe makes the table n<type>_<case> of
 * the fields NAME C(6), VAL N(4) and INC C(1) with the records
 *
 *     PERA 10 Y, MANGO 20 N, UVA 30 Y, KIWI 40 N, FRESA 50 Y
 *
 * and the order K on STR(VAL,8) in a file of its own (compound for ADT and
 * CDX), then counts with AdsGetRecordCount on the table's handle and the
 * order's, with ADS_RESPECTFILTERS (1), ADS_IGNOREFILTERS (2) and
 * ADS_RESPECTSCOPES (3). The cases:
 *
 *   plain       the order without a condition;
 *   for         the order FOR VAL >= 20;
 *   scope       the plain order scoped to "      20" to "      40";
 *   for scope   the order FOR VAL >= 20 scoped the same way;
 *   filter      the order FOR VAL >= 20, the table filtered INC = 'Y';
 *   moved       the order FOR VAL >= 20, after AdsGotoTop of its handle.
 *
 * Each row is {"type", "case", "handle", "option", "rc", "count"}. */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;
static HMODULE ace;
#define F(name) ((void *)GetProcAddress(ace, name))

int main(int argc, char **argv) {
    if (argc != 3 || !argv[2][0]) return 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) return 1;
    H conn = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
        (U8 *)argv[2], 1, NULL, NULL, 0, &conn);
    if (rc) return printf("connect: %lu\n", rc), 1;
    static const struct { const char *name, *ext; U16 type; U32 options; } types[] = {
        {"NTX", "ntx", 1, 0}, {"CDX", "cdx", 2, 2}, {"ADT", "adi", 3, 2}, {"VFP", "idx", 4, 0}};
    static const char *cases[] = {"plain", "for", "scope", "for scope", "filter", "moved"};
    static const char *names[] = {"PERA", "MANGO", "UVA", "KIWI", "FRESA"};
    static const char *incs[] = {"Y", "N", "Y", "N", "Y"};
    int first = 1;
    printf("[\n");
    for (int t = 0; t < 4; t++) {
        for (int c = 0; c < 6; c++) {
            char name[16], bag[24];
            H table = 0, order = 0;
            snprintf(name, sizeof name, "n%u_%d", types[t].type, c);
            snprintf(bag, sizeof bag, "%s_k.%s", name, types[t].ext);
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *))F(
                "AdsCreateTable"))(conn, (U8 *)name, NULL, types[t].type, 1, 0, 1, 0,
                                   (U8 *)"NAME,C,6,0;VAL,N,4,0;INC,C,1,0", &table);
            if (rc) return printf("create %s: %lu\n", name, rc), 1;
            for (int r = 0; r < 5; r++) {
                ((U32(WINAPI *)(H))F("AdsAppendRecord"))(table);
                ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetString"))(
                    table, (U8 *)"NAME", (U8 *)names[r], (U32)strlen(names[r]));
                ((U32(WINAPI *)(H, U8 *, double))F("AdsSetDouble"))(table, (U8 *)"VAL",
                                                                    10.0 * (r + 1));
                ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetString"))(table, (U8 *)"INC",
                                                                        (U8 *)incs[r], 1);
                ((U32(WINAPI *)(H))F("AdsWriteRecord"))(table);
            }
            const char *condition = c == 0 || c == 2 ? "" : "VAL >= 20";
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, U32, H *))F(
                "AdsCreateIndex61"))(table, (U8 *)bag, (U8 *)"K", (U8 *)"STR(VAL,8)",
                                     (U8 *)condition, (U8 *)"", types[t].options, 512, &order);
            if (rc) return printf("index %s: %lu\n", bag, rc), 1;
            if (c == 2 || c == 3) {
                ((U32(WINAPI *)(H, U16, U8 *, U16, U16))F("AdsSetScope"))(
                    order, 1, (U8 *)"      20", 8, 2);
                ((U32(WINAPI *)(H, U16, U8 *, U16, U16))F("AdsSetScope"))(
                    order, 2, (U8 *)"      40", 8, 2);
            }
            if (c == 4)
                ((U32(WINAPI *)(H, U8 *))F("AdsSetFilter"))(table, (U8 *)"INC = 'Y'");
            if (c == 5) ((U32(WINAPI *)(H))F("AdsGotoTop"))(order);
            for (int h = 0; h < 2; h++) {
                for (U16 option = 1; option <= 3; option++) {
                    U32 count = 777;
                    rc = ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(
                        h ? order : table, option, &count);
                    printf("%s{\"type\": \"%s\", \"case\": \"%s\", \"handle\": \"%s\", "
                           "\"option\": %u, \"rc\": %lu, \"count\": %lu}",
                           first ? "" : ",\n", types[t].name, cases[c], h ? "order" : "table",
                           option, rc, count);
                    first = 0;
                }
            }
            ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
        }
    }
    printf("\n]\n");
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
    return 0;
}
