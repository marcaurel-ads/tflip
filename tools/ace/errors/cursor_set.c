/* Records what the original engine answers when the AdsSet* setters are
 * called on a SQL cursor, live (one table's columns, and of a statement set
 * read-only) and static, for
 * fixtures/errors/cursor_set.json.
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o cursor_set.exe cursor_set.c
 *     wine cursor_set.exe <ace32.dll> <folder as a Windows path>
 *
 * The folder holds all_types.adt and .adm, and a copy of them in orig\
 * that restores the table before every case. Each case is
 * {"cursor": <kind>, "fn": <setter>, "field": <name>, "rc": <rc>, "code",
 * "text": <AdsGetLastError after it>, "how": <write, move or close>, "write": <rc of
 * AdsWriteRecord (write) or AdsGotoBottom (move)>,
 * "write_code", "write_text", "value": <the field as AdsGetField reads it
 * afterwards, or null>, "table_changed": <whether all_types.adt or .adm
 * differ afterwards>}. */
#include <windows.h>
#include <stdio.h>
#include <string.h>
#include <stdlib.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;
static HMODULE ace;
#define F(name) ((void *)GetProcAddress(ace, name))
#define CALL(ret, args, name) ((ret(WINAPI *) args) F(name))

static const struct {
    const char *name, *sql;
} cursors[] = {
    {"live_star", "SELECT * FROM all_types"},
    {"live_read_only", "SELECT * FROM all_types"},
    {"live_columns", "SELECT c_char, c_integer, c_double, c_logical, c_date, c_blob, c_money, "
                     "c_timestamp FROM all_types"},
    {"static_join", "SELECT a.c_char, a.c_integer, a.c_double, a.c_logical, a.c_date, a.c_blob, "
                    "a.c_money, a.c_timestamp FROM all_types a JOIN all_types b ON "
                    "a.c_integer = b.c_integer"},
    {"static_group", "SELECT c_char, c_integer, COUNT(*) AS n FROM all_types GROUP BY c_char, "
                     "c_integer"},
};

enum { K_STRING, K_STRINGW, K_FIELD, K_LONG, K_SHORT, K_DOUBLE, K_LOGICAL, K_DATE, K_MONEY, K_NUL, K_EMPTY, K_BINARY,
       K_JULIAN, K_TIMESTAMP, K_BIGINT, K_LONGTEXT, K_BADNUMBER };
static const struct {
    int kind;
    const char *fn, *field;
} sets[] = {
    {K_STRING, "AdsSetString", "c_char"},      {K_STRINGW, "AdsSetStringW", "c_char"},
    {K_FIELD, "AdsSetField", "c_char"},        {K_LONG, "AdsSetLong", "c_integer"},
    {K_SHORT, "AdsSetShort", "c_integer"},     {K_DOUBLE, "AdsSetDouble", "c_double"},
    {K_LOGICAL, "AdsSetLogical", "c_logical"}, {K_DATE, "AdsSetDate", "c_date"},
    {K_MONEY, "AdsSetMoney", "c_money"},       {K_NUL, "AdsSetNull", "c_integer"},
    {K_EMPTY, "AdsSetEmpty", "c_char"},        {K_BINARY, "AdsSetBinary", "c_blob"},
    {K_JULIAN, "AdsSetJulian", "c_date"},      {K_TIMESTAMP, "AdsSetTimeStamp", "c_timestamp"},
    {K_BIGINT, "AdsSetLongLong", "c_integer"}, {K_STRING, "AdsSetString", "nosuch"},
    {K_STRING, "AdsSetString", "#99"}, {K_STRING, "AdsSetString", "#0"},
    {K_STRING, "AdsSetString", "n"},
    /* A text longer than its field, and one that is no number. */
    {K_LONGTEXT, "AdsSetString long", "c_char"}, {K_BADNUMBER, "AdsSetString bad", "c_integer"},
};

static void json(const char *s) {
    putchar('"');
    for (; *s; s++) {
        if (*s == '"' || *s == '\\') printf("\\%c", *s);
        else if ((U8)*s < 0x20 || (U8)*s > 0x7e) printf("\\u%04x", (U8)*s);
        else putchar(*s);
    }
    putchar('"');
}

