/* Records what an Advantage Client Engine reports about DBF tables, for
 * fixtures/dbf/getters.json.
 *
 *     dbf.exe <ace32.dll> <data folder> <type> <char type> <table> ...  > getters.json
 *
 * Built with i686-w64-mingw32-gcc and run under Wine on copies of the DBF
 * fixtures (see run.sh). Each table is opened as the table type and
 * character type given before it (ADS_NTX 1, ADS_CDX 2, ADS_VFP 4; ADS_ANSI
 * 1, ADS_OEM 2). For each table: its record count, last update, memo block
 * size, record length and fields; then every getter on every field of every
 * record and at EOF, with AdsIsNull, AdsIsEmpty and AdsIsRecordDeleted; then
 * the records it moves through with AdsShowDeleted(FALSE). On an OEM table
 * only the text getters are called, whose conversion it is about. Each call is a
 * JSON object as in fields.c, with the table:
 *
 *     {"table": "ntx/dbf_types.dbf", "case": "record 1", "fn": "AdsGetLong",
 *      "field": "C_NUMERIC", "rc": 0, "raw": "..."}
 *
 * Outputs start as a sentinel (0x5A bytes) and are recorded whatever the
 * return code. Text is recorded as hex. */
#include <windows.h>
#include <stdio.h>
#include <string.h>
#include <stdlib.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;

static HMODULE ace;
#define F(name) GetProcAddress(ace, name)
#define SENTINEL 0x5A

static int first = 1;
static const char *table_name = "";

static void begin(const char *c, const char *fn, const char *field, U32 rc) {
    printf("%s\n{\"table\": \"%s\", \"case\": \"%s\", \"fn\": \"%s\", \"field\": \"%s\", \"rc\": %lu",
           first ? "[" : ",", table_name, c, fn, field, rc);
    first = 0;
}

static void hex(const char *key, const U8 *bytes, size_t n) {
    printf(", \"%s\": \"", key);
    for (size_t i = 0; i < n; i++) printf("%02x", bytes[i]);
    printf("\"");
}

static size_t written(const U8 *buf, size_t size) {
    while (size && buf[size - 1] == SENTINEL) size--;
    return size;
}

typedef U32(WINAPI *Getter)(H, U8 *, void *);
typedef U32(WINAPI *TextGetter)(H, U8 *, U8 *, U32 *, U16);
typedef U32(WINAPI *ShortTextGetter)(H, U8 *, U8 *, U16 *);

static void scalar(const char *c, const char *fn, H t, U8 *field, const char *name, int size) {
    U8 out[8];
    memset(out, SENTINEL, sizeof out);
    U32 rc = ((Getter)F(fn))(t, field, out);
    begin(c, fn, name, rc);
    if (!strcmp(fn, "AdsGetDouble")) {
        double d;
        memcpy(&d, out, 8);
        printf(", \"value\": \"%.17g\"", d);
    }
    hex("raw", out, size);
    printf("}");
}

static void text(const char *c, const char *fn, H t, U8 *field, const char *name, U32 len,
                 U16 option) {
    static U8 buf[140000];
    memset(buf, SENTINEL, len * 2 + 8);
    U32 n = len;
    U32 rc = ((TextGetter)F(fn))(t, field, buf, &n, option);
    begin(c, fn, name, rc);
    printf(", \"size\": %lu, \"option\": %u, \"len\": %lu", len, option, n);
    size_t unit = strcmp(fn, "AdsGetStringW") && strcmp(fn, "AdsGetFieldW") ? 1 : 2;
    hex("value", buf, written(buf, len * unit + 4));
    printf("}");
}

static void short_text(const char *c, const char *fn, H t, U8 *field, const char *name, U16 len) {
    U8 buf[200];
    memset(buf, SENTINEL, sizeof buf);
    U16 n = len;
    U32 rc = ((ShortTextGetter)F(fn))(t, field, buf, &n);
    begin(c, fn, name, rc);
    printf(", \"size\": %u, \"len\": %u", len, n);
    hex("value", buf, written(buf, len + 4u));
    printf("}");
}

static const char *scalars[] = {"AdsGetLong", "AdsGetLongLong", "AdsGetShort", "AdsGetDouble",
                                "AdsGetLogical", "AdsGetJulian", "AdsGetMilliseconds",
                                "AdsGetMoney", "AdsGetMemoLength", "AdsIsFieldBinary",
                                "AdsIsNull", "AdsIsEmpty", "AdsGetFieldType",
                                "AdsGetFieldDecimals", "AdsGetFieldLength"};
static const int sizes[] = {4, 8, 2, 8, 2, 4, 4, 8, 4, 2, 2, 2, 2, 2, 4};

static int text_only = 0;

static void every_getter(const char *c, H t, U8 *field, const char *name) {
    if (!text_only)
        for (int k = 0; k < (int)(sizeof scalars / sizeof *scalars); k++)
            scalar(c, scalars[k], t, field, name, sizes[k]);
    text(c, "AdsGetString", t, field, name, 66000, 0);
    text(c, "AdsGetStringW", t, field, name, 66000, 0);
    text(c, "AdsGetField", t, field, name, 66000, 0);
    if (!text_only) short_text(c, "AdsGetDate", t, field, name, 60);
}

