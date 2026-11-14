/* Records how an Advantage Client Engine counts, numbers and seeks the keys
 * of an index order when records are deleted, shown and hidden
 * (AdsShowDeleted), for fixtures/seek/deleted.json.
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o deleted.exe deleted.c
 *     cd <empty folder> && wine deleted.exe <ace32.dll> <the folder as a Windows path>
 *
 * The original resolves relative table names against the current directory,
 * so the folder must be the current one too. For each table type (ADT, NTX,
 * CDX, VFP) the probe makes a table of 8 records whose field K holds
 *
 *     1 A1   2 A2   3 A3   4 B1   5 B2   6 B3   7 C1   8 C2
 *
 * with an order on K, and deletes records 1 (the first of the A group), 6
 * (the last of B), 7 and 8 (all of C). Then, with deleted records shown
 * (show 1) and hidden (show 0), each row is one call or walk:
 *
 *     {"type", "show", "op": "keycount"|"recordcount"|"keynum", "option",
 *      "rc", "n"}                       n is the count (777 if untouched);
 *                                       keynum is taken at the order's top
 *     {..., "op": "scoped keycount", "option", "rc", "n"}
 *                                       with the scope B to B
 *     {..., "op": "seek", "key", "seek_type", "rc", "found", "at"}
 *     {..., "op": "seeklast", "key", "rc", "found", "at"}
 *     {..., "op": "walk", "rows": [records from the top to EOF]}
 *     {..., "op": "aof keycount"|"aof scoped keycount"|"aof keynum", ...}
 *                                       the counts again with the AOF
 *                                       K <> 'A2' (ADS_RESOLVE_DYNAMIC)
 *     {..., "op": "aof seek"|"aof seeklast"|"filter seek"|"filter seeklast"}
 *                                       the seeks again with the AOF, then
 *                                       the filter (AdsSetFilter),
 *                                       K <> 'A1' .AND. K <> 'B3'
 *
 * "at" is the record AdsGetRecordNum(ADS_IGNOREFILTERS) gives after the
 * call, "E" at EOF, "B" at BOF. Seeks are ADS_STRINGKEY and start from
 * record 2. Last, with deleted records hidden, record 2 is deleted without
 * AdsWriteRecord and the key count taken again ("op": "keycount after
 * delete"). */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;
static HMODULE ace;
#define F(name) ((void *)GetProcAddress(ace, name))

static int first = 1;
static const char *type_name;
static int show;

static void head(const char *op) {
    printf("%s{\"type\": \"%s\", \"show\": %d, \"op\": \"%s\"", first ? "" : ",\n", type_name,
           show, op);
    first = 0;
}

static void op(const char *prefix, const char *name) {
    char full[64];
    snprintf(full, sizeof full, "%s%s", prefix, name);
    head(full);
}

static void at(H table) {
    U16 bof = 0, eof = 0;
    U32 recno = 0;
    ((U32(WINAPI *)(H, U16 *))F("AdsAtBOF"))(table, &bof);
    ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(table, &eof);
    ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordNum"))(table, 2, &recno);
    if (eof) printf(", \"at\": \"E\"");
    else if (bof) printf(", \"at\": \"B\"");
    else printf(", \"at\": %lu", recno);
}

static void count(const char *op, const char *function, H handle, U16 option) {
    U32 n = 777;
    U32 rc = ((U32(WINAPI *)(H, U16, U32 *))F(function))(handle, option, &n);
    head(op);
    printf(", \"option\": %u, \"rc\": %lu, \"n\": %lu}", option, rc, n);
}

/* Each key, by each seek type and by AdsSeekLast, from record 2; the ops
 * are named with prefix. */
