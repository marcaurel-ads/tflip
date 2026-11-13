/* Records what the original engine answers when AdsGetRecordCount is given
 * handles that are no table, for fixtures/errors/record_count.json.
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o record_count.exe record_count.c
 *     wine record_count.exe <ace32.dll> <folder with keys.adt as a Windows path>
 *
 * Each row is {"handle": <kind given>, "option": <filter option>, "null":
 * <whether the count pointer was NULL>, "rc": <rc>, "count": <the count,
 * which starts as 777>, "last": <AdsGetLastError's code>, "text": <its
 * text>}. A record number that is not there is provoked just before each
 * call (5018 is not what it gives), so a call that leaves the last error
 * shows that. */
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
static H table;

static void row(const char *name, H handle, U16 option, int null) {
    U32 count = 777, code = 0;
    U8 text[600] = "";
    U16 len = sizeof text;
    ((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(table, 999999);
    U32 rc = ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(handle, option,
                                                                     null ? NULL : &count);
    ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
    printf("%s{\"handle\": \"%s\", \"option\": %u, \"null\": %d, \"rc\": %lu, "
           "\"count\": %lu, \"last\": %lu, \"text\": \"",
           first ? "" : ",\n", name, option, null, rc, count, code);
    for (U8 *p = text; *p; p++) {
        if (*p == '"' || *p == '\\') putchar('\\');
        putchar(*p);
    }
    printf("\"}");
    first = 0;
}

int main(int argc, char **argv) {
    if (argc < 3) return fprintf(stderr, "usage: record_count.exe <ace32.dll> <folder>\n"), 2;
    setvbuf(stdout, NULL, _IONBF, 0);
    ace = LoadLibraryA(argv[1]);
    if (!ace) return fprintf(stderr, "cannot load %s\n", argv[1]), 1;
    H conn = 0, order = 0, stmt = 0, stmt2 = 0, stmt3 = 0, cursor = 0, closed = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
        (U8 *)argv[2], 1, NULL, NULL, 0, &conn);
    if (rc) return fprintf(stderr, "connect: %lu\n", rc), 1;
    char path[600];
    snprintf(path, sizeof path, "%s\\keys.adt", argv[2]);
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
        conn, (U8 *)path, NULL, 3, 1, 1, 1, 2, &table);
    if (rc) return fprintf(stderr, "open: %lu\n", rc), 1;
    ((U32(WINAPI *)(H, U16, H *))F("AdsGetIndexHandleByOrder"))(table, 1, &order);
    /* stmt is fresh, stmt2 has run a SELECT. */
    ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(conn, &stmt);
    ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(conn, &stmt2);
    ((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(
        stmt2, (U8 *)"SELECT * FROM keys", &cursor);
    ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(conn, &stmt3);
    /* Closed last, with nothing created after, so no new handle can take
     * the place of the ones closed. */
    ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
        conn, (U8 *)path, NULL, 3, 1, 1, 1, 2, &closed);
    ((U32(WINAPI *)(H))F("AdsCloseTable"))(closed);
    ((U32(WINAPI *)(H))F("AdsCloseSQLStatement"))(stmt3);
    const struct {
        const char *name;
        H h;
    } hs[] = {
        {"none", 0x12345}, {"zero", 0},          {"connection", conn},
        {"statement", stmt}, {"statement_after_select", stmt2}, {"order", order},
        {"closed", closed}, {"closed_statement", stmt3}, {"table", table},    {"cursor", cursor},
    };
    printf("[\n");
    for (size_t i = 0; i < sizeof hs / sizeof *hs; i++) {
        row(hs[i].name, hs[i].h, 1, 0);
        row(hs[i].name, hs[i].h, 0, 0);
        row(hs[i].name, hs[i].h, 1, 1);
        row(hs[i].name, hs[i].h, 0, 1);
    }
    printf("\n]\n");
    return 0;
}
