/* Records what an Advantage Client Engine does with the text of Visual FoxPro
 * tables opened as OEM, for fixtures/dbf/vfp_oem.json.
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o vfp_oem.exe vfp_oem.c
 *     cd <empty folder> && wine vfp_oem.exe <ace32.dll> <the folder as a Windows path>
 *
 * The original resolves relative table names against the current directory,
 * so the folder must be the current one too. For each case (a CDX table made
 * OEM, to compare with; VFP tables made with the char types ADS_ANSI, ADS_OEM,
 * 117, 118, 26 (GENERAL_VFP_CI_AS_437) and 12 (GENERAL_VFP_CI_AS_1252); and a
 * VFP table made ADS_ANSI with the collations "oem", GENERAL_VFP_CI_AS_1252,
 * GENERAL_VFP_CI_AS_437 and TURKISH_ADS_CS_AS_857 by AdsCreateTable90) the
 * probe makes the table v<case> of the field C C(6), appends the records
 *
 *     1 "A"  2 "\xC4"  3 "\xE4"  4 "a"  5 "z"  6 "\xE9"  7 "\xDF"  8 "\x8E"
 *
 * with AdsSetString and makes an order on C (tag K in the table's structural
 * CDX file). Each row is
 *
 *     {"case", "step", "rc", "char_type", "strings": [hex...], "wide": [hex...],
 *      "keys": [hex...], "walk": [records], "seek": [rc, found, at],
 *      "filter": count, "raw": [hex...]}
 *
 * for the steps "create" (the table as made), "ansi" and "oem" (closed and
 * opened again with ADS_ANSI or ADS_OEM and no collation, by AdsOpenTable) and
 * "own" (opened again
 * as it was made, by AdsOpenTable90). "strings" are AdsGetString (right
 * trimmed) of records 1 to 8 in hex, "wide" AdsGetStringW's UTF-16 code
 * units, "keys" AdsExtractKey of the order K, "walk" the records in its order,
 * "seek" AdsSeek of "\xE4" (hard, ADS_STRINGKEY) from record 1 ("at" is "E" at
 * EOF), "filter" the records the filter C = '\xE4' lets through, and "raw"
 * the field's bytes in the file once the table is closed again. */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;
static HMODULE ace;
#define F(name) ((void *)GetProcAddress(ace, name))

static const char *values[] = {"A", "\xC4", "\xE4", "a", "z", "\xE9", "\xDF", "\x8E"};
#define RECORDS 8

static void hex(const U8 *bytes, unsigned len) {
    printf("\"");
    for (unsigned i = 0; i < len; i++) printf("%02x", bytes[i]);
    printf("\"");
}

static void record_at(H table) {
    U16 eof = 0;
    U32 recno = 0;
    ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(table, &eof);
    ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordNum"))(table, 2, &recno);
    if (eof) printf("\"E\"");
    else printf("%lu", recno);
}

