/* What opening a table does when its ADI index was built under another
 * collation than the connection's, with each of the collation mismatch open
 * options (ADS_REINDEX_ON_COLLATION_MISMATCH 0x100, ADS_IGNORE_COLLATION_MISMATCH
 * 0x200):
 *
 *     mismatch.exe <ace32.dll> <data folder> <table> <index file> <table type> <options hex>
 *                  [<other user: 0|1> [<collation>]]
 *
 * Built with i686-w64-mingw32-gcc, run under Wine by run_mismatch.sh, which
 * wraps the answers in fixtures/collation/mismatch.json. The table (type 3
 * ADT, ANSI, proprietary locking, checking rights) is opened with AdsOpenTable,
 * or AdsOpenTable90 when a collation is given; with `other user` a second
 * connection opens it shared first (with the ignore option). Prints one JSON
 * object: the open's return code and error text, and if it opened the number
 * of orders, the table's and first order's collations, a soft seek of the key
 * E4 in that order (found, record number), the first eight records of a walk
 * from its top, and after closing the CRC (the 32-bit word at 0x14) and size
 * of the index file, which the original engine rewrites to reindex. */
#include <windows.h>
#include <stdio.h>
#include <stdlib.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;

static HMODULE ace;
#define F(name) GetProcAddress(ace, name)

static void error(void) {
    U32 code = 0;
    U8 text[600];
    U16 len = sizeof text;
    ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
    printf(", \"error\": %lu, \"text\": \"", code);
    for (int i = 0; i < len; i++) {
        if (text[i] == '\\' || text[i] == '"') putchar('\\');
        putchar(text[i]);
    }
    putchar('"');
}

static H open_table(H connection, const char *path, U16 type, U32 options, const char *collation,
                    U32 *rc) {
    H table = 0;
    *rc = collation ? ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, U8 *, H *))F(
                          "AdsOpenTable90"))(connection, (U8 *)path, NULL, type, 1, 1, 2, options,
                                             (U8 *)collation, &table)
                    : ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F(
                          "AdsOpenTable"))(connection, (U8 *)path, NULL, type, 1, 1, 2, options,
                                           &table);
    return table;
}

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IONBF, 0);
    if (argc < 7) return fprintf(stderr, "usage: see the header\n"), 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) return fprintf(stderr, "cannot load %s\n", argv[1]), 1;
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    H connection = 0;
    if (((U32(WINAPI *)(U8 *, U16, H *))F("AdsConnect26"))((U8 *)argv[2], 1, &connection))
        return fprintf(stderr, "connect failed\n"), 1;
    char path[1024], index[1024];
    snprintf(path, sizeof path, "%s\\%s", argv[2], argv[3]);
    snprintf(index, sizeof index, "%s\\%s", argv[2], argv[4]);
    U16 type = (U16)atoi(argv[5]);
    U32 options = strtoul(argv[6], NULL, 16);
    int other = argc > 7 && atoi(argv[7]);
    const char *collation = argc > 8 ? argv[8] : NULL;
    if (other) {
        H second = 0, held = 0;
        U32 rc;
        ((U32(WINAPI *)(U8 *, U16, H *))F("AdsConnect26"))((U8 *)argv[2], 1, &second);
        held = open_table(second, path, type, 0x204, NULL, &rc);
        if (rc) return fprintf(stderr, "the other user's open: %lu\n", rc), 1;
        (void)held;
    }
    U32 rc;
    H table = open_table(connection, path, type, options, collation, &rc);
    printf("{\"open\": %lu", rc);
    if (rc) error();
    if (!rc) {
        U16 count = 0;
        ((U32(WINAPI *)(H, U16 *))F("AdsGetNumIndexes"))(table, &count);
        printf(", \"indexes\": %u", count);
        U8 name[200];
        U16 len = sizeof name;
        rc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsGetTableCollation"))(table, name, &len);
        printf(", \"table_collation\": \"%.*s\"", rc ? 0 : (int)len, name);
        if (count) {
            H order = 0;
            ((U32(WINAPI *)(H, U16, H *))F("AdsGetIndexHandleByOrder"))(table, 1, &order);
            len = sizeof name;
            rc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsGetIndexCollation"))(order, name, &len);
            printf(", \"index_collation\": \"%.*s\"", rc ? 0 : (int)len, name);
            U16 found = 0;
            U32 seek = ((U32(WINAPI *)(H, U8 *, U16, U16, U16, U16 *))F("AdsSeek"))(
                order, (U8 *)"\xe4", 1, 2, 1, &found);
            U32 record = 0;
            ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordNum"))(table, 2, &record);
            printf(", \"seek\": [%lu, %u, %lu]", seek, found, record);
            ((U32(WINAPI *)(H))F("AdsGotoTop"))(order);
            printf(", \"walk\": [");
            for (int i = 0; i < 8; i++) {
                ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordNum"))(table, 2, &record);
                printf("%s%lu", i ? ", " : "", record);
                ((U32(WINAPI *)(H, long))F("AdsSkip"))(order, 1);
            }
            printf("]");
        }
        ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
    }
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(connection);
    FILE *file = fopen(index, "rb");
    if (file) {
        U8 header[0x18];
        fread(header, 1, sizeof header, file);
        fseek(file, 0, SEEK_END);
        printf(", \"index_crc\": \"%02x%02x%02x%02x\", \"index_size\": %ld", header[0x17],
               header[0x16], header[0x15], header[0x14], ftell(file));
        fclose(file);
    }
    printf("}\n");
    return 0;
}
