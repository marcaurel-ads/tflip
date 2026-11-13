/* Probes the *structure* of table encryption: how the keystream is selected for
 * each record, so the "advances per record" result from crypto.c can be pinned
 * down black-box, without any disassembly (see docs/REFERENCES.md "Deliberately
 * excluded").
 *
 *     stream.exe <ace32.dll> <data folder> <password> <fields> <records> [tag]  > out.json
 *
 * <fields> Character(250) columns give 250*<fields> data bytes per record;
 * <records> all-zero records are appended, so each record's on-disk ciphertext
 * *is* that record's keystream. Built with i686-w64-mingw32-gcc and run under
 * Wine (see run.sh). It:
 *
 *   1. creates a free ADT table with <fields> Character(250) columns;
 *   2. encrypts it with <password> and appends <records> all-zero records;
 *   3. reopens it WITHOUT the password and reads every record back, capturing
 *      each record image (the API hands out ciphertext, per crypto.c).
 *
 * The point is to vary two things across runs (run.sh) and compare, to tell a
 * keystream keyed by the record *number* from one that is a continuous stream
 * advancing by record *bytes*:
 *
 *   - same password, different record WIDTH: if record r's keystream is the same
 *     whatever the record width, the stream is selected by record number; if it
 *     shifts with the width, position is a byte offset into one stream.
 *   - same password, many records: scan for any period or repeat in record
 *     number (analyze.py), i.e. how the record number maps to a stream.
 *
 * No part of this reads the engine's code; it only drives the documented API and
 * reads the records the engine wrote. */
#include <windows.h>
#include <stdio.h>
#include <string.h>
#include <stdlib.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef long S32;
typedef ULONG_PTR H;

static HMODULE ace;
#define F(name) GetProcAddress(ace, name)
#define FIELD(n) ((U8 *)(ULONG_PTR)(n))
#define MAX_FIELDS 16
#define MAX_RECORDS 400

static int first_item = 1;

static void hex_raw(const U8 *bytes, size_t n) {
    for (size_t i = 0; i < n; i++) printf("%02x", bytes[i]);
}

static void num(const char *key, long v) {
    printf("%s\"%s\": %ld", first_item ? "" : ", ", key, v);
    first_item = 0;
}

static void str(const char *key, const char *s) {
    printf("%s\"%s\": \"", first_item ? "" : ", ", key);
    for (; *s; s++) {
        if (*s == '"' || *s == '\\') printf("\\%c", *s);
        else if ((U8)*s < 32 || (U8)*s > 126) printf("\\u%04x", (U8)*s);
        else putchar(*s);
    }
    printf("\"");
    first_item = 0;
}

static void last_error(void) {
    U32 code = 0;
    U8 text[600];
    U16 len = sizeof text;
    ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
    num("err", code);
    text[len < sizeof text ? len : sizeof text - 1] = 0;
    str("errtext", (char *)text);
}

static U32 set_raw(H t, int field, const U8 *bytes, U32 n) {
    return ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetFieldRaw"))(t, FIELD(field), (U8 *)bytes, n);
}

typedef U32(WINAPI *OpenTable)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *);

static U8 zeros[MAX_FIELDS * 250];
static U8 record_buf[MAX_FIELDS * 250 + 64];

int main(int argc, char **argv) {
    if (argc < 6) {
        fprintf(stderr, "usage: stream <ace32.dll> <data folder> <password> <fields> <records> [tag]\n");
        return 2;
    }
    const char *data_folder = argv[2];
    const char *password = argv[3];
    int fields = atoi(argv[4]);
    int records = atoi(argv[5]);
    const char *tag = argc > 6 ? argv[6] : "";
    if (fields < 1 || fields > MAX_FIELDS) fields = 1;
    if (records < 1 || records > MAX_RECORDS) records = 1;
    int data_bytes = fields * 250;

    ace = LoadLibraryA(argv[1]);
    if (!ace) return fprintf(stderr, "cannot load %s\n", argv[1]), 1;
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);

    char file[MAX_PATH], path[MAX_PATH], cols[MAX_FIELDS * 24];
    snprintf(file, sizeof file, "st%s.adt", tag);
    snprintf(path, sizeof path, "%s\\%s", data_folder, file);
    cols[0] = 0;
    for (int i = 1; i <= fields; i++) {
        char one[24];
        snprintf(one, sizeof one, "%sD%d,Character,250", i == 1 ? "" : ";", i);
        strncat(cols, one, sizeof cols - strlen(cols) - 1);
    }

    printf("{");
    str("password", password);
    str("file", file);
    num("fields", fields);
    num("data_bytes", data_bytes);
    num("records_requested", records);

    H conn = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, H *))F("AdsConnect26"))((U8 *)data_folder, 1, &conn);
    num("connect_rc", rc);
    if (rc) return printf("}\n"), 1;

    H t = 0;
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, U32, H *))F(
        "AdsCreateTable71"))(conn, (U8 *)path, NULL, 3 /*ADT*/, 1 /*ansi*/, 1, 1, 0,
                             (U8 *)cols, 0, &t);
    num("create_rc", rc);
    if (rc) { last_error(); return printf("}\n"), 1; }

    rc = ((U32(WINAPI *)(H, U8 *))F("AdsEnableEncryption"))(t, (U8 *)password);
    num("enable_rc", rc);
    rc = ((U32(WINAPI *)(H))F("AdsEncryptTable"))(t);
    num("encrypt_rc", rc);
    if (rc) last_error();

    for (int r = 0; r < records; r++) {
        U32 arc = ((U32(WINAPI *)(H))F("AdsAppendRecord"))(t);
        for (int c = 1; c <= fields && !arc; c++) arc = set_raw(t, c, zeros, 250);
        if (!arc) arc = ((U32(WINAPI *)(H))F("AdsWriteRecord"))(t);
        if (arc) { num("append_fail_at", r); last_error(); break; }
    }

    U32 reclen = 0;
    ((U32(WINAPI *)(H, U32 *))F("AdsGetRecordLength"))(t, &reclen);
    num("record_length", reclen);
    U32 count = 0;
    ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(t, 1, &count);
    num("record_count", count);
    ((U32(WINAPI *)(H))F("AdsCloseTable"))(t);
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);

    /* Reopen WITHOUT the password and read every record image (ciphertext). */
    conn = 0;
    ((U32(WINAPI *)(U8 *, U16, H *))F("AdsConnect26"))((U8 *)data_folder, 1, &conn);
    t = 0;
    rc = ((OpenTable)F("AdsOpenTable"))(conn, (U8 *)path, NULL, 3, 1, 1, 1, 0, &t);
    num("open_nopw_rc", rc);
    if (!rc) {
        ((U32(WINAPI *)(H))F("AdsGotoTop"))(t);
        printf("%s\"records\": [", first_item ? "" : ", ");
        first_item = 0;
        for (U32 r = 0; r < count; r++) {
            U32 len = sizeof record_buf;
            U32 grc = ((U32(WINAPI *)(H, U8 *, U32 *))F("AdsGetRecord"))(t, record_buf, &len);
            if (grc) break;
            if (r) printf(", ");
            printf("\"");
            hex_raw(record_buf, len < sizeof record_buf ? len : sizeof record_buf);
            printf("\"");
            if (((U32(WINAPI *)(H, S32))F("AdsSkip"))(t, 1)) break;
        }
        printf("]");
        ((U32(WINAPI *)(H))F("AdsCloseTable"))(t);
    }
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);

    printf("}\n");
    return 0;
}
