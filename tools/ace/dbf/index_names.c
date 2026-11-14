/* Records the names AdsGetIndexName gives orders, by how they were made and
 * found, for fixtures/dbf/index_names.json.
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o index_names.exe index_names.c
 *     cd <empty folder> && wine index_names.exe <ace32.dll> <the folder as a Windows path>
 *
 * The original resolves relative table names against the current directory,
 * so the folder must be the current one too. For each kind of order (ADT;
 * NTX; CDX and VFP without ADS_COMPOUND, an IDX file; CDX and VFP with it)
 * and each way of naming its file ("bag": lower case with the extension,
 * mixed case, upper case, without the extension, the full path), the probe
 * makes a table of the field K C(4) with one record, named after the case
 * (ta0, tb1, ...), and two orders with the tags "tg_n" and "TG_C" in the
 * file (an NTX order's file is named for the bag with the tag after it,
 * since one file holds one order). Each row is
 *
 *     {"kind", "bag", "step", "rc", "names": [...]}
 *
 * with the name of each order of the table (AdsGetIndexHandleByOrder from
 * 1 to AdsGetNumIndexes), and for the steps "create" (the handles
 * AdsCreateIndex61 gave, as "created"), "closed and opened"
 * (AdsCloseAllIndexes, then AdsOpenIndex of the file as the bag names it:
 * "opened" lists the handles it wrote) and "reopened" (the table closed and
 * opened again, AdsOpenIndex of the file unless it opened with the table). */
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
static const char *kind_name, *bag_kind;

static void name_of(H order, char *out) {
    U8 name[260] = "";
    U16 len = sizeof name;
    U32 rc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsGetIndexName"))(order, name, &len);
    if (rc) snprintf(out, 64, "E%lu", rc);
    else snprintf(out, 260, "%s", (char *)name);
}

static void list(const char *key, const H *orders, int n) {
    printf(", \"%s\": [", key);
    for (int i = 0; i < n; i++) {
        char name[260];
        name_of(orders[i], name);
        printf("%s\"%s\"", i ? ", " : "", name);
    }
    printf("]");
}

static void head(const char *step, U32 rc, H table) {
    U16 n = 0;
    H orders[16];
    ((U32(WINAPI *)(H, U16 *))F("AdsGetNumIndexes"))(table, &n);
    for (U16 i = 0; i < n && i < 16; i++)
        ((U32(WINAPI *)(H, U16, H *))F("AdsGetIndexHandleByOrder"))(table, i + 1, &orders[i]);
    printf("%s{\"kind\": \"%s\", \"bag\": \"%s\", \"step\": \"%s\", \"rc\": %lu",
           first ? "" : ",\n", kind_name, bag_kind, step, rc);
    first = 0;
    list("names", orders, n < 16 ? n : 16);
}

int main(int argc, char **argv) {
    if (argc != 3 || !argv[2][0]) return 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) return 1;
    H conn = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
        (U8 *)argv[2], 1, NULL, NULL, 0, &conn);
    if (rc) return printf("connect: %lu\n", rc), 1;
    static const struct { const char *name, *ext; U16 type; U32 options; } kinds[] = {
        {"ADT", "adi", 3, 0},  {"NTX", "ntx", 1, 0},       {"CDX idx", "cdx", 2, 0},
        {"VFP idx", "cdx", 4, 0}, {"CDX compound", "cdx", 2, 2}, {"VFP compound", "cdx", 4, 2},
    };
    static const char *bags[] = {"lower", "mixed", "upper", "bare", "full"};
    printf("[\n");
    for (int k = 0; k < 6; k++) {
        kind_name = kinds[k].name;
        int ntx = kinds[k].type == 1;
        for (int b = 0; b < 5; b++) {
            char table_name[16], stems[2][32], files[2][400], ext[8];
            H table = 0, created[2] = {0, 0};
            bag_kind = bags[b];
            snprintf(table_name, sizeof table_name, "t%c%d", 'a' + k, b);
            for (int i = 0; i < (int)strlen(kinds[k].ext) + 1; i++)
                ext[i] = b == 2 ? (char)toupper(kinds[k].ext[i]) : kinds[k].ext[i];
            for (int o = 0; o < 2; o++) {
                const char *base = b == 1 ? "Bag_" : b == 2 ? "BAG_" : "bag_";
                snprintf(stems[o], sizeof stems[o], "%s%c%d%s", base, 'a' + k,
                         b, ntx ? (o ? "c" : "n") : "");
                if (b == 3) snprintf(files[o], sizeof files[o], "%s", stems[o]);
                else if (b == 4)
                    snprintf(files[o], sizeof files[o], "%s\\%s.%s", argv[2], stems[o], ext);
                else snprintf(files[o], sizeof files[o], "%s.%s", stems[o], ext);
            }
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *))F(
                "AdsCreateTable"))(conn, (U8 *)table_name, NULL, kinds[k].type, 1, 0, 1, 0,
                                   (U8 *)"K,C,4,0", &table);
            if (rc) return printf("create %s: %lu\n", table_name, rc), 1;
            ((U32(WINAPI *)(H))F("AdsAppendRecord"))(table);
            ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetString"))(table, (U8 *)"K", (U8 *)"A",
                                                                    1);
            ((U32(WINAPI *)(H))F("AdsWriteRecord"))(table);
            static const char *tags[] = {"tg_n", "TG_C"};
            U32 rcs = 0;
            for (int o = 0; o < 2; o++) {
                rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, U32, H *))F(
                    "AdsCreateIndex61"))(table, (U8 *)files[ntx ? o : 0], (U8 *)tags[o],
                                         (U8 *)"K", (U8 *)"", (U8 *)"", kinds[k].options, 512,
                                         &created[o]);
                if (rc) rcs = rc;
            }
            head("create", rcs, table);
            list("created", created, 2);
            printf("}");

            ((U32(WINAPI *)(H))F("AdsCloseAllIndexes"))(table);
            H opened[16] = {0};
            U16 len = 16;
            rc = ((U32(WINAPI *)(H, U8 *, H *, U16 *))F("AdsOpenIndex"))(table, (U8 *)files[0],
                                                                       opened, &len);
            if (ntx) {
                H more[16] = {0};
                U16 more_len = 16;
                ((U32(WINAPI *)(H, U8 *, H *, U16 *))F("AdsOpenIndex"))(table, (U8 *)files[1],
                                                                      more, &more_len);
            }
            head("closed and opened", rc, table);
            list("opened", opened, rc == 0 || rc == 5065 ? len : 0);
            printf("}");
            ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);

            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
                conn, (U8 *)table_name, NULL, kinds[k].type, 1, 0, 1, 1, &table);
            if (rc) return printf("open %s: %lu\n", table_name, rc), 1;
            U16 n = 0;
            ((U32(WINAPI *)(H, U16 *))F("AdsGetNumIndexes"))(table, &n);
            memset(opened, 0, sizeof opened);
            len = 16;
            rc = 0;
            if (n == 0)
                rc = ((U32(WINAPI *)(H, U8 *, H *, U16 *))F("AdsOpenIndex"))(
                    table, (U8 *)files[0], opened, &len);
            head("reopened", rc, table);
            list("opened", opened, n == 0 && (rc == 0 || rc == 5065) ? len : 0);
            printf("}");
            ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
        }
    }
    printf("\n]\n");
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
    return 0;
}
