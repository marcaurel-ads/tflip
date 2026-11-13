/* Probes how the Advantage Client Engine encrypts a table's records, so the
 * cipher and its key derivation can be studied black-box, without any
 * disassembly (see docs/REFERENCES.md "Deliberately excluded").
 *
 *     crypto.exe <ace32.dll> <data folder> <password> [tag]  > out.json
 *
 * Built with i686-w64-mingw32-gcc and run under Wine (see run.sh). It:
 *
 *   1. creates a free ADT table (two Character(250) fields: 500 data bytes we
 *      control exactly) in the data folder, as enc<tag>.adt;
 *   2. turns on encryption with <password> (AdsEnableEncryption on the table
 *      handle) and AdsEncryptTable;
 *   3. appends records of known raw bytes through AdsSetFieldRaw:
 *        - two records of all 0x00 (so the ciphertext *is* the keystream),
 *        - one record of 0x00,0x01,...,0xFF repeating (to cross-check the
 *          keystream: ciphertext XOR plaintext),
 *        - one record of all 0xFF;
 *   4. closes, and reads the raw .adt file back (so the on-disk ciphertext is
 *      captured whatever the API does);
 *   5. reopens the table *without* a password and records what AdsGetRecord,
 *      AdsGetField, AdsIsTableEncrypted and AdsIsRecordEncrypted return (the
 *      question of whether the API hands out ciphertext);
 *   6. reopens it *with* the password and records AdsGetRecord, to confirm the
 *      plaintext round-trips.
 *
 * Everything is emitted as one JSON object. Run the same scenario under
 * several passwords (run.sh) and compare the keystreams:
 *   - the all-zero records' ciphertext is the keystream at those byte offsets;
 *   - if it changes with the password, the key is password-derived (compare
 *     "A" vs "AAAA" vs "AAAAAAAA" vs "aaaaaaaa" to learn how);
 *   - if the two all-zero records differ, the keystream advances per record
 *     (an IV or the record number is mixed in); if they match, it resets.
 *
 * No part of this reads the engine's code; it only drives the documented API
 * and reads files the engine wrote. */
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
#define DATA_BYTES 500 /* two Character(250) fields */

static const char *data_folder;
static int first_item = 1;

static void hex(const char *key, const U8 *bytes, size_t n) {
    printf("%s\"%s\": \"", first_item ? "" : ", ", key);
    for (size_t i = 0; i < n; i++) printf("%02x", bytes[i]);
    printf("\"");
    first_item = 0;
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

/* The last error's code and text, for a failed call. */
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

/* Appends one record whose 500 data bytes are `data`. */
static U32 append_record(H t, const U8 *data) {
    U32 rc = ((U32(WINAPI *)(H))F("AdsAppendRecord"))(t);
    if (!rc) rc = set_raw(t, 1, data, 250);
    if (!rc) rc = set_raw(t, 2, data + 250, 250);
    if (!rc) rc = ((U32(WINAPI *)(H))F("AdsWriteRecord"))(t);
    return rc;
}

/* Reads a whole file into `buf` (up to `max`); returns its length, or -1. */
static long read_file(const char *path, U8 *buf, long max) {
    FILE *f = fopen(path, "rb");
    if (!f) return -1;
    long n = (long)fread(buf, 1, max, f);
    fclose(f);
    return n;
}

static U8 record_buf[4096];

/* AdsGetRecord of the current record, as "rec<which>". */
static void dump_record(H t, const char *which) {
    U32 len = sizeof record_buf;
    U32 rc = ((U32(WINAPI *)(H, U8 *, U32 *))F("AdsGetRecord"))(t, record_buf, &len);
    char key[32];
    snprintf(key, sizeof key, "%s_rc", which);
    num(key, rc);
    snprintf(key, sizeof key, "%s", which);
    hex(key, record_buf, rc ? 0 : (len < sizeof record_buf ? len : sizeof record_buf));
}

typedef U32(WINAPI *OpenTable)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *);

