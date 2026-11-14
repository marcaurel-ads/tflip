/* Records how an Advantage Client Engine takes index files named with the
 * extension of another table type's format (.ntx, .cdx, .idx, .adi), for
 * fixtures/errors/foreign_extensions.json.
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o foreign_extensions.exe foreign_extensions.c
 *     cd <empty folder> && wine foreign_extensions.exe <ace32.dll> <the folder as a Windows path>
 *
 * The original resolves relative table names against the current directory,
 * so the folder must be the current one too. For each table type (NTX, CDX,
 * ADT, VFP), each extension and each of single and compound (ADS_COMPOUND),
 * the probe makes the table f<type>_<n> of the field ID N(4) with the
 * records 3, 1, 2, makes the order K on ID into f<type>_<n>.<extension> with
 * AdsCreateIndex61, reads the records through it, closes the table, opens it
 * again and opens the file by its name with AdsOpenIndex. Each row is
 *
 *     {"type", "extension", "compound", "create", "records", "open",
 *      "orders", "reopened"}
 *
 * "records" and "reopened" being the record numbers from AdsGotoTop and
 * AdsSkip of the order made and of the first order opened ([] where there
 * was none), "orders" how many AdsOpenIndex opened. */
#include <windows.h>
#include <stdio.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;
static HMODULE ace;
#define F(name) ((void *)GetProcAddress(ace, name))

static void records(H table, H order) {
    printf("[");
    if (order) {
        ((U32(WINAPI *)(H))F("AdsGotoTop"))(order);
        for (int i = 0; i < 5; i++) {
            U16 eof = 1;
            U32 recno = 0;
            if (((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(table, &eof) || eof) break;
            ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordNum"))(table, 2, &recno);
            printf("%s%lu", i ? ", " : "", recno);
            ((U32(WINAPI *)(H, U32))F("AdsSkip"))(order, 1);
        }
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
    static const struct { const char *name; U16 type; } types[] = {
        {"NTX", 1}, {"CDX", 2}, {"ADT", 3}, {"VFP", 4}};
    static const char *extensions[] = {"ntx", "cdx", "idx", "adi"};
    static const double ids[] = {3, 1, 2};
    int first = 1;
    printf("[\n");
    for (int t = 0; t < 4; t++) {
        for (int e = 0; e < 4; e++) {
            for (int compound = 0; compound < 2; compound++) {
                char name[16], file[24];
                H table = 0, order = 0;
                snprintf(name, sizeof name, "f%u_%d", types[t].type, e * 2 + compound);
                snprintf(file, sizeof file, "%s.%s", name, extensions[e]);
                rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *))F(
                    "AdsCreateTable"))(conn, (U8 *)name, NULL, types[t].type, 1, 0, 1, 0,
                                       (U8 *)"ID,N,4,0", &table);
                if (rc) return printf("create %s: %lu\n", name, rc), 1;
                for (int r = 0; r < 3; r++) {
                    ((U32(WINAPI *)(H))F("AdsAppendRecord"))(table);
                    ((U32(WINAPI *)(H, U8 *, double))F("AdsSetDouble"))(table, (U8 *)"ID", ids[r]);
                    ((U32(WINAPI *)(H))F("AdsWriteRecord"))(table);
                }
                U32 made = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, U32, H *))F(
                    "AdsCreateIndex61"))(table, (U8 *)file, (U8 *)"K", (U8 *)"ID", (U8 *)"",
                                         (U8 *)"", compound ? 2 : 0, 512, &order);
                printf("%s{\"type\": \"%s\", \"extension\": \"%s\", \"compound\": %d, "
                       "\"create\": %lu, \"records\": ",
                       first ? "" : ",\n", types[t].name, extensions[e], compound, made);
                first = 0;
                records(table, made ? 0 : order);
                ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
                table = 0;
                ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
                    conn, (U8 *)name, NULL, types[t].type, 1, 0, 1, 1, &table);
                H opened[8];
                U16 count = 8;
                U32 open = ((U32(WINAPI *)(H, U8 *, H *, U16 *))F("AdsOpenIndex"))(
                    table, (U8 *)file, opened, &count);
                if (open && open != 5065) count = 0;
                printf(", \"open\": %lu, \"orders\": %u, \"reopened\": ", open, count);
                records(table, count ? opened[0] : 0);
                printf("}");
                ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
            }
        }
    }
    printf("\n]\n");
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
    return 0;
}
