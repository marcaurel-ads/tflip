/* Records in which open modes an Advantage Client Engine opens a single-order
 * file named as a FoxPro table's structural .cdx with AdsOpenIndex, for
 * fixtures/errors/stray_index_modes.json.
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o stray_index_modes.exe stray_index_modes.c
 *     cd <empty folder> && wine stray_index_modes.exe <ace32.dll> <the folder as a Windows path>
 *
 * The original resolves relative table names against the current directory,
 * so the folder must be the current one too. For each table type (ADS_CDX 2,
 * ADS_VFP 4) and each open option (0 shared, ADS_EXCLUSIVE 1, ADS_READONLY
 * 2), the probe makes the table s<type>_<option> of one field ID N(4) with
 * two records, and an order K on ID made without ADS_COMPOUND into the file
 * s<type>_<option>.cdx (a single-order file, named as the table's structural
 * one), as OpenADS's tests make them. It closes the table, opens it again
 * with the option and gives the row
 *
 *     {"type", "options", "open", "n", "index", "orders", "again", "after"}
 *
 * of AdsOpenTable, AdsGetNumIndexes after it, AdsOpenIndex of the file by
 * its name and how many orders it wrote, AdsOpenIndex of it once more, and
 * AdsGetNumIndexes at the end. */
#include <windows.h>
#include <stdio.h>

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
    static const struct { const char *name; U16 type; } types[] = {{"CDX", 2}, {"VFP", 4}};
    int first = 1;
    printf("[\n");
    for (int t = 0; t < 2; t++) {
        for (U32 options = 0; options < 3; options++) {
            char name[16], bag[24];
            H table = 0, order = 0;
            snprintf(name, sizeof name, "s%u_%lu", types[t].type, options);
            snprintf(bag, sizeof bag, "%s.cdx", name);
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *))F(
                "AdsCreateTable"))(conn, (U8 *)name, NULL, types[t].type, 1, 0, 1, 0,
                                   (U8 *)"ID,N,4,0", &table);
            if (rc) return printf("create %s: %lu\n", name, rc), 1;
            for (int r = 0; r < 2; r++) {
                ((U32(WINAPI *)(H))F("AdsAppendRecord"))(table);
                ((U32(WINAPI *)(H, U8 *, double))F("AdsSetDouble"))(table, (U8 *)"ID", r + 1);
                ((U32(WINAPI *)(H))F("AdsWriteRecord"))(table);
            }
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, U32, H *))F(
                "AdsCreateIndex61"))(table, (U8 *)bag, (U8 *)"K", (U8 *)"ID", (U8 *)"", (U8 *)"",
                                     0, 512, &order);
            if (rc) return printf("index %s: %lu\n", bag, rc), 1;
            ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
            U32 open = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F(
                "AdsOpenTable"))(conn, (U8 *)name, NULL, types[t].type, 1, 1, 1, options, &table);
            U16 n = 0, after = 0, count = 8, again_count = 8;
            H orders[8], again_orders[8];
            U32 index = 0, again = 0;
            if (open == 0) {
                ((U32(WINAPI *)(H, U16 *))F("AdsGetNumIndexes"))(table, &n);
                index = ((U32(WINAPI *)(H, U8 *, H *, U16 *))F("AdsOpenIndex"))(
                    table, (U8 *)bag, orders, &count);
                again = ((U32(WINAPI *)(H, U8 *, H *, U16 *))F("AdsOpenIndex"))(
                    table, (U8 *)bag, again_orders, &again_count);
                ((U32(WINAPI *)(H, U16 *))F("AdsGetNumIndexes"))(table, &after);
                ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
            }
            printf("%s{\"type\": \"%s\", \"options\": %lu, \"open\": %lu, \"n\": %u, "
                   "\"index\": %lu, \"orders\": %u, \"again\": %lu, \"after\": %u}",
                   first ? "" : ",\n", types[t].name, options, open, n, index,
                   index ? 0 : count, again, after);
            first = 0;
        }
    }
    printf("\n]\n");
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
    return 0;
}
