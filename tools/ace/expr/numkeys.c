/* Records how an Advantage Client Engine keys and seeks index orders on
 * numeric expressions, for fixtures/expr/numkeys.json.
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o numkeys.exe numkeys.c
 *     cd <empty folder> && wine numkeys.exe <ace32.dll> <the folder as a Windows path>
 *
 * The original resolves relative table names against the current directory,
 * so the folder must be the current one too. For each table type (ADT, NTX,
 * CDX, VFP) the probe makes the table n<type> of the fields ID N(6,0),
 * D N(8,2) and C C(4) and the records
 *
 *     1 (1001, 2.50, 'a')   2 (1002, -1.25, 'b')   3 (1003, 10.00, 'c')
 *     4 (999, 0.00, 'd')
 *
 * and, for each expression below, an order on it in a file of its own
 * (AdsCreateIndex61, compound on CDX and VFP). Last, as type "ADT types",
 * the ADT table ntypes adds the fields I Integer and F Double holding ID's
 * values, with orders on ID + 1, 1 + ID, I + 1, F + 1, I * 2, D + ID and I:
 * the original seeks an expression of one Numeric field by putting the
 * number in the field, but not one of other fields. Each row is
 *
 *     {"type", "expr", "rc", "key_length", "key_type", "keys": [hex...],
 *      "walk": [records], "seeks": [{"key", "how", "rc", "found", "at"}]}
 *
 * "keys" holds AdsExtractKey of records 1 to 4 in hex, "walk" the records
 * from the top to EOF. The seeks: the doubles 1003, 1002.5 and -1 hard
 * (ADS_DOUBLEKEY), 1002.5 soft, and "1003" as a string (ADS_STRINGKEY,
 * hard); "at" is the record after it, "E" at EOF. */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;
static HMODULE ace;
#define F(name) ((void *)GetProcAddress(ace, name))

static const char *exprs[] = {
    "ID", "ID + 1", "ID * 2", "ID - 1000", "D", "D + 1", "ID + D", "ID / 4", "-ID", NULL,
};
static const char *type_exprs[] = {
    "ID + 1", "1 + ID", "I + 1", "F + 1", "I * 2", "D + ID", "I", NULL,
};

static void at(H table) {
    U16 eof = 0;
    U32 recno = 0;
    ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(table, &eof);
    ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordNum"))(table, 2, &recno);
    if (eof) printf(", \"at\": \"E\"");
    else printf(", \"at\": %lu", recno);
}

int main(int argc, char **argv) {
    if (argc != 3 || !argv[2][0]) return 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) return 1;
    H conn = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
        (U8 *)argv[2], 1, NULL, NULL, 0, &conn);
    if (rc) return printf("connect: %lu\n", rc), 1;