static char *slurp(const char *path, long *len) {
    FILE *f = fopen(path, "rb");
    if (!f) return *len = 0, NULL;
    fseek(f, 0, SEEK_END);
    *len = ftell(f);
    rewind(f);
    char *buf = malloc(*len + 1);
    fread(buf, 1, *len, f);
    fclose(f);
    return buf;
}

static int differs(const char *folder, const char *ext) {
    char a[600], b[600];
    snprintf(a, sizeof a, "%s\\all_types.%s", folder, ext);
    snprintf(b, sizeof b, "%s\\orig\\all_types.%s", folder, ext);
    long la, lb;
    char *x = slurp(a, &la), *y = slurp(b, &lb);
    int d = la != lb || (la && memcmp(x, y, la));
    free(x);
    free(y);
    return d;
}

static void last(U32 *code, U8 *text, U16 len) {
    U16 n = len;
    text[0] = 0;
    CALL(U32, (U32 *, U8 *, U16 *), "AdsGetLastError")(code, text, &n);
}

int main(int argc, char **argv) {
    if (argc < 3) return fprintf(stderr, "usage: cursor_set.exe <ace32.dll> <folder>\n"), 2;
    setvbuf(stdout, NULL, _IONBF, 0);
    ace = LoadLibraryA(argv[1]);
    if (!ace) return fprintf(stderr, "cannot load %s\n", argv[1]), 1;
    printf("[\n");
    int first = 1;
    for (size_t c = 0; c < sizeof cursors / sizeof *cursors; c++) {
        for (size_t s = 0; s < sizeof sets / sizeof *sets; s++)
        for (int how = 0; how < 3; how++) {
            char from[600], to[600];
            const char *ext[] = {"adt", "adm"};
            for (int e = 0; e < 2; e++) {
                snprintf(from, sizeof from, "%s\\orig\\all_types.%s", argv[2], ext[e]);
                snprintf(to, sizeof to, "%s\\all_types.%s", argv[2], ext[e]);
                CopyFileA(from, to, FALSE);
            }
            H conn = 0, stmt = 0, cur = 0;
            U32 rc = CALL(U32, (U8 *, U16, U8 *, U8 *, U32, H *), "AdsConnect60")(
                (U8 *)argv[2], 1, NULL, NULL, 0, &conn);
            if (rc) return fprintf(stderr, "connect: %lu\n", rc), 1;
            CALL(U32, (H, H *), "AdsCreateSQLStatement")(conn, &stmt);
            if (!strcmp(cursors[c].name, "live_read_only"))
                CALL(U32, (H, U16), "AdsStmtSetTableReadOnly")(stmt, 1);
            rc = CALL(U32, (H, U8 *, H *), "AdsExecuteSQLDirect")(stmt, (U8 *)cursors[c].sql,
                                                                  &cur);
            if (rc || !cur) return fprintf(stderr, "%s: %lu\n", cursors[c].name, rc), 1;
            CALL(U32, (H), "AdsGotoTop")(cur);
            /* "#n" is the field number n, given instead of a name. */
            U8 *f = sets[s].field[0] == '#' ? (U8 *)(ULONG_PTR)atoi(sets[s].field + 1)
                                            : (U8 *)sets[s].field;
            switch (sets[s].kind) {
            case K_STRING:
                rc = CALL(U32, (H, U8 *, U8 *, U32), "AdsSetString")(cur, f, (U8 *)"changed", 7);
                break;
            case K_LONGTEXT:
                rc = CALL(U32, (H, U8 *, U8 *, U32), "AdsSetString")(
                    cur, f, (U8 *)"a text longer than the field", 28);
                break;
            case K_BADNUMBER:
                rc = CALL(U32, (H, U8 *, U8 *, U32), "AdsSetString")(cur, f, (U8 *)"abc", 3);
                break;
            case K_STRINGW: {
                WCHAR w[] = L"changed";
                rc = CALL(U32, (H, U8 *, WCHAR *, U32), "AdsSetStringW")(cur, f, w, 7);
                break;
            }
            case K_FIELD:
                rc = CALL(U32, (H, U8 *, U8 *, U32), "AdsSetField")(cur, f, (U8 *)"changed", 7);
                break;
            case K_LONG: rc = CALL(U32, (H, U8 *, int), "AdsSetLong")(cur, f, 4242); break;
            case K_SHORT: rc = CALL(U32, (H, U8 *, short), "AdsSetShort")(cur, f, 42); break;
            case K_DOUBLE: {
                double d = 42.5;
                rc = CALL(U32, (H, U8 *, double), "AdsSetDouble")(cur, f, d);
                break;
            }
            case K_LOGICAL: rc = CALL(U32, (H, U8 *, U16), "AdsSetLogical")(cur, f, 1); break;
            case K_DATE:
                rc = CALL(U32, (H, U8 *, U8 *, U16), "AdsSetDate")(cur, f, (U8 *)"01/02/2020", 10);
                break;
            case K_MONEY: {
                __int64 m = 12345;
                rc = CALL(U32, (H, U8 *, __int64), "AdsSetMoney")(cur, f, m);
                break;
            }
            case K_NUL: rc = CALL(U32, (H, U8 *), "AdsSetNull")(cur, f); break;
            case K_EMPTY: rc = CALL(U32, (H, U8 *), "AdsSetEmpty")(cur, f); break;
            case K_BINARY:
                rc = CALL(U32, (H, U8 *, U16, U32, U32, U8 *, U32), "AdsSetBinary")(
                    cur, f, 6, 3, 0, (U8 *)"abc", 3);
                break;
            case K_JULIAN: rc = CALL(U32, (H, U8 *, int), "AdsSetJulian")(cur, f, 2458850); break;
            case K_TIMESTAMP:
                rc = CALL(U32, (H, U8 *, U8 *, U32), "AdsSetTimeStamp")(
                    cur, f, (U8 *)"01/02/2020 03:04:05.250", 23);
                break;
            case K_BIGINT: {
                __int64 n = 4242;
                rc = CALL(U32, (H, U8 *, __int64), "AdsSetLongLong")(cur, f, n);
                break;
            }
            }
            U32 code = 0, wcode = 0;
            U8 text[600], wtext[600];
            last(&code, text, sizeof text);
            /* How the change is kept: AdsWriteRecord, a move to the last
             * record, or nothing but closing the cursor. */
            U32 wrc = 0;
            if (how == 0) wrc = CALL(U32, (H), "AdsWriteRecord")(cur);
            if (how == 1) wrc = CALL(U32, (H), "AdsGotoBottom")(cur);
            last(&wcode, wtext, sizeof wtext);
            if (how == 1) CALL(U32, (H), "AdsGotoTop")(cur);
            U8 value[600];
            U32 vlen = sizeof value;
            U32 vrc = CALL(U32, (H, U8 *, U8 *, U32 *, U16), "AdsGetField")(cur, f, value, &vlen, 0);
            value[vlen < sizeof value ? vlen : sizeof value - 1] = 0;
            CALL(U32, (H), "AdsCloseSQLStatement")(stmt);
            CALL(U32, (H), "AdsDisconnect")(conn);
            printf("%s{\"cursor\": \"%s\", \"fn\": \"%s\", \"field\": \"%s\", \"how\": \"%s\", \"rc\": %lu, "
                   "\"code\": %lu, \"text\": ",
                   first ? "" : ",\n", cursors[c].name, sets[s].fn, sets[s].field,
                   how == 0 ? "write" : how == 1 ? "move" : "close", rc, code);
            json((char *)text);
            printf(", \"write\": %lu, \"write_code\": %lu, \"write_text\": ", wrc, wcode);
            json((char *)wtext);
            printf(", \"value\": ");
            if (vrc) printf("null");
            else json((char *)value);
            printf(", \"table_changed\": %s}",
                   differs(argv[2], "adt") || differs(argv[2], "adm") ? "true" : "false");
            first = 0;
        }
    }
    printf("\n]\n");
    return 0;
}