/* Prints the row's fields read through the open table and its order. */
static void read_table(H table, H order) {
    U16 char_type = 0;
    ((U32(WINAPI *)(H, U16 *))F("AdsGetTableCharType"))(table, &char_type);
    printf(", \"char_type\": %u, \"strings\": [", char_type);
    for (U32 r = 1; r <= RECORDS; r++) {
        U8 text[64];
        U32 len = sizeof text;
        ((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(table, r);
        U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U32 *, U16))F("AdsGetString"))(
            table, (U8 *)"C", text, &len, 2);
        if (r > 1) printf(", ");
        if (rc) printf("\"E%lu\"", rc);
        else hex(text, len);
    }
    printf("], \"wide\": [");
    for (U32 r = 1; r <= RECORDS; r++) {
        WCHAR text[64];
        U32 len = 64;
        ((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(table, r);
        U32 rc = ((U32(WINAPI *)(H, U8 *, WCHAR *, U32 *, U16))F("AdsGetStringW"))(
            table, (U8 *)"C", text, &len, 2);
        printf("%s\"", r > 1 ? ", " : "");
        if (rc) printf("E%lu", rc);
        else for (U32 i = 0; i < len; i++) printf("%04x", text[i]);
        printf("\"");
    }
    printf("], \"keys\": [");
    for (U32 r = 1; r <= RECORDS && order; r++) {
        U8 key[64];
        U16 len = sizeof key;
        ((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(table, r);
        U32 rc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsExtractKey"))(order, key, &len);
        if (r > 1) printf(", ");
        if (rc) printf("\"E%lu\"", rc);
        else hex(key, len);
    }
    printf("], \"walk\": [");
    if (order) {
        ((U32(WINAPI *)(H))F("AdsGotoTop"))(order);
        for (int i = 0; i < RECORDS + 2; i++) {
            U16 eof = 0;
            U32 recno = 0;
            ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(table, &eof);
            if (eof) break;
            ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordNum"))(table, 2, &recno);
            printf("%s%lu", i ? ", " : "", recno);
            ((U32(WINAPI *)(H, U32))F("AdsSkip"))(order, 1);
        }
    }
    printf("], \"seek\": [");
    if (order) {
        U16 found = 7;
        ((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(table, 1);
        U32 rc = ((U32(WINAPI *)(H, U8 *, U16, U16, U16, U16 *))F("AdsSeek"))(
            order, (U8 *)"\xE4", 1, 1, 2, &found);
        printf("%lu, %u, ", rc, found);
        record_at(table);
    }
    printf("]");
    U32 rc = ((U32(WINAPI *)(H, U8 *))F("AdsSetFilter"))(table, (U8 *)"C = '\xE4'");
    int count = 0;
    if (rc == 0) {
        ((U32(WINAPI *)(H))F("AdsGotoTop"))(table);
        for (int i = 0; i < RECORDS + 2; i++) {
            U16 eof = 0;
            ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(table, &eof);
            if (eof) break;
            count++;
            ((U32(WINAPI *)(H, U32))F("AdsSkip"))(table, 1);
        }
        ((U32(WINAPI *)(H))F("AdsClearFilter"))(table);
        printf(", \"filter\": %d", count);
    } else printf(", \"filter\": \"E%lu\"", rc);
}

/* Prints the field's bytes of each record of the closed table file. */
static void raw(const char *name) {
    char path[64];
    U8 head[32], record[64];
    snprintf(path, sizeof path, "%s.dbf", name);
    FILE *file = fopen(path, "rb");
    printf(", \"raw\": [");
    if (file && fread(head, 1, 32, file) == 32) {
        unsigned start = head[8] | head[9] << 8, size = head[10] | head[11] << 8;
        for (int r = 0; r < RECORDS && size <= sizeof record; r++) {
            fseek(file, start + r * size, SEEK_SET);
            if (fread(record, 1, size, file) != size) break;
            if (r) printf(", ");
            hex(record + 1, 6);
        }
    }
    if (file) fclose(file);
    printf("]");
}

int main(int argc, char **argv) {
    if (argc != 3 || !argv[2][0]) return 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) return 1;
    H conn = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
        (U8 *)argv[2], 1, NULL, NULL, 0, &conn);
    if (rc) return printf("connect: %lu\n", rc), 1;
    static const struct { const char *name; U16 type, char_type; const char *collation; } cases[] = {
        {"CDX oem", 2, 2, NULL},    {"VFP ansi", 4, 1, NULL}, {"VFP oem", 4, 2, NULL},
        {"VFP 117", 4, 117, NULL},  {"VFP 118", 4, 118, NULL}, {"VFP 26", 4, 26, NULL},
        {"VFP 12", 4, 12, NULL},    {"VFP collation oem", 4, 1, "oem"},
        {"VFP collation 12", 4, 1, "GENERAL_VFP_CI_AS_1252"},
        {"VFP collation 26", 4, 1, "GENERAL_VFP_CI_AS_437"},
        {"VFP collation 117", 4, 1, "TURKISH_ADS_CS_AS_857"},
    };
    int n = sizeof cases / sizeof cases[0];
    printf("[\n");
    for (int c = 0; c < n; c++) {
        char name[16];
        H table = 0, order = 0;
        snprintf(name, sizeof name, "v%d", c);
        rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, U32, U8 *, H *))F(
            "AdsCreateTable90"))(conn, (U8 *)name, NULL, cases[c].type, cases[c].char_type, 1, 1,
                                 0, (U8 *)"C,C,6,0", 0, (U8 *)cases[c].collation, &table);
        printf("%s{\"case\": \"%s\", \"step\": \"create\", \"rc\": %lu", c ? ",\n" : "",
               cases[c].name, rc);
        if (rc) {
            printf("}");
            continue;
        }
        for (int i = 0; i < RECORDS; i++) {
            ((U32(WINAPI *)(H))F("AdsAppendRecord"))(table);
            ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetString"))(
                table, (U8 *)"C", (U8 *)values[i], strlen(values[i]));
            ((U32(WINAPI *)(H))F("AdsWriteRecord"))(table);
        }
        rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, U32, H *))F(
            "AdsCreateIndex61"))(table, (U8 *)name, (U8 *)"K", (U8 *)"C", (U8 *)"", (U8 *)"", 2,
                                 512, &order);
        if (rc) printf(", \"index\": %lu", rc), order = 0;
        read_table(table, order);
        ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
        raw(name);
        printf("}");
        for (int step = 0; step < 3; step++) {
            table = order = 0;
            if (step < 2)
                rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F(
                    "AdsOpenTable"))(conn, (U8 *)name, NULL, cases[c].type, step + 1, 1, 1, 0,
                                     &table);
            else
                rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, U8 *, H *))F(
                    "AdsOpenTable90"))(conn, (U8 *)name, NULL, cases[c].type, cases[c].char_type,
                                       1, 1, 0, (U8 *)cases[c].collation, &table);
            printf(",\n{\"case\": \"%s\", \"step\": \"%s\", \"rc\": %lu", cases[c].name,
                   step == 2 ? "own" : step ? "oem" : "ansi", rc);
            if (rc) {
                printf("}");
                continue;
            }
            ((U32(WINAPI *)(H, U8 *, H *))F("AdsGetIndexHandle"))(table, (U8 *)"K", &order);
            read_table(table, order);
            ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
            raw(name);
            printf("}");
        }
    }
    printf("\n]\n");
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
    return 0;
}
