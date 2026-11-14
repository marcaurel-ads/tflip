/* Records the last error AdsGotoTop and AdsCloseIndex leave when given
 * handle 0 or a value that is no handle (0x12345), after each kind of
 * error a call before left, for fixtures/errors/moves_after_errors.json.
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o moves_after_errors.exe moves_after_errors.c
 *     cd <empty folder> && wine moves_after_errors.exe <ace32.dll> <the folder as a Windows path>
 *
 * The probe makes the ADT table z (K C(4), no records). For each way of
 * leaving an error first ("before": none, after a call that succeeded; 5013,
 * AdsGetRecordCount with option 0; 5018 not readable and 5018 must be,
 * AdsGotoTop of 0 and of the connection; 5023, AdsWriteRecord of the
 * connection; 5024, AdsRollbackTransaction of the table; 5106,
 * AdsCloseSQLStatement of the table) and each call and handle, a row
 *
 *     {"before", "before_last", "call", "handle": "zero"|"none", "rc",
 *      "last", "text"}
 *
 * gives the error the first call left (its code), what the second call
 * returns, and the code and text AdsGetLastError gives after it. */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;
static HMODULE ace;
#define F(name) ((void *)GetProcAddress(ace, name))

static H conn, table;
static int first = 1;

static U32 last(U8 *text) {
    U32 code = 0;
    U16 len = 600;
    text[0] = 0;
    ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
    return code;
}

static void leave(const char *before) {
    U32 n;
    ((U32(WINAPI *)(H))F("AdsGotoTop"))(table);
    if (!strcmp(before, "5013")) ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(table, 0, &n);
    if (!strcmp(before, "5018 zero")) ((U32(WINAPI *)(H))F("AdsGotoTop"))(0);
    if (!strcmp(before, "5018 conn")) ((U32(WINAPI *)(H))F("AdsGotoTop"))(conn);
    if (!strcmp(before, "5023")) ((U32(WINAPI *)(H))F("AdsWriteRecord"))(conn);
    if (!strcmp(before, "5024")) ((U32(WINAPI *)(H))F("AdsRollbackTransaction"))(table);
    if (!strcmp(before, "5106")) ((U32(WINAPI *)(H))F("AdsCloseSQLStatement"))(table);
}

int main(int argc, char **argv) {
    if (argc != 3 || !argv[2][0]) return 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) return 1;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
        (U8 *)argv[2], 1, NULL, NULL, 0, &conn);
    if (rc) return printf("connect: %lu\n", rc), 1;
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *))F("AdsCreateTable"))(
        conn, (U8 *)"z", NULL, 3, 1, 0, 1, 0, (U8 *)"K,C,4,0", &table);
    if (rc) return printf("create: %lu\n", rc), 1;
    static const char *befores[] = {"none", "5013", "5018 zero", "5018 conn", "5023", "5024", "5106"};
    static const char *calls[] = {"AdsGotoTop", "AdsCloseIndex"};
    static const struct { const char *name; H value; } handles[] = {{"zero", 0}, {"none", 0x12345}};
    printf("[\n");
    for (int b = 0; b < 7; b++)
        for (int c = 0; c < 2; c++)
            for (int h = 0; h < 2; h++) {
                U8 text[600];
                leave(befores[b]);
                U32 before_last = last(text);
                rc = ((U32(WINAPI *)(H))F(calls[c]))(handles[h].value);
                U32 code = last(text);
                printf("%s{\"before\": \"%s\", \"before_last\": %lu, \"call\": \"%s\", \"handle\": "
                       "\"%s\", \"rc\": %lu, \"last\": %lu, \"text\": \"%s\"}",
                       first ? "" : ",\n", befores[b], before_last, calls[c], handles[h].name, rc,
                       code, text);
                first = 0;
            }
    printf("\n]\n");
    ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
    return 0;
}
