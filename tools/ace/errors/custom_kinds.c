/* Records which orders an Advantage Client Engine takes custom keys in, for
 * fixtures/errors/custom_kinds.json.
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o custom_kinds.exe custom_kinds.c
 *     cd <empty folder> && wine custom_kinds.exe <ace32.dll> <the folder as a Windows path>
 *
 * The original resolves relative table names against the current directory,
 * so the folder must be the current one too. For each kind of order below
 * (ADT; NTX; a FoxPro table's compound and single-order files; a Visual
 * FoxPro table's), made with and without ADS_CUSTOM, the probe makes a
 * table of one field ID N(4) with two records and an order on ID in a file
 * of its own, goes to record 2 and gives the row
 *
 *     {"kind", "custom", "create", "add", "delete", "table"}
 *
 * of what AdsCreateIndex61, AdsAddCustomKey and AdsDeleteCustomKey of the
 * order, and AdsAddCustomKey of the table handle, return. */
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
    static const struct { const char *name, *ext; U16 type; U32 options; } kinds[] = {
        {"ADT", "adi", 3, 2},        {"NTX", "ntx", 1, 0}, {"CDX", "cdx", 2, 2},
        {"CDX single", "idx", 2, 0}, {"VFP", "cdx", 4, 2}, {"VFP single", "idx", 4, 0}};
    int n = sizeof kinds / sizeof kinds[0];
    printf("[\n");
    for (int k = 0; k < n; k++) {
        for (int custom = 0; custom < 2; custom++) {
            char table_name[16], bag[16];
            H table = 0, order = 0;
            snprintf(table_name, sizeof table_name, "c%d%d", k, custom);
            snprintf(bag, sizeof bag, "c%d%d.%s", k, custom, kinds[k].ext);
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *))F(
                "AdsCreateTable"))(conn, (U8 *)table_name, NULL, kinds[k].type, 1, 0, 1, 0,
                                   (U8 *)"ID,N,4,0", &table);
            if (rc) return printf("create %s: %lu\n", table_name, rc), 1;
            for (int r = 0; r < 2; r++) {
                ((U32(WINAPI *)(H))F("AdsAppendRecord"))(table);
                ((U32(WINAPI *)(H))F("AdsWriteRecord"))(table);
            }
            U32 created = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, U32, H *))F(
                "AdsCreateIndex61"))(table, (U8 *)bag, (U8 *)"K", (U8 *)"ID", (U8 *)"", (U8 *)"",
                                     kinds[k].options | (custom ? 4 : 0), 512, &order);
            ((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(table, 2);
            U32 add = ((U32(WINAPI *)(H))F("AdsAddCustomKey"))(order);
            U32 del = ((U32(WINAPI *)(H))F("AdsDeleteCustomKey"))(order);
            U32 on_table = ((U32(WINAPI *)(H))F("AdsAddCustomKey"))(table);
            printf("%s{\"kind\": \"%s\", \"custom\": %d, \"create\": %lu, \"add\": %lu, "
                   "\"delete\": %lu, \"table\": %lu}",
                   k || custom ? ",\n" : "", kinds[k].name, custom, created, add, del, on_table);
            ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
        }
    }
    printf("\n]\n");
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
    return 0;
}
