/* Records what an Advantage Client Engine does when a compound order and a
 * single one are made into the same index file of a FoxPro or Visual FoxPro
 * table, for fixtures/errors/order_files.json.
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o order_files.exe order_files.c
 *     cd <empty folder> && wine order_files.exe <ace32.dll> <the folder as a Windows path>
 *
 * The original resolves relative table names against the current directory,
 * so the folder must be the current one too. For each table type (CDX, VFP),
 * each extension of the file (cdx, xyz) and each order of the two (single
 * then compound, compound then single), the probe makes the table
 * m<type>_<n> of one field ID N(4) with the records 3, 2, 1, and the orders
 * K1 then K2 on ID into the file p<type>_<n>.<extension> with
 * AdsCreateIndex61 (ADS_COMPOUND or not). Each row is
 *
 *     {"type", "extension", "first", "second", "rc": [first, second],
 *      "orders", "reopen", "names"}
 *
 * "orders" being AdsGetNumIndexes after both, and "reopen" and "names" what
 * AdsOpenIndex of the file returns, and the names (AdsGetIndexName) of the
 * orders it opens, after the table is closed and opened again. */
#include <windows.h>
#include <stdio.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;
static HMODULE ace;
#define F(name) ((void *)GetProcAddress(ace, name))

static U32 create_index(H table, const char *bag, const char *tag, U32 options) {
    H order = 0;
    return ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, U32, H *))F("AdsCreateIndex61"))(
        table, (U8 *)bag, (U8 *)tag, (U8 *)"ID", (U8 *)"", (U8 *)"", options, 512, &order);
}

int main(int argc, char **argv) {
    if (argc != 3 || !argv[2][0]) return 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) return 1;
    H conn = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
        (U8 *)argv[2], 1, NULL, NULL, 0, &conn);
    if (rc) return printf("connect: %lu\n", rc), 1;
    static const struct { const char *name; U16 type; } types[] = {{"CDX", 2}, {"VFP", 4}};
    static const char *extensions[] = {"cdx", "xyz"};
    int first = 1, n = 0;
    printf("[\n");
    for (int t = 0; t < 2; t++) {
        for (int e = 0; e < 2; e++) {
            for (int way = 0; way < 2; way++, n++) {
                char name[16], bag[24];
                H table = 0;
                snprintf(name, sizeof name, "m%u_%d", types[t].type, n);
                snprintf(bag, sizeof bag, "p%u_%d.%s", types[t].type, n, extensions[e]);
                rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *))F(
                    "AdsCreateTable"))(conn, (U8 *)name, NULL, types[t].type, 1, 0, 1, 0,
                                       (U8 *)"ID,N,4,0", &table);
                if (rc) return printf("create %s: %lu\n", name, rc), 1;
                for (int r = 0; r < 3; r++) {
                    ((U32(WINAPI *)(H))F("AdsAppendRecord"))(table);
                    ((U32(WINAPI *)(H, U8 *, double))F("AdsSetDouble"))(table, (U8 *)"ID", 3 - r);
                    ((U32(WINAPI *)(H))F("AdsWriteRecord"))(table);
                }
                U32 a = create_index(table, bag, "K1", way ? 2 : 0);
                U32 b = create_index(table, bag, "K2", way ? 0 : 2);
                U16 orders = 0;
                ((U32(WINAPI *)(H, U16 *))F("AdsGetNumIndexes"))(table, &orders);
                ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
                ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
                    conn, (U8 *)name, NULL, types[t].type, 1, 0, 1, 0, &table);
                H opened[8];
                U16 count = 8;
                U32 reopen = ((U32(WINAPI *)(H, U8 *, H *, U16 *))F("AdsOpenIndex"))(
                    table, (U8 *)bag, opened, &count);
                if (reopen && reopen != 5065) count = 0;
                printf("%s{\"type\": \"%s\", \"extension\": \"%s\", \"first\": \"%s\", "
                       "\"second\": \"%s\", \"rc\": [%lu, %lu], \"orders\": %u, \"reopen\": %lu, "
                       "\"names\": [",
                       first ? "" : ",\n", types[t].name, extensions[e],
                       way ? "compound" : "single", way ? "single" : "compound", a, b, orders,
                       reopen);
                first = 0;
                for (int o = 0; o < count; o++) {
                    U8 tag[32];
                    U16 len = sizeof tag;
                    ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsGetIndexName"))(opened[o], tag, &len);
                    printf("%s\"%.*s\"", o ? ", " : "", len, tag);
                }
                printf("]}");
                ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
            }
        }
    }
    printf("\n]\n");
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
    return 0;
}