static void seeks(H table, H order, const char *prefix) {
    static const char *keys[] = {"A", "B", "C", "D", "0", "A1", "A2", "B3", "C1", "B2"};
    static const U16 seek_types[] = {1, 2, 4};
    for (int k = 0; k < (int)(sizeof keys / sizeof *keys); k++) {
        for (int s = 0; s < 3; s++) {
            U16 found = 7;
            ((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(table, 2);
            U32 rc = ((U32(WINAPI *)(H, U8 *, U16, U16, U16, U16 *))F("AdsSeek"))(
                order, (U8 *)keys[k], (U16)strlen(keys[k]), 2, seek_types[s], &found);
            op(prefix, "seek");
            printf(", \"key\": \"%s\", \"seek_type\": %u, \"rc\": %lu, \"found\": %u", keys[k],
                   seek_types[s], rc, found);
            at(table);
            printf("}");
        }
        U16 found = 7;
        ((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(table, 2);
        U32 rc = ((U32(WINAPI *)(H, U8 *, U16, U16, U16 *))F("AdsSeekLast"))(
            order, (U8 *)keys[k], (U16)strlen(keys[k]), 2, &found);
        op(prefix, "seeklast");
        printf(", \"key\": \"%s\", \"rc\": %lu, \"found\": %u", keys[k], rc, found);
        at(table);
        printf("}");
    }
}

static void measure(H table, H order) {
    for (U16 option = 1; option <= 3; option++)
        count("keycount", "AdsGetKeyCount", order, option);
    for (U16 option = 1; option <= 3; option++)
        count("recordcount", "AdsGetRecordCount", table, option);
    ((U32(WINAPI *)(H))F("AdsGotoTop"))(order);
    for (U16 option = 1; option <= 3; option++)
        count("keynum", "AdsGetKeyNum", order, option);
    ((U32(WINAPI *)(H, U16, U8 *, U16, U16))F("AdsSetScope"))(order, 1, (U8 *)"B", 1, 2);
    ((U32(WINAPI *)(H, U16, U8 *, U16, U16))F("AdsSetScope"))(order, 2, (U8 *)"B", 1, 2);
    for (U16 option = 1; option <= 3; option++)
        count("scoped keycount", "AdsGetKeyCount", order, option);
    ((U32(WINAPI *)(H, U16))F("AdsClearScope"))(order, 1);
    ((U32(WINAPI *)(H, U16))F("AdsClearScope"))(order, 2);
    seeks(table, order, "");
    ((U32(WINAPI *)(H, U8 *, U16))F("AdsSetAOF"))(table, (U8 *)"K <> 'A2'", 2);
    for (U16 option = 1; option <= 3; option++)
        count("aof keycount", "AdsGetKeyCount", order, option);
    ((U32(WINAPI *)(H))F("AdsGotoTop"))(order);
    for (U16 option = 1; option <= 3; option++)
        count("aof keynum", "AdsGetKeyNum", order, option);
    ((U32(WINAPI *)(H, U16, U8 *, U16, U16))F("AdsSetScope"))(order, 1, (U8 *)"B", 1, 2);
    ((U32(WINAPI *)(H, U16, U8 *, U16, U16))F("AdsSetScope"))(order, 2, (U8 *)"B", 1, 2);
    for (U16 option = 1; option <= 3; option++)
        count("aof scoped keycount", "AdsGetKeyCount", order, option);
    ((U32(WINAPI *)(H, U16))F("AdsClearScope"))(order, 1);
    ((U32(WINAPI *)(H, U16))F("AdsClearScope"))(order, 2);
    ((U32(WINAPI *)(H))F("AdsClearAOF"))(table);
    ((U32(WINAPI *)(H, U8 *, U16))F("AdsSetAOF"))(table, (U8 *)"K <> 'A1' .AND. K <> 'B3'", 2);
    seeks(table, order, "aof ");
    ((U32(WINAPI *)(H))F("AdsClearAOF"))(table);
    ((U32(WINAPI *)(H, U8 *))F("AdsSetFilter"))(table, (U8 *)"K <> 'A1' .AND. K <> 'B3'");
    seeks(table, order, "filter ");
    ((U32(WINAPI *)(H))F("AdsClearFilter"))(table);
    head("walk");
    printf(", \"rows\": [");
    ((U32(WINAPI *)(H))F("AdsGotoTop"))(order);
    for (int i = 0; i < 20; i++) {
        U16 eof = 0;
        U32 recno = 0;
        ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(table, &eof);
        if (eof) break;
        ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordNum"))(table, 2, &recno);
        printf("%s%lu", i ? ", " : "", recno);
        ((U32(WINAPI *)(H, U32))F("AdsSkip"))(order, 1);
    }
    printf("]}");
}

int main(int argc, char **argv) {
    if (argc != 3 || !argv[2][0]) return 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) return 1;
    H conn = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
        (U8 *)argv[2], 1, NULL, NULL, 0, &conn);
    if (rc) return printf("connect: %lu\n", rc), 1;
    static const struct { const char *name, *table, *bag; U16 type; } types[] = {
        {"ADT", "ta.adt", "ta.adi", 3},
        {"NTX", "tn.dbf", "tn_k.ntx", 1},
        {"CDX", "tc.dbf", "tc.cdx", 2},
        {"VFP", "tv.dbf", "tv.cdx", 4},
    };
    static const char *values[] = {"A1", "A2", "A3", "B1", "B2", "B3", "C1", "C2"};
    printf("[\n");
    for (int t = 0; t < 4; t++) {
        H table = 0, order = 0;
        type_name = types[t].name;
        ((U32(WINAPI *)(U16))F("AdsShowDeleted"))(1);
        rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *))F(
            "AdsCreateTable"))(conn, (U8 *)types[t].table, NULL, types[t].type, 1, 1, 1, 0,
                               (U8 *)"K,C,8,0", &table);
        if (rc) return printf("create %s: %lu\n", types[t].table, rc), 1;
        for (int i = 0; i < 8; i++) {
            ((U32(WINAPI *)(H))F("AdsAppendRecord"))(table);
            ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetString"))(table, (U8 *)"K",
                                                                    (U8 *)values[i], 2);
            ((U32(WINAPI *)(H))F("AdsWriteRecord"))(table);
        }
        rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, U32, H *))F(
            "AdsCreateIndex61"))(table, (U8 *)types[t].bag, (U8 *)"K", (U8 *)"K", (U8 *)"",
                                 (U8 *)"", 0, 512, &order);
        if (rc) return printf("index %s: %lu\n", types[t].bag, rc), 1;
        static const U32 gone[] = {1, 6, 7, 8};
        for (int i = 0; i < 4; i++) {
            ((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(table, gone[i]);
            ((U32(WINAPI *)(H))F("AdsDeleteRecord"))(table);
            ((U32(WINAPI *)(H))F("AdsWriteRecord"))(table);
        }
        for (show = 1; show >= 0; show--) {
            ((U32(WINAPI *)(U16))F("AdsShowDeleted"))((U16)show);
            measure(table, order);
        }
        show = 0;
        ((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(table, 2);
        ((U32(WINAPI *)(H))F("AdsDeleteRecord"))(table);
        count("keycount after delete", "AdsGetKeyCount", order, 1);
        ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
    }
    ((U32(WINAPI *)(U16))F("AdsShowDeleted"))(1);
    printf("\n]\n");
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
    return 0;
}
