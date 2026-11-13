/* Records the last error the original engine leaves after calls given wrong
 * handles, one after another, for fixtures/errors/handle_order.json: what
 * a call records depends on the error left by the call before it.
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o handle_order.exe handle_order.c
 *     wine handle_order.exe <ace32.dll> <folder with fixtures/write/goto_deleted/before as a Windows path>
 *
 * Each step of `steps` (a call taking one handle, and the handle given:
 * conn, table, order, stmt, none for 0x12345, zero, or closed for a table
 * handle closed before the steps) is {"step": "<fn> <handle>", "rc": <rc>,
 * "last": <AdsGetLastError's code>, "text": <its text>}. */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;
static HMODULE ace;
#define F(name) ((void *)GetProcAddress(ace, name))

static const char *steps[] = {
    "AdsWriteRecord conn",         "AdsWriteRecord none",
    "AdsGotoTop table",            "AdsWriteRecord none",
    "AdsWriteRecord conn",         "AdsWriteRecord order",
    "AdsGotoTop table",            "AdsWriteRecord order",
    "AdsWriteRecord stmt",         "AdsGotoTop conn",
    "AdsWriteRecord conn",         "AdsGotoTop table",
    "AdsGotoTop stmt",             "AdsGotoTop none",
    "AdsGotoTop zero",             "AdsCloseSQLStatement conn",
    "AdsCloseSQLStatement none",   "AdsCloseSQLStatement table",
    "AdsGotoTop table",            "AdsCloseSQLStatement none",
    "AdsWriteRecord conn",         "AdsCloseTable closed",
    "AdsWriteRecord closed",       "AdsGotoTop closed",
    "AdsGotoTop table",            "AdsCloseTable closed",
    "AdsWriteRecord conn",         "AdsGotoTop table",
    "AdsClearAOF order",           "AdsClearAOF conn",
    "AdsGotoTop table",            "AdsRollbackTransaction none",
    "AdsRollbackTransaction table", "AdsGotoTop table",
    "AdsRollbackTransaction table", "AdsWriteRecord zero",
    "AdsReindex conn",             "AdsGotoTop table",
    "AdsReindex conn",             "AdsClearSQLParams order",
    "AdsGotoTop table",            "AdsClearSQLParams none",
    "AdsGotoTop table",            "AdsCloseTable order",
    "AdsLockTable stmt",           "AdsGotoTop table",
    "AdsLockTable none",
};

static void json(const U8 *s, size_t n) {
    putchar('"');
    for (size_t i = 0; i < n; i++) {
        U8 c = s[i];
        if (c == '"' || c == '\\') printf("\\%c", c);
        else if (c < 32 || c >= 127) printf("\\u%04x", c);
        else putchar(c);
    }
    putchar('"');
}

int main(int argc, char **argv) {
    if (argc < 3) return fprintf(stderr, "usage: handle_order.exe <ace32.dll> <folder>\n"), 2;
    setvbuf(stdout, NULL, _IONBF, 0);
    ace = LoadLibraryA(argv[1]);
    if (!ace) return fprintf(stderr, "cannot load %s\n", argv[1]), 1;
    H conn = 0, table = 0, closed = 0, order = 0, stmt = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
        (U8 *)argv[2], 1, NULL, NULL, 0, &conn);
    if (rc) return fprintf(stderr, "connect: %lu\n", rc), 1;
    char path[600];
    snprintf(path, sizeof path, "%s\\p.adt", argv[2]);
    typedef U32(WINAPI * Open)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *);
    rc = ((Open)F("AdsOpenTable"))(conn, (U8 *)path, NULL, 3, 1, 1, 1, 2, &table);
    if (rc) return fprintf(stderr, "open: %lu\n", rc), 1;
    rc = ((Open)F("AdsOpenTable"))(conn, (U8 *)path, NULL, 3, 1, 1, 1, 2, &closed);
    if (rc) return fprintf(stderr, "open: %lu\n", rc), 1;
    ((U32(WINAPI *)(H))F("AdsCloseTable"))(closed);
    U16 n = 1;
    ((U32(WINAPI *)(H, H *, U16 *))F("AdsGetAllIndexes"))(table, &order, &n);
    ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(conn, &stmt);
    printf("[\n");
    for (size_t i = 0; i < sizeof steps / sizeof *steps; i++) {
        char fn[64], which[16];
        sscanf(steps[i], "%63s %15s", fn, which);
        H handle = !strcmp(which, "conn")     ? conn
                   : !strcmp(which, "table")  ? table
                   : !strcmp(which, "order")  ? order
                   : !strcmp(which, "stmt")   ? stmt
                   : !strcmp(which, "none")   ? 0x12345
                   : !strcmp(which, "closed") ? closed
                                              : 0;
        U32 rc = ((U32(WINAPI *)(H))F(fn))(handle);
        U32 code = 0;
        U8 text[600];
        U16 len = sizeof text;
        ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
        printf("%s{\"step\": \"%s\", \"rc\": %lu, \"last\": %lu, \"text\": ", i ? ",\n" : "",
               steps[i], rc, code);
        json(text, code ? len : 0);
        printf("}");
    }
    printf("\n]\n");
    return 0;
}
