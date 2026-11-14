/* Records which values after a field's type an Advantage Client Engine takes
 * in AdsCreateTable's field definitions, for
 * fixtures/errors/field_attributes.json.
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o field_attributes.exe field_attributes.c
 *     cd <empty folder> && wine field_attributes.exe <ace32.dll> <the folder as a Windows path>
 *
 * The original resolves relative table names against the current directory,
 * so the folder must be the current one too. For each table type (NTX, CDX,
 * ADT, VFP) and each definition below, the probe makes the table
 * a<type>_<n> of that one field and gives the row
 *
 *     {"type", "def", "rc", "error"}
 *
 * "error" being AdsGetLastError's text after a failed AdsCreateTable. */
#include <windows.h>
#include <stdio.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;
static HMODULE ace;
#define F(name) ((void *)GetProcAddress(ace, name))

static const char *defs[] = {
    "F,Character,4,0,NOT NULL", "F,Character,4,0,XYZ",      "F,Character,4,NOT NULL",
    "F,Character,4,0,NOT  NULL", "F,Character,4,0,not null", "F,Numeric,5,1,NOT NULL",
    "F,Logical,NOT NULL",        "F,Character,4,0,NULL",     "F,Character,4,0,NOCPTRANS",
    "F,Character,4,0,0",         "F,Character,4,0,",         NULL,
};

static void json_text(const U8 *text, unsigned len) {
    printf("\"");
    for (unsigned i = 0; i < len; i++) {
        if (text[i] == '"' || text[i] == '\\') printf("\\%c", text[i]);
        else if (text[i] >= 0x20 && text[i] < 0x7F) printf("%c", text[i]);
        else printf("\\u%04x", text[i]);
    }
    printf("\"");
}

int main(int argc, char **argv) {
    if (argc != 3 || !argv[2][0]) return 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) return 1;
    H conn = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
        (U8 *)argv[2], 1, NULL, NULL, 0, &conn);
    if (rc) return printf("connect: %lu\n", rc), 1;
    static const struct { const char *name; U16 type; } types[] = {
        {"NTX", 1}, {"CDX", 2}, {"ADT", 3}, {"VFP", 4}};
    int first = 1;
    printf("[\n");
    for (int t = 0; t < 4; t++) {
        for (int d = 0; defs[d]; d++) {
            char name[16];
            H table = 0;
            snprintf(name, sizeof name, "a%u_%d", types[t].type, d);
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *))F(
                "AdsCreateTable"))(conn, (U8 *)name, NULL, types[t].type, 1, 0, 1, 0,
                                   (U8 *)defs[d], &table);
            printf("%s{\"type\": \"%s\", \"def\": \"%s\", \"rc\": %lu", first ? "" : ",\n",
                   types[t].name, defs[d], rc);
            first = 0;
            if (rc) {
                U8 text[600];
                U16 len = sizeof text;
                U32 code = 0;
                ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
                printf(", \"error\": ");
                json_text(text, len);
            } else {
                ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
            }
            printf("}");
        }
    }
    printf("\n]\n");
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
    return 0;
}
