/* Records how an Advantage Client Engine reads field names that are also
 * words of its expression language (AND, OR, NOT, TRUE, FALSE, NULL), in
 * filters and index keys, for fixtures/expr/keywords.json.
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o keywords.exe keywords.c
 *     cd <empty folder> && wine keywords.exe <ace32.dll> <the folder as a Windows path>
 *
 * The original resolves relative table names against the current directory,
 * so the folder must be the current one too. For each table type (ADT, NTX,
 * CDX) the probe makes two tables:
 *
 *   "kw"     the fields AND, OR, TRUE, FALSE, NULL C(2), NOT and X Logical,
 *            with the records
 *                1 ('a', 'b', 'a', 'b', 'a', .T., .T.)
 *                2 ('b', 'a', 'b', 'a', 'b', .F., .T.)
 *                3 ('a', 'a', 'a', 'a', 'a', .F., .F.)
 *   "plain"  the fields C C(2) and X Logical, with the records ('a', .T.),
 *            ('b', .T.), ('a', .F.)
 *
 * and gives, for each expression of the lists below, the rows
 *
 *     {"type", "table", "op": "filter", "expr", "rc", "count_rc", "count"}
 *              AdsSetFilter, then AdsGetRecordCount(ADS_RESPECTFILTERS) and
 *              what it returned; "count" is 777 where it was left alone
 *     {"type", "table", "op": "index", "expr", "rc", "walk": [records]}
 *              AdsCreateIndex61 on the expression in a bag of its own,
 *              then the records in its order */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;
static HMODULE ace;
#define F(name) ((void *)GetProcAddress(ace, name))

static const char *kw_filters[] = {
    "AND = 'a'", "OR = 'a'", "TRUE = 'a'", "FALSE = 'a'", "NULL = 'a'",
    "NOT", "NOT = .T.", ".NOT. NOT", "NOT NOT", "NOT X", "X AND NOT", "X .AND. NOT",
    "NOT AND X", "NOT OR X", "NOT .AND. X", "AND = 'a' AND OR = 'a'",
    "AND = 'a' OR OR = 'a'", "AND = 'a' .AND. OR = 'a'", "TRUE", "FALSE", "NULL",
    "X = TRUE", "X = FALSE", "AND", "OR", "AND = OR", "TRUE = FALSE",
    "NULL = 'a' AND NOT", "UPPER(AND) = 'A'", "AND $ 'ab'", NULL,
};
static const char *kw_indexes[] = {
    "AND", "OR", "TRUE", "FALSE", "NULL", "UPPER(AND)", "AND+OR", "AND + OR", "NOT",
    "TRUE+FALSE", "IIF(NOT, AND, OR)", NULL,
};
static const char *plain_filters[] = {
    "X AND X", "X OR X", "NOT X", "X AND NOT X", "TRUE", "FALSE", "NULL", "X = TRUE",
    "X = FALSE", "AND = 'a'", "C = NULL", "NOT", NULL,
};
static const char *plain_indexes[] = {"TRUE", "NULL", "AND", NULL};

static int first = 1;

static void filters(H table, const char *type, const char *name, const char **list) {
    for (int e = 0; list[e]; e++) {
        U32 count = 777, count_rc = 0;
        U32 rc = ((U32(WINAPI *)(H, U8 *))F("AdsSetFilter"))(table, (U8 *)list[e]);
        if (rc == 0) {
            count_rc = ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(table, 1, &count);
            ((U32(WINAPI *)(H))F("AdsClearFilter"))(table);
        }
        printf("%s{\"type\": \"%s\", \"table\": \"%s\", \"op\": \"filter\", \"expr\": \"%s\", "
               "\"rc\": %lu, \"count_rc\": %lu, \"count\": %lu}",
               first ? "" : ",\n", type, name, list[e], rc, count_rc, count);
        first = 0;
    }
}

