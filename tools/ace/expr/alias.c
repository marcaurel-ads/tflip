/* Records which qualified names (`qualifier->field`) an Advantage Client
 * Engine accepts in index keys, filters, AOFs and evaluated expressions, for
 * fixtures/expr/alias.json.
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o alias.exe alias.c
 *     cd <empty folder> && wine alias.exe <ace32.dll> <the folder as a Windows path>
 *
 * The original resolves relative table names against the current directory,
 * so the folder must be the current one too. For each table type (ADT, NTX,
 * CDX, VFP) the probe makes the table `acct` with the fields ID N(6) and
 * C C(4) and the records (1001, 'a'), (1002, 'b'), (1003, 'c'), and opens it
 * once with its own alias (acct) and once with the alias XX. With the record
 * pointer on record 2, each expression E in the list below gives the rows
 *
 *     {"type", "alias", "expr", "op": "index", "rc", "error", "key_expr",
 *      "key_length", "key_type", "seek_rc", "found", "at"}
 *                  AdsCreateIndex61 on E in a bag of its own; then the
 *                  expression AdsGetIndexExpr gives back, the key's length
 *                  and type, and AdsSeek of the double 1003 (ADS_DOUBLEKEY,
 *                  hard) with whether it was found and the record it is on
 *     {..., "op": "valid", "rc", "error", "valid"}        AdsIsExprValid
 *     {..., "op": "eval", "rc", "error", "value"}         AdsEvalNumericExpr
 *     {..., "op": "filter", "rc", "error", "count"}       AdsSetFilter on
 *                  "(E) > 1001", then AdsGetRecordCount(ADS_RESPECTFILTERS)
 *     {..., "op": "aof", "rc", "error", "count"}          the same by
 *                  AdsSetAOF(ADS_RESOLVE_DYNAMIC)
 *
 * "error" is the text AdsGetLastError gives after a call that fails, and
 * "count" is 777 where the call leaves it untouched. */
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
    "ID",
    "FIELD->ID",
    "field->id",
    "Field->Id",
    "FIELD -> ID",
    "FIELD ->ID",
    "FIELD-> ID",
    "ACCT -> ID",
    "ACCT ->ID",
    "ACCT-> ID",
    "_FIELD->ID",
    "FIELDS->ID",
    "FIEL->ID",
    "ACCT->ID",
    "acct->id",
    "XX->ID",
    "OTHER->ID",
    "M->ID",
    "MEMVAR->ID",
    "FIELD->NOPE",
    "FIELD->FIELD->ID",
    "ACCT->FIELD->ID",
    "FIELD->ACCT->ID",
    "FIELD->ID + 1",
    "ID + 1",
    "STR(FIELD->ID)",
    "FIELD->C",
    "UPPER(FIELD->C)",
    "FIELD->",
    "->ID",
};
#define NEXPRS ((int)(sizeof exprs / sizeof *exprs))

static int first = 1;
static const char *type_name, *alias_name;

static void json_str(const char *s) {
    putchar('"');
    for (; *s; s++) {
        if (*s == '"' || *s == '\\') putchar('\\');
        if ((U8)*s < 0x20) printf("\\u%04x", (U8)*s);
        else putchar(*s);
    }
    putchar('"');
}

static void head(const char *expr, const char *op, U32 rc) {
    printf("%s{\"type\": \"%s\", \"alias\": \"%s\", \"expr\": ", first ? "" : ",\n", type_name,
           alias_name);
    json_str(expr);
    printf(", \"op\": \"%s\", \"rc\": %lu", op, rc);
    first = 0;
    if (rc) {
        U32 code = 0;
        U8 text[600] = "";
        U16 len = sizeof text;
        ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
        printf(", \"error\": ");
        json_str((const char *)text);
    }
}

static void at(H table) {
    U16 eof = 0;
    U32 recno = 0;
    ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(table, &eof);
    ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordNum"))(table, 2, &recno);
    if (eof) printf(", \"at\": \"E\"");
    else printf(", \"at\": %lu", recno);
}

