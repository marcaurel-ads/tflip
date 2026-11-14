/* Records what an Advantage Client Engine stores of a NUL byte in the text
 * AdsSetString and AdsSetField are given, for fixtures/fields/nul_strings.json.
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o nul_strings.exe nul_strings.c
 *     cd <empty folder> && wine nul_strings.exe <ace32.dll> <the folder as a Windows path>
 *
 * The original resolves relative table names against the current directory,
 * so the folder must be the current one too. For each table type (NTX, CDX,
 * ADT, VFP) the probe makes the table n<type> of the fields C C(8) and M
 * Memo (and, on ADT, V VarChar(8) and Z CIChar(8)), and for each text below
 * and each setter appends a record, sets each field to the text, writes it
 * and reads each back with AdsGetField (its length and bytes) and seeks it
 * through an order on the field (on C, V, Z; AdsSeek hard of the text's
 * bytes, ADS_STRINGKEY):
 *
 *     {"type", "setter", "text", "field", "rc", "got", "found"}
 *
 * "text" and "got" naming a NUL "\0" ("got" null where the read failed,
 * "found" null where the field has no order). The texts:
 *
 *   ab\0cd     5 bytes
 *   abcd\0     5 bytes
 *   \0abc      4 bytes
 *   ab\0\0\0\0\0\0  8 bytes */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;
static HMODULE ace;
#define F(name) ((void *)GetProcAddress(ace, name))

static void bytes(const U8 *text, U32 len) {
    putchar('"');
    for (U32 i = 0; i < len; i++) printf(text[i] ? "%c" : "\\\\0", text[i]);
    putchar('"');
}

int main(int argc, char **argv) {
    if (argc != 3 || !argv[2][0]) return 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) return 1;
    H conn = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
        (U8 *)argv[2], 1, NULL, NULL, 0, &conn);
    if (rc) return printf("connect: %lu\n", rc), 1;
    static const struct { const char *name, *ext, *fields; U16 type; U32 options; } types[] = {
        {"NTX", "ntx", "C,C,8,0;M,M,10,0", 1, 0},
        {"CDX", "cdx", "C,C,8,0;M,M,10,0", 2, 2},
        {"ADT", "adi", "C,Char,8;M,Memo;V,VarChar,8;Z,CIChar,8", 3, 2},
        {"VFP", "cdx", "C,C,8,0;M,M,10,0", 4, 2}};
    static const struct { const char *bytes; U32 len; } texts[] = {
        {"ab\0cd", 5}, {"abcd\0", 5}, {"\0abc", 4}, {"ab\0\0\0\0\0\0", 8}};
    static const char *fields[] = {"C", "M", "V", "Z"};
    int first = 1;
    printf("[\n");
    for (int t = 0; t < 4; t++) {
        char name[16];
        H table = 0;
        H orders[4] = {0};
        int nfields = types[t].type == 3 ? 4 : 2;
        snprintf(name, sizeof name, "n%u", types[t].type);
        rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *))F(
            "AdsCreateTable"))(conn, (U8 *)name, NULL, types[t].type, 1, 0, 1, 0,
                               (U8 *)types[t].fields, &table);
        if (rc) return printf("create %s: %lu\n", name, rc), 1;
        for (int f = 0; f < nfields; f++) {
            char bag[24];
            if (f == 1) continue;
            snprintf(bag, sizeof bag, "n%u%s.%s", types[t].type, types[t].type == 1 ? fields[f] : "",
                     types[t].ext);
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, U32, H *))F(
                "AdsCreateIndex61"))(table, (U8 *)bag, (U8 *)fields[f], (U8 *)fields[f], (U8 *)"",
                                     (U8 *)"", types[t].options, 512, &orders[f]);
            if (rc) return printf("index %s: %lu\n", fields[f], rc), 1;
        }
        for (int s = 0; s < 2; s++) {
            for (int x = 0; x < 4; x++) {
                U32 set[4] = {0};
                ((U32(WINAPI *)(H))F("AdsAppendRecord"))(table);
                for (int f = 0; f < nfields; f++)
                    set[f] = ((U32(WINAPI *)(H, U8 *, U8 *, U32))F(s ? "AdsSetField"
                                                                     : "AdsSetString"))(
                        table, (U8 *)fields[f], (U8 *)texts[x].bytes, texts[x].len);
                ((U32(WINAPI *)(H))F("AdsWriteRecord"))(table);
                U32 record = 0;
                ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordNum"))(table, 2, &record);
                for (int f = 0; f < nfields; f++) {
                    U8 got[64];
                    U32 len = sizeof got;
                    ((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(table, record);
                    U32 read = ((U32(WINAPI *)(H, U8 *, U8 *, U32 *, U16))F("AdsGetField"))(
                        table, (U8 *)fields[f], got, &len, 0);
                    printf("%s{\"type\": \"%s\", \"setter\": \"%s\", \"text\": ",
                           first ? "" : ",\n", types[t].name, s ? "field" : "string");
                    first = 0;
                    bytes((const U8 *)texts[x].bytes, texts[x].len);
                    printf(", \"field\": \"%s\", \"rc\": %lu, \"got\": ", fields[f], set[f]);
                    if (read) printf("null");
                    else bytes(got, len);
                    printf(", \"found\": ");
                    if (!orders[f]) {
                        printf("null}");
                        continue;
                    }
                    U16 found = 9;
                    U32 at = 0;
                    ((U32(WINAPI *)(H, U8 *, U16, U16, U16, U16 *))F("AdsSeek"))(
                        orders[f], (U8 *)texts[x].bytes, (U16)texts[x].len, 2, 2, &found);
                    ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordNum"))(table, 2, &at);
                    printf("%s}", found && at == record ? "true" : "false");
                }
            }
        }
        ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
    }
    printf("\n]\n");
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
    return 0;
}