int main(int argc, char **argv) {
    if (argc < 4) {
        fprintf(stderr, "usage: crypto <ace32.dll> <data folder> <password> [tag]\n");
        return 2;
    }
    data_folder = argv[2];
    const char *password = argv[3];
    const char *tag = argc > 4 ? argv[4] : "";
    ace = LoadLibraryA(argv[1]);
    if (!ace) return fprintf(stderr, "cannot load %s\n", argv[1]), 1;
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);

    char file[MAX_PATH], path[MAX_PATH];
    snprintf(file, sizeof file, "enc%s.adt", tag);
    snprintf(path, sizeof path, "%s\\%s", data_folder, file);

    printf("{");
    str("password", password);
    str("file", file);

    H conn = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, H *))F("AdsConnect26"))((U8 *)data_folder, 1, &conn);
    num("connect_rc", rc);
    if (rc) return printf("}\n"), 1;

    /* 1. Create the table. */
    H t = 0;
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, U32, H *))F(
        "AdsCreateTable71"))(conn, (U8 *)path, NULL, 3 /*ADT*/, 1 /*ansi*/, 1, 1, 0,
                             (U8 *)"D1,Character,250;D2,Character,250", 0, &t);
    num("create_rc", rc);
    if (rc) {
        last_error();
        return printf("}\n"), 1;
    }

    /* 2. Turn on encryption and encrypt the (empty) table. */
    rc = ((U32(WINAPI *)(H, U8 *))F("AdsEnableEncryption"))(t, (U8 *)password);
    num("enable_rc", rc);
    rc = ((U32(WINAPI *)(H))F("AdsEncryptTable"))(t);
    num("encrypt_rc", rc);
    if (rc) last_error();

    /* 3. Append the known records. */
    static U8 zeros[DATA_BYTES], ramp[DATA_BYTES], ones[DATA_BYTES];
    for (int i = 0; i < DATA_BYTES; i++) {
        zeros[i] = 0x00;
        ramp[i] = (U8)(i & 0xFF);
        ones[i] = 0xFF;
    }
    num("append0_rc", append_record(t, zeros));
    num("append1_rc", append_record(t, zeros));
    num("append2_rc", append_record(t, ramp));
    num("append3_rc", append_record(t, ones));

    U32 reclen = 0;
    ((U32(WINAPI *)(H, U32 *))F("AdsGetRecordLength"))(t, &reclen);
    num("record_length", reclen);
    U32 count = 0;
    ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(t, 1, &count);
    num("record_count", count);

    ((U32(WINAPI *)(H))F("AdsCloseTable"))(t);
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);

    /* 4. The raw file the engine wrote (header + encrypted records). */
    static U8 filebuf[1 << 16];
    long flen = read_file(path, filebuf, sizeof filebuf);
    num("file_length", flen);
    hex("file_hex", filebuf, flen < 0 ? 0 : flen);

    /* 5. Reopen WITHOUT the password: does the API hand out ciphertext? */
    conn = 0;
    rc = ((U32(WINAPI *)(U8 *, U16, H *))F("AdsConnect26"))((U8 *)data_folder, 1, &conn);
    t = 0;
    rc = ((OpenTable)F("AdsOpenTable"))(conn, (U8 *)path, NULL, 3, 1, 1, 1, 0, &t);
    num("open_nopw_rc", rc);
    if (!rc) {
        U16 enc = 0x55;
        U32 r = ((U32(WINAPI *)(H, U16 *))F("AdsIsTableEncrypted"))(t, &enc);
        num("is_table_encrypted_rc", r);
        num("is_table_encrypted", enc);
        ((U32(WINAPI *)(H))F("AdsGotoTop"))(t);
        enc = 0x55;
        r = ((U32(WINAPI *)(H, U16 *))F("AdsIsRecordEncrypted"))(t, &enc);
        num("is_record_encrypted_rc", r);
        num("is_record_encrypted", enc);
        dump_record(t, "nopw_rec0");
        /* AdsGetField of the first field without the password. */
        U32 len = sizeof record_buf;
        U32 fr = ((U32(WINAPI *)(H, U8 *, U8 *, U32 *, U16))F("AdsGetField"))(t, FIELD(1),
                                                                             record_buf, &len, 0);
        num("nopw_getfield_rc", fr);
        hex("nopw_field1", record_buf, fr ? 0 : (len < sizeof record_buf ? len : 0));
        ((U32(WINAPI *)(H, S32))F("AdsSkip"))(t, 1);
        dump_record(t, "nopw_rec1");
        ((U32(WINAPI *)(H))F("AdsCloseTable"))(t);
    }
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);

    /* 6. Reopen WITH the password: plaintext should round-trip. */
    conn = 0;
    ((U32(WINAPI *)(U8 *, U16, H *))F("AdsConnect26"))((U8 *)data_folder, 1, &conn);
    t = 0;
    rc = ((OpenTable)F("AdsOpenTable"))(conn, (U8 *)path, NULL, 3, 1, 1, 1, 0, &t);
    num("open_pw_rc", rc);
    if (!rc) {
        U32 er = ((U32(WINAPI *)(H, U8 *))F("AdsEnableEncryption"))(t, (U8 *)password);
        num("reenable_rc", er);
        U16 en = 0x55;
        ((U32(WINAPI *)(H, U16 *))F("AdsIsEncryptionEnabled"))(t, &en);
        num("is_encryption_enabled", en);
        ((U32(WINAPI *)(H))F("AdsGotoTop"))(t);
        dump_record(t, "pw_rec0");
        ((U32(WINAPI *)(H, S32))F("AdsSkip"))(t, 1);
        dump_record(t, "pw_rec1");
        ((U32(WINAPI *)(H))F("AdsCloseTable"))(t);
    }
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);

    printf("}\n");
    return 0;
}