static void probe(H table, int t, int a, int e, const char *ext) {
    const char *expr = exprs[e];
    char bag[32], cond[96];
    H order = 0;
    snprintf(bag, sizeof bag, "b%d%d_%d.%s", t, a, e, ext);
    ((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(table, 2);
    U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, U32, H *))F(
        "AdsCreateIndex61"))(table, (U8 *)bag, (U8 *)"T", (U8 *)expr, (U8 *)"", (U8 *)"", 0,
                             512, &order);
    head(expr, "index", rc);
    if (!rc) {
        U8 back[300] = "";
        U16 len = sizeof back, key_length = 0, key_type = 0, found = 7;
        ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsGetIndexExpr"))(order, back, &len);
        ((U32(WINAPI *)(H, U16 *))F("AdsGetKeyLength"))(order, &key_length);
        ((U32(WINAPI *)(H, U16 *))F("AdsGetKeyType"))(order, &key_type);
        printf(", \"key_expr\": ");
        json_str((const char *)back);
        double key = 1003.0;
        U32 seek_rc = ((U32(WINAPI *)(H, U8 *, U16, U16, U16, U16 *))F("AdsSeek"))(
            order, (U8 *)&key, sizeof key, 4, 2, &found);
        printf(", \"key_length\": %u, \"key_type\": %u, \"seek_rc\": %lu, \"found\": %u",
               key_length, key_type, seek_rc, found);
        at(table);
        ((U32(WINAPI *)(H))F("AdsCloseIndex"))(order);
    }
    printf("}");

    U16 valid = 7;
    ((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(table, 2);
    rc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsIsExprValid"))(table, (U8 *)expr, &valid);
    head(expr, "valid", rc);
    printf(", \"valid\": %u}", valid);

    double value = 777;
    rc = ((U32(WINAPI *)(H, U8 *, double *))F("AdsEvalNumericExpr"))(table, (U8 *)expr, &value);
    head(expr, "eval", rc);
    printf(", \"value\": %g}", value);

    snprintf(cond, sizeof cond, "(%s) > 1001", expr);
    U32 count = 777;
    rc = ((U32(WINAPI *)(H, U8 *))F("AdsSetFilter"))(table, (U8 *)cond);
    head(expr, "filter", rc);
    if (!rc) ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(table, 1, &count);
    printf(", \"count\": %lu}", count);
    ((U32(WINAPI *)(H))F("AdsClearFilter"))(table);

    count = 777;
    rc = ((U32(WINAPI *)(H, U8 *, U16))F("AdsSetAOF"))(table, (U8 *)cond, 2);
    head(expr, "aof", rc);
    if (!rc) ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(table, 1, &count);
    printf(", \"count\": %lu}", count);
    ((U32(WINAPI *)(H))F("AdsClearAOF"))(table);
}

int main(int argc, char **argv) {
    if (argc != 3 || !argv[2][0]) return 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) return 1;
    H conn = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
        (U8 *)argv[2], 1, NULL, NULL, 0, &conn);
    if (rc) return printf("connect: %lu\n", rc), 1;
    static const struct { const char *name, *dir, *ext; U16 type; } types[] = {
        {"ADT", "adt", "adi", 3},
        {"NTX", "ntx", "ntx", 1},
        {"CDX", "cdx", "cdx", 2},
        {"VFP", "vfp", "cdx", 4},
    };
    static const char *cs[] = {"a", "b", "c"};
    printf("[\n");
    for (int t = 0; t < 4; t++) {
        char name[32];
        H table = 0;
        type_name = types[t].name;
        CreateDirectoryA(types[t].dir, NULL);
        snprintf(name, sizeof name, "%s\\acct", types[t].dir);
        rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *))F(
            "AdsCreateTable"))(conn, (U8 *)name, NULL, types[t].type, 1, 1, 1, 0,
                               (U8 *)"ID,N,6,0;C,C,4,0", &table);
        if (rc) return printf("create %s: %lu\n", name, rc), 1;
        for (int i = 0; i < 3; i++) {
            ((U32(WINAPI *)(H))F("AdsAppendRecord"))(table);
            ((U32(WINAPI *)(H, U8 *, double))F("AdsSetDouble"))(table, (U8 *)"ID", 1001 + i);
            ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetString"))(table, (U8 *)"C",
                                                                    (U8 *)cs[i], 1);
            ((U32(WINAPI *)(H))F("AdsWriteRecord"))(table);
        }
        ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
        static const char *aliases[] = {NULL, "XX"};
        for (int a = 0; a < 2; a++) {
            alias_name = aliases[a] ? aliases[a] : "acct";
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F(
                "AdsOpenTable"))(conn, (U8 *)name, (U8 *)aliases[a], types[t].type, 1, 1, 1, 1,
                                 &table);
            if (rc) return printf("open %s: %lu\n", name, rc), 1;
            for (int e = 0; e < NEXPRS; e++) probe(table, t, a, e, types[t].ext);
            ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
        }
    }
    printf("\n]\n");
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
    return 0;
}
