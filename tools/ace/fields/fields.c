/* Records what an Advantage Client Engine's typed field getters return, for
 * fixtures/fields/getters.json.
 *
 *     fields.exe <ace32.dll> <data folder>  > getters.json
 *
 * Built with i686-w64-mingw32-gcc and run under Wine on a copy of the
 * all_types fixture (see run.sh). Every getter is called on every field of
 * every record, and at BOF and EOF; then the cases around them (null
 * pointers, short buffers, options, the date format, other handles); then
 * every getter on every record of the adi_keys fixture's table, whose
 * numbers are halves and boundaries. Each call is a JSON object:
 *
 *     {"case": ..., "fn": "AdsGetLong", "field": "c_integer", "rc": 0,
 *      "value": ..., "len": ...}
 *
 * Outputs start as a sentinel (0x5A bytes) and are recorded whatever the
 * return code, so a failure that writes is seen. Text is recorded as hex
 * (the bytes the getter wrote, as far as *len or the sentinel says), so
 * nothing depends on the host's code page. */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;
typedef long long S64;

static HMODULE ace;
#define F(name) GetProcAddress(ace, name)
#define FIELD(n) ((U8 *)(ULONG_PTR)(n))
#define SENTINEL 0x5A

static int first = 1;

static void begin(const char *c, const char *fn, const char *field, U32 rc) {
    printf("%s\n{\"case\": \"%s\", \"fn\": \"%s\", \"field\": \"%s\", \"rc\": %lu", first ? "[" : ",",
           c, fn, field, rc);
    first = 0;
}

static void hex(const char *key, const U8 *bytes, size_t n) {
    printf(", \"%s\": \"", key);
    for (size_t i = 0; i < n; i++) printf("%02x", bytes[i]);
    printf("\"");
}

/* The bytes of `buf` of `size` up to the last one that is not the sentinel. */
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
    U8 buf[1200];
    memset(buf, SENTINEL, sizeof buf);
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
                                "AdsGetMoney", "AdsGetMemoLength", "AdsGetFieldNum",
                                "AdsIsFieldBinary"};
static const int sizes[] = {4, 8, 2, 8, 2, 4, 4, 8, 4, 2, 2};

static void every_getter(const char *c, H t, U8 *field, const char *name) {
    for (int k = 0; k < (int)(sizeof scalars / sizeof *scalars); k++)
        scalar(c, scalars[k], t, field, name, sizes[k]);
    text(c, "AdsGetString", t, field, name, 300, 0);
    text(c, "AdsGetStringW", t, field, name, 300, 0);
    short_text(c, "AdsGetDate", t, field, name, 60);
    short_text(c, "AdsGetTime", t, field, name, 60);
}

static H conn;