static void indexes(H table, const char *type, const char *name, const char *ext,
                    U32 options, const char **list) {
    for (int e = 0; list[e]; e++) {
        char bag[32];
        H order = 0;
        snprintf(bag, sizeof bag, "%c%d.%s", name[0], e, ext);
        U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, U32, H *))F(
            "AdsCreateIndex61"))(table, (U8 *)bag, (U8 *)"T", (U8 *)list[e], (U8 *)"",
                                 (U8 *)"", options, 512, &order);
        printf(",\n{\"type\": \"%s\", \"table\": \"%s\", \"op\": \"index\", \"expr\": \"%s\", "
               "\"rc\": %lu, \"walk\": [",
               type, name, list[e], rc);
        if (rc == 0) {
            ((U32(WINAPI *)(H))F("AdsGotoTop"))(order);
            for (int i = 0; i < 5; i++) {
                U16 eof = 0;
                U32 recno = 0;
                ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(table, &eof);
                if (eof) break;
                ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordNum"))(table, 2, &recno);
                printf("%s%lu", i ? ", " : "", recno);
                ((U32(WINAPI *)(H, U32))F("AdsSkip"))(order, 1);
            }
            ((U32(WINAPI *)(H))F("AdsCloseIndex"))(order);
        }
        printf("]}");
    }
}

int main(int argc, char **argv) {
    if (argc != 3 || !argv[2][0]) return 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) return 1;
    H conn = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
        (U8 *)argv[2], 1, NULL, NULL, 0, &conn);
    if (rc) return printf("connect: %lu\n", rc), 1;
    static const struct { const char *name, *ext; U16 type; U32 options; } types[] = {
        {"ADT", "adi", 3, 2}, {"NTX", "ntx", 1, 0}, {"CDX", "cdx", 2, 2}};
    static const char *kw_chars[] = {"AND", "OR", "TRUE", "FALSE", "NULL"};
    static const char *kw_values[3][5] = {
        {"a", "b", "a", "b", "a"}, {"b", "a", "b", "a", "b"}, {"a", "a", "a", "a", "a"}};
    static const U16 kw_not[3] = {1, 0, 0}, xs[3] = {1, 1, 0};
    static const char *plain_c[3] = {"a", "b", "a"};
    printf("[\n");
    for (int t = 0; t < 3; t++) {
        for (int k = 0; k < 2; k++) {
            char name[16];
            H table = 0;
            snprintf(name, sizeof name, "%s%d", k ? "plain" : "kw", t);
            const char *fields = k ? "C,C,2,0;X,Logical"
                                   : "AND,C,2,0;OR,C,2,0;TRUE,C,2,0;FALSE,C,2,0;NULL,C,2,0;"
                                     "NOT,Logical;X,Logical";
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *))F(
                "AdsCreateTable"))(conn, (U8 *)name, NULL, types[t].type, 1, 0, 1, 0,
                                   (U8 *)fields, &table);
            if (rc) return printf("create %s: %lu\n", name, rc), 1;
            for (int r = 0; r < 3; r++) {
                ((U32(WINAPI *)(H))F("AdsAppendRecord"))(table);
                if (k) {
                    ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetString"))(
                        table, (U8 *)"C", (U8 *)plain_c[r], 1);
                } else {
                    for (int f = 0; f < 5; f++)
                        ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetString"))(
                            table, (U8 *)kw_chars[f], (U8 *)kw_values[r][f], 1);
                    ((U32(WINAPI *)(H, U8 *, U16))F("AdsSetLogical"))(table, (U8 *)"NOT",
                                                                       kw_not[r]);
                }
                ((U32(WINAPI *)(H, U8 *, U16))F("AdsSetLogical"))(table, (U8 *)"X", xs[r]);
                ((U32(WINAPI *)(H))F("AdsWriteRecord"))(table);
            }
            filters(table, types[t].name, k ? "plain" : "kw", k ? plain_filters : kw_filters);
            indexes(table, types[t].name, k ? "plain" : "kw", types[t].ext, types[t].options,
                    k ? plain_indexes : kw_indexes);
            ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
        }
    }
    printf("\n]\n");
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
    return 0;
}
