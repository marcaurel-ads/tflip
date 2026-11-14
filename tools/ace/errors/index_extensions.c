/* Records how an Advantage Client Engine takes index files whose extension
 * is not their format's (.fts, .xyz), for
 * fixtures/errors/index_extensions.json.
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o index_extensions.exe index_extensions.c
 *     cd <empty folder> && wine index_extensions.exe <ace32.dll> <the folder as a Windows path>
 *
 * The original resolves relative table names against the current directory,
 * so the folder must be the current one too. For each table type (NTX, CDX,
 * ADT, VFP) and each order below, the probe makes the table x<type>_<n> of
 * the fields ID N(4) and NOTES C(40) with the records (3, "Quick brown fox
 * jumps over the lazy dog"), (1, "The lazy dog sleeps in the sun"),
 * (2, "Quick fox returns to the den at dusk"), opened exclusively, makes the
 * order, closes the table, opens it again and opens the file by its name.
 * The orders:
 *
 *   fts        AdsCreateFTSIndex of NOTES into x<type>_<n>.fts (min 3, max
 *              30 letters, the default lists);
 *   index      AdsCreateIndex61 of ID into x<type>_<n>.xyz, not compound;
 *   compound   the same with ADS_COMPOUND (not on NTX).
 *
 * Each row is
 *
 *     {"type", "order", "create", "open", "orders", "found"}
 *
 * of the create, AdsOpenIndex of the file after the table was opened again
 * and how many orders it wrote, and what the first of them finds: for fts
 * how many records AdsSetFilter("CONTAINS(NOTES, 'quick')") leaves (with
 * AdsGetRecordCount(ADS_RESPECTFILTERS)), else the records from AdsGotoTop
 * of the order on ([] where nothing could be read). */
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
    static const char *orders[] = {"fts", "index", "compound"};
    static const char *notes[] = {"Quick brown fox jumps over the lazy dog",
                                  "The lazy dog sleeps in the sun",
                                  "Quick fox returns to the den at dusk"};
    static const double ids[] = {3, 1, 2};
    int first = 1;
    printf("[\n");
    for (int t = 0; t < 4; t++) {
        for (int o = 0; o < 3; o++) {
            if (types[t].type == 1 && o == 2) continue;
            char name[16];
            H table = 0;
            snprintf(name, sizeof name, "x%u_%d", types[t].type, o);
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *))F(
                "AdsCreateTable"))(conn, (U8 *)name, NULL, types[t].type, 1, 0, 1, 0,
                                   (U8 *)"ID,N,4,0;NOTES,C,40,0", &table);
            if (rc) return printf("create %s: %lu\n", name, rc), 1;
            for (int r = 0; r < 3; r++) {
                ((U32(WINAPI *)(H))F("AdsAppendRecord"))(table);
                ((U32(WINAPI *)(H, U8 *, double))F("AdsSetDouble"))(table, (U8 *)"ID", ids[r]);
                ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetString"))(
                    table, (U8 *)"NOTES", (U8 *)notes[r], (U32)strlen(notes[r]));
                ((U32(WINAPI *)(H))F("AdsWriteRecord"))(table);
            }
            char file[24];
            snprintf(file, sizeof file, o == 0 ? "%s.fts" : "%s.xyz", name);
            H order = 0;
            U8 empty[2] = {0, 0};
            U32 made = o == 0
                ? ((Fts)F("AdsCreateFTSIndex"))(table, (U8 *)file, (U8 *)"NOTESFTS",
                                                (U8 *)"NOTES", 0, 3, 30, 1, empty, 1, empty, 1,
                                                empty, 1, empty, empty, empty, 0)
                : ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, U32, H *))F(
                      "AdsCreateIndex61"))(table, (U8 *)file, (U8 *)"K", (U8 *)"ID", (U8 *)"",
                                           (U8 *)"", o == 2 ? 2 : 0, 512, &order);
            ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
            table = 0;
            ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
                conn, (U8 *)name, NULL, types[t].type, 1, 0, 1, 1, &table);
            H opened[8];
            U16 count = 8;
            U32 open = ((U32(WINAPI *)(H, U8 *, H *, U16 *))F("AdsOpenIndex"))(
                table, (U8 *)file, opened, &count);
            if (open && open != 5065) count = 0;
            printf("%s{\"type\": \"%s\", \"order\": \"%s\", \"create\": %lu, \"open\": %lu, "
                   "\"orders\": %u, \"found\": ",
                   first ? "" : ",\n", types[t].name, orders[o], made, open, count);
            first = 0;
            if (o == 0) {
                U32 found = 9;
                if (((U32(WINAPI *)(H, U8 *))F("AdsSetFilter"))(
                        table, (U8 *)"CONTAINS(NOTES, 'quick')") == 0)
                    ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(table, 1, &found);
                printf("%lu}", found);
            } else {
                printf("[");
                if (count) {
                    ((U32(WINAPI *)(H))F("AdsGotoTop"))(opened[0]);
                    for (int i = 0; i < 5; i++) {
                        U16 eof = 0;
                        U32 recno = 0;
                        ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(table, &eof);
                        if (eof) break;
                        ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordNum"))(table, 2, &recno);
                        printf("%s%lu", i ? ", " : "", recno);
                        ((U32(WINAPI *)(H, U32))F("AdsSkip"))(opened[0], 1);
                    }
                }
                printf("]}");
            }
            ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
        }
    }
    printf("\n]\n");
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
    return 0;
}
