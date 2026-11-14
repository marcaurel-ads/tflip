/* Records which fields an Advantage Client Engine says can hold NULL, and
 * what AdsSetNull does to them, for fixtures/fields/nullable.json.
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o nullable.exe nullable.c
 *     cd <empty folder> && wine nullable.exe <ace32.dll> <the folder as a Windows path>
 *
 * The original resolves relative table names against the current directory,
 * so the folder must be the current one too. For each table type (ADT, NTX,
 * CDX, VFP) and each field definition below, the probe makes the table
 * n<type>_<n> of the field F so defined and the field X C(2), appends a
 * record, sets F to a value where the list has one, and gives the row
 *
 *     {"type", "def", "create", "nullable", "is_null", "set_null",
 *      "null_after", "empty_after"}
 *
 * of AdsCreateTable, AdsIsNullable of F, AdsIsNull of F before AdsSetNull,
 * AdsSetNull of F, then AdsIsNull and AdsIsEmpty of F (the values are 9 where
 * the call failed or was not made). */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;
static HMODULE ace;
#define F(name) ((void *)GetProcAddress(ace, name))

static const struct { const char *def, *value; } defs[] = {
    {"F,Character,4", "abcd"},
    {"F,Character,4,0", "abcd"},
    {"F,Character,4,0,NULL", "abcd"},
    {"F,Character,4,0,NOT NULL", "abcd"},
    {"F,C,4,0,NULL", "abcd"},
    {"F,Numeric,5,1", "12.5"},
    {"F,Numeric,5,1,NULL", "12.5"},
    {"F,Date", NULL},
    {"F,Date,8,0,NULL", NULL},
    {"F,Logical", NULL},
    {"F,Logical,1,0,NULL", NULL},
    {"F,Memo", "abc"},
    {"F,Memo,10,0,NULL", "abc"},
    {"F,Integer", "7"},
    {"F,Integer,4,0,NULL", "7"},
    {"F,Double", "1.5"},
    {"F,Double,8,2,NULL", "1.5"},
    {"F,Currency", "3"},
    {"F,AutoInc", NULL},
    {"F,VarChar,6", "ab"},
    {"F,VarChar,6,0,NULL", "ab"},
    {"F,Timestamp", NULL},
    {NULL, NULL},
};

int main(int argc, char **argv) {
    if (argc != 3 || !argv[2][0]) return 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) return 1;
    H conn = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
        (U8 *)argv[2], 1, NULL, NULL, 0, &conn);
    if (rc) return printf("connect: %lu\n", rc), 1;
    static const struct { const char *name; U16 type; } types[] = {
        {"ADT", 3}, {"NTX", 1}, {"CDX", 2}, {"VFP", 4}};
    int first = 1;
    printf("[\n");
    for (int t = 0; t < 4; t++) {
        for (int d = 0; defs[d].def; d++) {
            char name[16], fields[64];
            H table = 0;
            snprintf(name, sizeof name, "n%u_%d", types[t].type, d);
            snprintf(fields, sizeof fields, "%s;X,Character,2,0", defs[d].def);
            U32 create = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *))F(
                "AdsCreateTable"))(conn, (U8 *)name, NULL, types[t].type, 1, 0, 1, 0,
                                   (U8 *)fields, &table);
            U16 nullable = 9, is_null = 9, null_after = 9, empty_after = 9;
            U32 set_null = 9;
            if (create == 0) {
                ((U32(WINAPI *)(H))F("AdsAppendRecord"))(table);
                if (defs[d].value)
                    ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetField"))(
                        table, (U8 *)"F", (U8 *)defs[d].value, (U32)strlen(defs[d].value));
                ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsIsNullable"))(table, (U8 *)"F", &nullable);
                ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsIsNull"))(table, (U8 *)"F", &is_null);
                set_null = ((U32(WINAPI *)(H, U8 *))F("AdsSetNull"))(table, (U8 *)"F");
                ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsIsNull"))(table, (U8 *)"F", &null_after);
                ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsIsEmpty"))(table, (U8 *)"F", &empty_after);
                ((U32(WINAPI *)(H))F("AdsWriteRecord"))(table);
                ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
            }
            printf("%s{\"type\": \"%s\", \"def\": \"%s\", \"create\": %lu, \"nullable\": %u, "
                   "\"is_null\": %u, \"set_null\": %lu, \"null_after\": %u, \"empty_after\": %u}",
                   first ? "" : ",\n", types[t].name, defs[d].def, create, nullable, is_null,
                   set_null, null_after, empty_after);
            first = 0;
        }
    }
    printf("\n]\n");
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
    return 0;
}
