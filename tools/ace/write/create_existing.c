/* Records what an Advantage Client Engine does when AdsCreateTable names a
 * table file that exists already, for fixtures/errors/create_existing.json.
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o create_existing.exe create_existing.c
 *     cd <empty folder> && wine create_existing.exe <ace32.dll> <the folder as a Windows path>
 *
 * The original resolves relative table names against the current directory,
 * so the folder must be the current one too. For each table type (NTX, CDX,
 * ADT, VFP) the probe makes a table of the fields NAME C(20) and M Memo with
 * two records and an order on NAME in the index file named after the table
 * (compound but for NTX), closed, and then in each scenario creates the
 * table again, as "NAME,Character,20,0;AGE,Numeric,4,0", on connection 1:
 *
 *   closed           nothing has it open;
 *   open shared      connection 2 has it open shared;
 *   open exclusive   connection 2 has it open exclusively;
 *   open here        connection 1 has it open shared;
 *   junk             the table's file holds the 5 bytes "hello" (no memo or
 *                    index file).
 *
 * Each row is
 *
 *     {"type", "scenario", "rc", "count", "fields", "orders", "files",
 *      "other_count"}
 *
 * "count" and "fields" being AdsGetRecordCount and AdsGetNumFields of the
 * new table, "orders" AdsGetNumIndexes of it, "files" which of the table's
 * files (table, memo, index) exist after the create (and the new table's
 * close), and "other_count" AdsGetRecordCount of the handle connection 2 or
 * 1 kept open (9 where there was none or a call failed). The table is
 * removed and made again between scenarios. */
#include <windows.h>
#include <stdio.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;
static HMODULE ace;
#define F(name) ((void *)GetProcAddress(ace, name))

static const struct { const char *name, *table, *memo, *index; U16 type; U32 options; } types[] = {
    {"NTX", "dbf", "dbt", "ntx", 1, 0},
    {"CDX", "dbf", "fpt", "cdx", 2, 2},
    {"ADT", "adt", "adm", "adi", 3, 2},
    {"VFP", "dbf", "fpt", "cdx", 4, 2},
};

static U32 create(H conn, const char *name, U16 type, const char *fields, H *table) {
    return ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *))F(
        "AdsCreateTable"))(conn, (U8 *)name, NULL, type, 1, 0, 1, 0, (U8 *)fields, table);
}

static U32 open(H conn, const char *name, U16 type, U32 options, H *table) {
    return ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
        conn, (U8 *)name, NULL, type, 1, 0, 1, options, table);
}

static void remove_files(int t, const char *stem) {
    const char *exts[] = {types[t].table, types[t].memo, types[t].index};
    for (int e = 0; e < 3; e++) {
        char file[64];
        snprintf(file, sizeof file, "%s.%s", stem, exts[e]);
        DeleteFileA(file);
    }
}

static int setup(H conn, int t, const char *stem) {
    H table = 0, order = 0;
    remove_files(t, stem);
    U32 rc = create(conn, stem, types[t].type, "NAME,Character,20,0;M,Memo", &table);
    if (rc) return printf("setup %s: %lu\n", stem, rc), 0;
    for (int r = 0; r < 2; r++) {
        ((U32(WINAPI *)(H))F("AdsAppendRecord"))(table);
        ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetString"))(table, (U8 *)"NAME", (U8 *)"abc", 3);
        ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetString"))(table, (U8 *)"M", (U8 *)"memo", 4);
        ((U32(WINAPI *)(H))F("AdsWriteRecord"))(table);
    }
    char bag[64];
    snprintf(bag, sizeof bag, "%s.%s", stem, types[t].index);
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, U32, H *))F("AdsCreateIndex61"))(
        table, (U8 *)bag, (U8 *)"N", (U8 *)"NAME", (U8 *)"", (U8 *)"", types[t].options, 512,
        &order);
    if (rc) return printf("index %s: %lu\n", bag, rc), 0;
    ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
    return 1;
}

int main(int argc, char **argv) {
    if (argc != 3 || !argv[2][0]) return 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) return 1;
    H one = 0, two = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
        (U8 *)argv[2], 1, NULL, NULL, 0, &one);
    if (rc) return printf("connect: %lu\n", rc), 1;
    ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))((U8 *)argv[2], 1, NULL,
                                                                         NULL, 0, &two);
    static const char *scenarios[] = {"closed", "open shared", "open exclusive", "open here",
                                      "junk"};
    int first = 1;
    printf("[\n");
    for (int t = 0; t < 4; t++) {
        for (int s = 0; s < 5; s++) {
            char stem[16];
            snprintf(stem, sizeof stem, "e%u_%d", types[t].type, s);
            if (!setup(one, t, stem)) return 1;
            H kept = 0;
            if (s == 1) open(two, stem, types[t].type, 0, &kept);
            if (s == 2) open(two, stem, types[t].type, 1, &kept);
            if (s == 3) open(one, stem, types[t].type, 0, &kept);
            if (s == 4) {
                remove_files(t, stem);
                char file[64];
                snprintf(file, sizeof file, "%s.%s", stem, types[t].table);
                FILE *out = fopen(file, "wb");
                fwrite("hello", 1, 5, out);
                fclose(out);
            }
            H table = 0;
            rc = create(one, stem, types[t].type, "NAME,Character,20,0;AGE,Numeric,4,0", &table);
            U32 count = 9, other = 9;
            U16 fields = 9, orders = 9;
            if (rc == 0) {
                ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(table, 1, &count);
                ((U32(WINAPI *)(H, U16 *))F("AdsGetNumFields"))(table, &fields);
                ((U32(WINAPI *)(H, U16 *))F("AdsGetNumIndexes"))(table, &orders);
                ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
            }
            if (kept) {
                if (((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(kept, 1, &other))
                    other = 9;
                ((U32(WINAPI *)(H))F("AdsCloseTable"))(kept);
            }
            const char *exts[] = {types[t].table, types[t].memo, types[t].index};
            printf("%s{\"type\": \"%s\", \"scenario\": \"%s\", \"rc\": %lu, \"count\": %lu, "
                   "\"fields\": %u, \"orders\": %u, \"files\": [",
                   first ? "" : ",\n", types[t].name, scenarios[s], rc, count, fields, orders);
            first = 0;
            int listed = 0;
            for (int e = 0; e < 3; e++) {
                char file[64];
                snprintf(file, sizeof file, "%s.%s", stem, exts[e]);
                if (GetFileAttributesA(file) != INVALID_FILE_ATTRIBUTES)
                    printf("%s\"%s\"", listed++ ? ", " : "", exts[e]);
            }
            printf("], \"other_count\": %lu}", other);
        }
    }
    printf("\n]\n");
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(two);
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(one);
    return 0;
}