#define PLAIN "ID,N,6,0;D,N,8,2;C,C,4,0"
#define TYPED "ID,N,6,0;D,N,8,2;C,C,4,0;I,Integer;F,Double,2"
    static const struct { const char *name, *table, *ext, *fields, **exprs; U16 type; } types[] = {
        {"ADT", "nadt", "adi", PLAIN, exprs, 3},
        {"NTX", "nntx", "ntx", PLAIN, exprs, 1},
        {"CDX", "ncdx", "cdx", PLAIN, exprs, 2},
        {"VFP", "nvfp", "cdx", PLAIN, exprs, 4},
        {"ADT types", "ntypes", "adi", TYPED, type_exprs, 3},
    };
    static const char *ids[] = {"1001", "1002", "1003", "999"};
    static const char *ds[] = {"2.50", "-1.25", "10.00", "0.00"};
    static const char *cs[] = {"a", "b", "c", "d"};
    int first = 1;
    printf("[\n");
    for (int t = 0; t < 5; t++) {
        H table = 0;
        rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *))F(
            "AdsCreateTable"))(conn, (U8 *)types[t].table, NULL, types[t].type, 1, 0, 1, 0,
                               (U8 *)types[t].fields, &table);
        if (rc) return printf("create %s: %lu\n", types[t].table, rc), 1;
        for (int i = 0; i < 4; i++) {
            ((U32(WINAPI *)(H))F("AdsAppendRecord"))(table);
            ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetField"))(table, (U8 *)"ID", (U8 *)ids[i],
                                                                   strlen(ids[i]));
            ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetField"))(table, (U8 *)"D", (U8 *)ds[i],
                                                                   strlen(ds[i]));
            ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetField"))(table, (U8 *)"C", (U8 *)cs[i], 1);
            if (types[t].exprs == type_exprs) {
                ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetField"))(table, (U8 *)"I",
                                                                       (U8 *)ids[i], strlen(ids[i]));
                ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetField"))(table, (U8 *)"F",
                                                                       (U8 *)ids[i], strlen(ids[i]));
            }
            ((U32(WINAPI *)(H))F("AdsWriteRecord"))(table);
        }
        for (int e = 0; types[t].exprs[e]; e++) {
            const char *expr = types[t].exprs[e];
            char bag[32];
            H order = 0;
            snprintf(bag, sizeof bag, "k%d_%d.%s", t, e, types[t].ext);
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, U32, H *))F(
                "AdsCreateIndex61"))(table, (U8 *)bag, (U8 *)"K", (U8 *)expr, (U8 *)"",
                                     (U8 *)"", types[t].type == 1 ? 0 : 2, 512, &order);
            printf("%s{\"type\": \"%s\", \"expr\": \"%s\", \"rc\": %lu", first ? "" : ",\n",
                   types[t].name, expr, rc);
            first = 0;
            if (rc) {
                printf("}");
                continue;
            }
            U16 key_length = 0, key_type = 0;
            ((U32(WINAPI *)(H, U16 *))F("AdsGetKeyLength"))(order, &key_length);
            ((U32(WINAPI *)(H, U16 *))F("AdsGetKeyType"))(order, &key_type);
            printf(", \"key_length\": %u, \"key_type\": %u, \"keys\": [", key_length, key_type);
            for (U32 r = 1; r <= 4; r++) {
                U8 key[300];
                U16 len = sizeof key;
                ((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(table, r);
                U32 krc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsExtractKey"))(order, key, &len);
                printf("%s\"", r > 1 ? ", " : "");
                if (krc) printf("E%lu", krc);
                else for (U16 i = 0; i < len; i++) printf("%02x", key[i]);
                printf("\"");
            }
            printf("], \"walk\": [");
            ((U32(WINAPI *)(H))F("AdsGotoTop"))(order);
            for (int i = 0; i < 10; i++) {
                U16 eof = 0;
                U32 recno = 0;
                ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(table, &eof);
                if (eof) break;
                ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordNum"))(table, 2, &recno);
                printf("%s%lu", i ? ", " : "", recno);
                ((U32(WINAPI *)(H, U32))F("AdsSkip"))(order, 1);
            }
            printf("], \"seeks\": [");
            static const struct { double value; U16 how; } doubles[] = {
                {1003, 2}, {1002.5, 2}, {-1, 2}, {1002.5, 1}};
            for (int s = 0; s < 5; s++) {
                U16 found = 7;
                U32 src;
                ((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(table, 1);
                if (s < 4) {
                    double v = doubles[s].value;
                    src = ((U32(WINAPI *)(H, U8 *, U16, U16, U16, U16 *))F("AdsSeek"))(
                        order, (U8 *)&v, 8, 4, doubles[s].how, &found);
                    printf("%s{\"key\": %g, \"how\": \"%s\"", s ? ", " : "", v,
                           doubles[s].how == 1 ? "soft" : "hard");
                } else {
                    src = ((U32(WINAPI *)(H, U8 *, U16, U16, U16, U16 *))F("AdsSeek"))(
                        order, (U8 *)"1003", 4, 2, 2, &found);
                    printf(", {\"key\": \"1003\", \"how\": \"string\"");
                }
                printf(", \"rc\": %lu, \"found\": %u", src, found);
                at(table);
                printf("}");
            }
            printf("]}");
        }
        ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
    }
    printf("\n]\n");
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
    return 0;
}
