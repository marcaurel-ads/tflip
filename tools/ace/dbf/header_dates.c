/* Records which dates of the last update in a DBF header an Advantage Client
 * Engine opens a table with, for fixtures/dbf/header_dates.json.
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o header_dates.exe header_dates.c
 *     cd <empty folder> && wine header_dates.exe <ace32.dll> <the folder as a Windows path>
 *
 * The original resolves relative table names against the current directory,
 * so the folder must be the current one too. For each table type (ADS_NTX 1,
 * ADS_CDX 2, ADS_VFP 4) and each date below (the header's bytes 1 to 3: the
 * year since 1900, the month, the day), the probe writes the table h<type>_<n>
 * of one field C C(4) and one record "abcd" (a dBASE III header, 0x03, for
 * NTX and CDX; a Visual FoxPro one, 0x30 with its 263-byte backlink area, for
 * VFP) and opens it with AdsOpenTable. Each row is
 *
 *     {"type", "date": [year, month, day], "rc", "error", "value"}
 *
 * "error" is AdsGetLastError's text after a failed open, "value" the field
 * read with AdsGetString after a successful one. */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;
static HMODULE ace;
#define F(name) ((void *)GetProcAddress(ace, name))

static const U8 dates[][3] = {
    {0, 0, 0},    {0, 1, 1},    {124, 0, 1},  {124, 1, 0},   {124, 13, 1},
    {124, 12, 31}, {124, 2, 31}, {255, 12, 31}, {124, 1, 32},  {124, 1, 255},
    {124, 255, 1}, {0, 0, 1},   {0, 1, 0},    {99, 12, 31},  {100, 1, 1},
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
        {"NTX", 1}, {"CDX", 2}, {"VFP", 4}};
    int n = sizeof dates / sizeof dates[0], first = 1;
    printf("[\n");
    for (int t = 0; t < 3; t++) {
        int vfp = types[t].type == 4;
        for (int d = 0; d < n; d++) {
            U8 file[512] = {0};
            int at = 32, header = 32 + 32 + 1 + (vfp ? 263 : 0);
            file[0] = vfp ? 0x30 : 0x03;
            memcpy(file + 1, dates[d], 3);
            file[4] = 1;
            file[8] = header & 0xFF;
            file[9] = header >> 8;
            file[10] = 5;
            file[at] = 'C';
            file[at + 11] = 'C';
            file[at + 16] = 4;
            at += 32;
            file[at++] = 0x0D;
            if (vfp) at += 263;
            file[at++] = ' ';
            memcpy(file + at, "abcd", 4);
            at += 4;
            file[at++] = 0x1A;
            char name[16], path[32];
            snprintf(name, sizeof name, "h%u_%d", types[t].type, d);
            snprintf(path, sizeof path, "%s.dbf", name);
            FILE *out = fopen(path, "wb");
            if (!out) return 1;
            fwrite(file, 1, at, out);
            fclose(out);
            H table = 0;
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
                conn, (U8 *)name, NULL, types[t].type, 1, 1, 1, 0, &table);
            printf("%s{\"type\": \"%s\", \"date\": [%u, %u, %u], \"rc\": %lu", first ? "" : ",\n",
                   types[t].name, dates[d][0], dates[d][1], dates[d][2], rc);
            first = 0;
            if (rc) {
                U8 text[600];
                U16 len = sizeof text;
                U32 code = 0;
                ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
                printf(", \"error\": ");
                json_text(text, len);
            } else {
                U8 text[16];
                U32 len = sizeof text;
                ((U32(WINAPI *)(H))F("AdsGotoTop"))(table);
                ((U32(WINAPI *)(H, U8 *, U8 *, U32 *, U16))F("AdsGetString"))(
                    table, (U8 *)"C", text, &len, 2);
                printf(", \"value\": ");
                json_text(text, len);
                ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
            }
            printf("}");
        }
    }
    printf("\n]\n");
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
    return 0;
}