/* A call on the table that writes a number of `size` bytes. */
static void table_number(const char *c, const char *fn, U32 rc, const void *out, int size) {
    begin(c, fn, "", rc);
    hex("raw", out, size);
    printf("}");
}

static void every_record(H t) {
    U16 nfields = 0;
    ((U32(WINAPI *)(H, U16 *))F("AdsGetNumFields"))(t, &nfields);
    char names[64][32];
    for (U16 i = 1; i <= nfields && i < 64; i++) {
        U16 l = sizeof names[i];
        ((U32(WINAPI *)(H, U16, U8 *, U16 *))F("AdsGetFieldName"))(t, i, (U8 *)names[i], &l);
    }
    ((U32(WINAPI *)(H))F("AdsGotoTop"))(t);
    for (int record = 1;; record++) {
        U16 eof = 0;
        if (((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(t, &eof)) break;
        char c[40];
        if (eof) snprintf(c, sizeof c, "eof");
        else snprintf(c, sizeof c, "record %d", record);
        U8 out[8];
        memset(out, SENTINEL, sizeof out);
        U32 rc = ((U32(WINAPI *)(H, U16 *))F("AdsIsRecordDeleted"))(t, (U16 *)out);
        table_number(c, "AdsIsRecordDeleted", rc, out, 2);
        for (U16 i = 1; i <= nfields && i < 64; i++) every_getter(c, t, (U8 *)names[i], names[i]);
        if (eof) break;
        ((U32(WINAPI *)(H, long))F("AdsSkip"))(t, 1);
    }
}

static void about(H t) {
    U8 out[64];
    U32 rc;
    for (U16 option = 1; option <= 3; option++) {
        memset(out, SENTINEL, sizeof out);
        rc = ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(t, option, (U32 *)out);
        char c[40];
        snprintf(c, sizeof c, "option %u", option);
        table_number(c, "AdsGetRecordCount", rc, out, 4);
    }
    memset(out, SENTINEL, sizeof out);
    rc = ((U32(WINAPI *)(H, U16 *))F("AdsGetMemoBlockSize"))(t, (U16 *)out);
    table_number("table", "AdsGetMemoBlockSize", rc, out, 2);
    memset(out, SENTINEL, sizeof out);
    rc = ((U32(WINAPI *)(H, U32 *))F("AdsGetRecordLength"))(t, (U32 *)out);
    table_number("table", "AdsGetRecordLength", rc, out, 4);
    memset(out, SENTINEL, sizeof out);
    rc = ((U32(WINAPI *)(H, U16 *))F("AdsGetTableCharType"))(t, (U16 *)out);
    table_number("table", "AdsGetTableCharType", rc, out, 2);
    memset(out, SENTINEL, sizeof out);
    U16 n = 20;
    rc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsGetLastTableUpdate"))(t, out, &n);
    begin("table", "AdsGetLastTableUpdate", "", rc);
    printf(", \"len\": %u", n);
    hex("value", out, written(out, 24));
    printf("}");
}

/* The records a walk passes with deleted records hidden, then the counts. */
static void hidden_deleted(H t) {
    ((U32(WINAPI *)(U16))F("AdsShowDeleted"))(0);
    U32 rc = ((U32(WINAPI *)(H))F("AdsGotoTop"))(t);
    printf(",\n{\"table\": \"%s\", \"case\": \"hidden deleted\", \"fn\": \"walk\", \"field\": \"\", "
           "\"rc\": %lu, \"records\": [", table_name, rc);
    for (int i = 0; i < 100000; i++) {
        U16 eof = 1;
        ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(t, &eof);
        if (eof) break;
        U32 rec = 0;
        ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordNum"))(t, 1, &rec);
        printf("%s%lu", i ? ", " : "", rec);
        ((U32(WINAPI *)(H, long))F("AdsSkip"))(t, 1);
    }
    printf("]}");
    U8 out[8];
    for (U16 option = 1; option <= 2; option++) {
        memset(out, SENTINEL, sizeof out);
        rc = ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(t, option, (U32 *)out);
        char c[40];
        snprintf(c, sizeof c, "hidden deleted, option %u", option);
        table_number(c, "AdsGetRecordCount", rc, out, 4);
    }
    ((U32(WINAPI *)(U16))F("AdsShowDeleted"))(1);
}

int main(int argc, char **argv) {
    ace = LoadLibraryA(argv[1]);
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    H conn = 0;
    ((U32(WINAPI *)(char *, U16, H *))F("AdsConnect26"))(argv[2], 1, &conn);
    for (int a = 3; a + 2 < argc; a += 3) {
        U16 type = (U16)atoi(argv[a]), char_type = (U16)atoi(argv[a + 1]);
        table_name = argv[a + 2];
        text_only = char_type == 2;
        char path[600];
        snprintf(path, sizeof path, "%s\\%s", argv[2], argv[a + 2]);
        for (char *p = path; *p; p++)
            if (*p == '/') *p = '\\';
        H t = 0;
        U32 rc = ((U32(WINAPI *)(H, char *, char *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
            conn, path, NULL, type, char_type, 1, 1, 0, &t);
        begin("open", "AdsOpenTable", "", rc);
        printf("}");
        if (rc) continue;
        about(t);
        every_record(t);
        hidden_deleted(t);
        ((U32(WINAPI *)(H))F("AdsCloseTable"))(t);
    }
    return printf("\n]\n") < 0;
}