static H open_table(const char *folder, const char *name) {
    char path[600];
    H t = 0;
    snprintf(path, sizeof path, "%s\\%s", folder, name);
    ((U32(WINAPI *)(H, char *, char *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
        conn, path, NULL, 3, 1, 1, 1, 4, &t);
    return t;
}

/* Every getter on every field of `t`, record by record, then at EOF; the
 * cases are named "<prefix>record <n>" and "<prefix>eof". */
static void every_record(H t, const char *prefix) {
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
        /* A table that did not open (a collation mismatch) has no records. */
        if (((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(t, &eof)) break;
        char c[40];
        if (eof) snprintf(c, sizeof c, "%seof", prefix);
        else snprintf(c, sizeof c, "%srecord %d", prefix, record);
        for (U16 i = 1; i <= nfields && i < 64; i++) every_getter(c, t, (U8 *)names[i], names[i]);
        if (eof) break;
        ((U32(WINAPI *)(H, long))F("AdsSkip"))(t, 1);
    }
}

int main(int argc, char **argv) {
    ace = LoadLibraryA(argv[1]);
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    ((U32(WINAPI *)(char *, U16, H *))F("AdsConnect26"))(argv[2], 1, &conn);
    H t = open_table(argv[2], "all_types.adt");
    every_record(t, "");
    ((U32(WINAPI *)(H))F("AdsGotoTop"))(t);
    ((U32(WINAPI *)(H, long))F("AdsSkip"))(t, -1);
    every_getter("bof", t, (U8 *)"c_integer", "c_integer");
    every_getter("bof", t, (U8 *)"c_char", "c_char");

    /* Around them, on the first record. */
    ((U32(WINAPI *)(H))F("AdsGotoTop"))(t);
    every_getter("field number", t, FIELD(15), "15");
    every_getter("field 0", t, FIELD(0), "0");
    every_getter("field 25", t, FIELD(25), "25");
    every_getter("no such field", t, (U8 *)"nosuch", "nosuch");
    every_getter("upper case", t, (U8 *)"C_INTEGER", "C_INTEGER");
    every_getter("connection", conn, (U8 *)"c_integer", "c_integer");
    for (int k = 0; k < (int)(sizeof scalars / sizeof *scalars); k++) {
        U32 rc = ((Getter)F(scalars[k]))(t, (U8 *)"c_integer", NULL);
        begin("null output", scalars[k], "c_integer", rc);
        printf("}");
    }
    const char *texts[] = {"c_char", "c_varchar", "c_nchar", "c_memo", "c_nmemo", "c_blob",
                           "c_raw", "c_guid", "c_numeric", "c_double", "c_date", "c_time",
                           "c_timestamp", "c_logical"};
    for (int k = 0; k < (int)(sizeof texts / sizeof *texts); k++) {
        U8 *f = (U8 *)texts[k];
        U16 options[] = {1, 2, 3, 4, 8, 16};
        for (int o = 0; o < 6; o++) {
            text("option", "AdsGetString", t, f, texts[k], 300, options[o]);
            text("option", "AdsGetStringW", t, f, texts[k], 300, options[o]);
            text("option", "AdsGetField", t, f, texts[k], 300, options[o]);
            text("option", "AdsGetFieldW", t, f, texts[k], 300, options[o]);
        }
        U32 lens[] = {0, 1, 3, 4, 5, 10, 11};
        for (int l = 0; l < 7; l++) {
            text("short buffer", "AdsGetString", t, f, texts[k], lens[l], 0);
            text("short buffer", "AdsGetStringW", t, f, texts[k], lens[l], 0);
        }
        U16 dlens[] = {0, 1, 8, 9, 10, 11, 12, 13};
        for (int l = 0; l < 8; l++) {
            short_text("short buffer", "AdsGetDate", t, f, texts[k], dlens[l]);
            short_text("short buffer", "AdsGetTime", t, f, texts[k], dlens[l]);
        }
        U32 rc = ((TextGetter)F("AdsGetString"))(t, f, NULL, NULL, 0);
        begin("null buffer", "AdsGetString", texts[k], rc);
        printf("}");
        U32 n = 0;
        rc = ((TextGetter)F("AdsGetString"))(t, f, NULL, &n, 0);
        begin("null buffer, length 0", "AdsGetString", texts[k], rc);
        printf(", \"len\": %lu}", n);
        U16 dn = 60;
        rc = ((ShortTextGetter)F("AdsGetDate"))(t, f, NULL, &dn);
        begin("null buffer", "AdsGetDate", texts[k], rc);
        printf("}");
    }
    const char *formats[] = {"DD.MM.CCYY", "YYYY/MM/DD", "MM/DD/YY", "D.M.CCYY"};
    for (int k = 0; k < 4; k++) {
        ((U32(WINAPI *)(U8 *))F("AdsSetDateFormat"))((U8 *)formats[k]);
        char c[40];
        snprintf(c, sizeof c, "date format %s", formats[k]);
        short_text(c, "AdsGetDate", t, (U8 *)"c_date", "c_date", 60);
        short_text(c, "AdsGetDate", t, (U8 *)"c_timestamp", "c_timestamp", 60);
        text(c, "AdsGetString", t, (U8 *)"c_date", "c_date", 300, 0);
        short_text(c, "AdsGetDate", t, (U8 *)"c_date", "c_date", 8);
    }
    ((U32(WINAPI *)(U8 *))F("AdsSetDateFormat"))((U8 *)"MM/DD/CCYY");

    /* Boundaries and halves of every numeric type (the adi_keys fixture). */
    every_record(open_table(argv[2], "keys.adt"), "keys ");
    return printf("\n]\n") < 0;
}
