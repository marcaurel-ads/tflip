/* Black-box probe: is a table's encryption password per handle?
 *
 *     share.exe <ace dll> <data folder>  > share.json
 *
 * The data folder must hold copies of the files of
 * fixtures/adt/dictionary_encrypted (rc4.add, ADSSYS password "adssyspw").
 * Run it from a folder with the patched engine's 64-bit DLLs, whose OpenSSL
 * the AES part needs.
 *
 * 1. A free RC4 table (enc.adt, password "secret", made here with
 *    AdsEnableEncryption + AdsEncryptTable) is opened by one handle that
 *    gives the password, then by handles that do not (the same connection,
 *    another connection), give a wrong one, or open first; each reports what
 *    it reads and whether it may write.
 * 2. The dictionary's encrypted table rc4_secret, open through the
 *    dictionary, is then opened as a free table of the folder.
 * 3. A free AES table (aes.adt), as the RC4 one.
 * 4. A record-level RC4 table (rec.adt: the password enabled, record 1
 *    enciphered), read and written on both records by a handle without the
 *    password.
 *
 * Only the documented API is driven; nothing reads the engine's code.
 */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;

static HMODULE ace;
#define F(name) GetProcAddress(ace, name)
#define FN1(name) ((U32(WINAPI *)(H))F(name))

static int first = 1;
static void key(const char *k) {
    printf("%s\n \"%s\": ", first ? "" : ",", k);
    first = 0;
}
static void num(const char *k, long v) { key(k); printf("%ld", v); }
static void text(const char *k, const U8 *s, long n) {
    key(k);
    putchar('"');
    for (long i = 0; i < n; i++) {
        if (s[i] == '"' || s[i] == '\\') printf("\\%c", s[i]);
        else if (s[i] < 32 || s[i] > 126) printf("\\u%04x", s[i]);
        else putchar(s[i]);
    }
    putchar('"');
}

static const char *folder;

/* Opens `name`: a free table's file (with an extension) in the folder, by its
 * full path (under Wine the local server takes a relative name against the
 * folder's parent), or a dictionary's table by its name (ADS_DEFAULT). */
static U32 open_table(H conn, const char *name, U32 options, H *t) {
    char path[MAX_PATH];
    int free_table = strchr(name, '.') != NULL;
    if (free_table) snprintf(path, sizeof path, "%s\\%s", folder, name);
    else snprintf(path, sizeof path, "%s", name);
    *t = 0;
    U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
        conn, (U8 *)path, NULL, free_table ? 3 : 0, 1, 1, 1, options, t);
    if (rc) {
        U32 code = 0; U8 msg[600]; U16 n = sizeof msg;
        ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, msg, &n);
        fprintf(stderr, "open %s: %.*s\n", name, (int)n, msg);
    }
    return rc;
}

/* The record the reports and writes are on: the first, or `at_record`. */
static U32 at_record = 0;
static void go(H t) {
    if (at_record) ((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(t, at_record);
    else FN1("AdsGotoTop")(t);
}

/* What handle t reads and may do, as "<tag>_..." entries. */
static void report(const char *tag, H t, const char *field, const char *memo,
                   const char *index, const char *seek) {
    char k[96];
    U32 rc;
    U16 e = 0x55;
    ((U32(WINAPI *)(H, U16 *))F("AdsIsEncryptionEnabled"))(t, &e);
    snprintf(k, sizeof k, "%s_enabled", tag); num(k, e);
    U32 count = 0;
    rc = ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(t, 2, &count);
    snprintf(k, sizeof k, "%s_count_rc", tag); num(k, rc);
    snprintf(k, sizeof k, "%s_count", tag); num(k, count);
    go(t);
    U8 buf[256]; U32 len = sizeof buf;
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U32 *, U16))F("AdsGetField"))(
        t, (U8 *)field, buf, &len, 0);
    snprintf(k, sizeof k, "%s_field_rc", tag); num(k, rc);
    snprintf(k, sizeof k, "%s_field", tag); text(k, buf, rc ? 0 : (long)len);
    e = 0x55;
    ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsIsEmpty"))(t, (U8 *)field, &e);
    snprintf(k, sizeof k, "%s_field_empty", tag); num(k, e);
    if (strcmp(field, "D1") == 0) {
        len = sizeof buf;
        rc = ((U32(WINAPI *)(H, U8 *, U8 *, U32 *, U16))F("AdsGetField"))(
            t, (U8 *)"N1", buf, &len, 0);
        snprintf(k, sizeof k, "%s_n1_rc", tag); num(k, rc);
        snprintf(k, sizeof k, "%s_n1", tag); text(k, buf, rc ? 0 : (long)len);
    }
    if (memo) {
        len = sizeof buf;
        rc = ((U32(WINAPI *)(H, U8 *, U8 *, U32 *, U16))F("AdsGetField"))(
            t, (U8 *)memo, buf, &len, 0);
        snprintf(k, sizeof k, "%s_memo_rc", tag); num(k, rc);
        snprintf(k, sizeof k, "%s_memo", tag); text(k, buf, rc ? 0 : (long)len);
    }
    e = 0x55;
    ((U32(WINAPI *)(H, U16 *))F("AdsIsRecordEncrypted"))(t, &e);
    snprintf(k, sizeof k, "%s_record_encrypted", tag); num(k, e);
    if (index) {
        H order = 0;
        rc = ((U32(WINAPI *)(H, U8 *, H *))F("AdsGetIndexHandle"))(t, (U8 *)index, &order);
        snprintf(k, sizeof k, "%s_index_rc", tag); num(k, rc);
        if (!rc) {
            U16 found = 0x55;
            rc = ((U32(WINAPI *)(H, U8 *, U16, U16, U16, U16 *))F("AdsSeek"))(
                order, (U8 *)seek, (U16)strlen(seek), 2, 2, &found);
            snprintf(k, sizeof k, "%s_seek_rc", tag); num(k, rc);
            snprintf(k, sizeof k, "%s_seek_found", tag); num(k, found);
            U32 rec = 0;
            ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordNum"))(t, 2, &rec);
            snprintf(k, sizeof k, "%s_seek_record", tag); num(k, rec);
        }
    }
}

/* Whether handle t may write: an update of record 1 (the field set, then the
 * record written), an append, a deletion, and the record (de)ciphering. */
static void try_writes(const char *tag, H t, const char *field) {
    char k[96];
    go(t);
    U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetField"))(
        t, (U8 *)field, (U8 *)"zulu", 4);
    snprintf(k, sizeof k, "%s_setfield_rc", tag); num(k, rc);
    rc = FN1("AdsWriteRecord")(t);
    snprintf(k, sizeof k, "%s_write_rc", tag); num(k, rc);
    FN1("AdsCancelUpdate")(t);
    rc = FN1("AdsAppendRecord")(t);
    snprintf(k, sizeof k, "%s_append_rc", tag); num(k, rc);
    rc = FN1("AdsWriteRecord")(t);
    snprintf(k, sizeof k, "%s_append_write_rc", tag); num(k, rc);
    FN1("AdsCancelUpdate")(t);
    go(t);
    rc = FN1("AdsDeleteRecord")(t);
    snprintf(k, sizeof k, "%s_delete_rc", tag); num(k, rc);
    go(t);
    rc = FN1("AdsDecryptRecord")(t);
    snprintf(k, sizeof k, "%s_decryptrecord_rc", tag); num(k, rc);
    rc = FN1("AdsEncryptRecord")(t);
    snprintf(k, sizeof k, "%s_encryptrecord_rc", tag); num(k, rc);
}

static U32 enable(H t, const char *pw) {
    return ((U32(WINAPI *)(H, U8 *))F("AdsEnableEncryption"))(t, (U8 *)pw);
}

static U32 append(H t, const char *d1, const char *m1) {
    U32 rc = FN1("AdsAppendRecord")(t);
    if (!rc) rc = ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetField"))(
        t, (U8 *)"D1", (U8 *)d1, strlen(d1));
    if (!rc) rc = ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetField"))(
        t, (U8 *)"M1", (U8 *)m1, strlen(m1));
    if (!rc) rc = FN1("AdsWriteRecord")(t);
    return rc;
}

int main(int argc, char **argv) {
    if (argc < 3) return fprintf(stderr, "usage: share <dll> <folder>\n"), 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) return fprintf(stderr, "cannot load %s\n", argv[1]), 1;
    folder = argv[2];
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    printf("{");

    H c1 = 0, c2 = 0;
    char dir[MAX_PATH];
    snprintf(dir, sizeof dir, "%s\\", folder);
    num("connect1_rc", ((U32(WINAPI *)(U8 *, U16, H *))F("AdsConnect26"))((U8 *)dir, 1, &c1));
    num("connect2_rc", ((U32(WINAPI *)(U8 *, U16, H *))F("AdsConnect26"))((U8 *)dir, 1, &c2));

    /* The free RC4 table. */
    char path[MAX_PATH], adi[MAX_PATH];
    snprintf(path, sizeof path, "%s\\enc.adt", folder);
    snprintf(adi, sizeof adi, "%s\\enc.adi", folder);
    H t = 0, order = 0;
    U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, U32, H *))F(
        "AdsCreateTable71"))(c1, (U8 *)path, NULL, 3, 1, 1, 1, 0,
                             (U8 *)"D1,Character,20;N1,Integer;M1,Memo", 0, &t);
    num("create_rc", rc);
    ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, H *))F("AdsCreateIndex"))(
        t, (U8 *)adi, (U8 *)"IDX1", (U8 *)"D1", NULL, NULL, 0, &order);
    append(t, "alpha", "memo-alpha");
    append(t, "bravo", "memo-bravo");
    num("enable_rc", enable(t, "secret"));
    num("encrypt_rc", FN1("AdsEncryptTable")(t));
    FN1("AdsCloseTable")(t);

    /* 1. A handle with the password, then one without it on the same
     * connection, one on another connection, and one with a wrong one. */
    H h1, h2, h3, h4, h5;
    num("h1_open_rc", open_table(c1, "enc.adt", 4, &h1));
    num("h1_enable_rc", enable(h1, "secret"));
    report("h1", h1, "D1", "M1", "IDX1", "bravo");
    num("h2_open_rc", open_table(c1, "enc.adt", 4, &h2));
    report("h2", h2, "D1", "M1", "IDX1", "bravo");
    try_writes("h2", h2, "D1");
    num("h3_open_rc", open_table(c2, "enc.adt", 4, &h3));
    report("h3", h3, "D1", "M1", "IDX1", "bravo");
    try_writes("h3", h3, "D1");
    num("h4_open_rc", open_table(c2, "enc.adt", 4, &h4));
    num("h4_enable_rc", enable(h4, "wrong"));
    report("h4", h4, "D1", "M1", NULL, NULL);
    /* Another handle with the password; the first lets go of its own. */
    num("h5_open_rc", open_table(c2, "enc.adt", 4, &h5));
    num("h5_enable_rc", enable(h5, "secret"));
    num("h1_disable_rc", FN1("AdsDisableEncryption")(h1));
    report("h1_disabled", h1, "D1", "M1", NULL, NULL);
    report("h5", h5, "D1", "M1", NULL, NULL);
    FN1("AdsCloseTable")(h1); FN1("AdsCloseTable")(h2); FN1("AdsCloseTable")(h3);
    FN1("AdsCloseTable")(h4); FN1("AdsCloseTable")(h5);

    /* The handle without the password opened first. */
    H h6, h7;
    num("h6_open_rc", open_table(c1, "enc.adt", 4, &h6));
    num("h7_open_rc", open_table(c2, "enc.adt", 4, &h7));
    num("h7_enable_rc", enable(h7, "secret"));
    report("h6", h6, "D1", "M1", NULL, NULL);
    report("h7", h7, "D1", "M1", NULL, NULL);
    FN1("AdsCloseTable")(h6); FN1("AdsCloseTable")(h7);

    /* Exclusively, without the password: decrypting the table. */
    H h8;
    num("h8_open_rc", open_table(c1, "enc.adt", 1, &h8));
    num("h8_decrypttable_rc", FN1("AdsDecryptTable")(h8));
    num("h8_encrypttable_rc", FN1("AdsEncryptTable")(h8));
    report("h8", h8, "D1", "M1", NULL, NULL);
    FN1("AdsCloseTable")(h8);

    /* 2. Dictionary-bound encrypted tables, open through the dictionary,
     * then opened as free tables. */
    char add[MAX_PATH];
    H d1 = 0, b1, f1;
    snprintf(add, sizeof add, "%s\\rc4.add", folder);
    num("rc4dict_connect_rc", ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
        (U8 *)add, 1, (U8 *)"ADSSYS", (U8 *)"adssyspw", 0, &d1));
    num("rc4bound_open_rc", open_table(d1, "rc4_secret", 0, &b1));
    report("rc4bound", b1, "name", "note", NULL, NULL);
    num("rc4free_open_rc", open_table(c1, "rc4_secret.adt", 4, &f1));
    if (f1) {
        report("rc4free", f1, "name", "note", NULL, NULL);
        FN1("AdsCloseTable")(f1);
    }
    FN1("AdsCloseTable")(b1);
    num("rc4free_alone_open_rc", open_table(c1, "rc4_secret.adt", 4, &f1));
    if (f1) FN1("AdsCloseTable")(f1);

    /* 3. A free AES table (needs the OpenSSL DLLs beside the engine), made on
     * an AES connection; then opened as the RC4 one was. */
    char cs[MAX_PATH + 96];
    H c3 = 0, a1, a2, a3;
    snprintf(cs, sizeof cs, "Data Source=%s;ServerType=LOCAL;EncryptionType=AES128;FIPS=FALSE;",
             dir);
    num("aes_connect_rc", ((U32(WINAPI *)(U8 *, H *, H *))F("AdsConnect101"))((U8 *)cs, NULL, &c3));
    snprintf(path, sizeof path, "%s\\aes.adt", folder);
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, U32, H *))F(
        "AdsCreateTable71"))(c3, (U8 *)path, NULL, 3, 1, 1, 1, 0,
                             (U8 *)"D1,Character,20;N1,Integer;M1,Memo", 0, &t);
    num("aes_create_rc", rc);
    num("aes_enable_rc", enable(t, "secret"));
    num("aes_encrypt_rc", FN1("AdsEncryptTable")(t));
    append(t, "alpha", "memo-alpha");
    append(t, "bravo", "memo-bravo");
    FN1("AdsCloseTable")(t);
    num("a1_open_rc", open_table(c1, "aes.adt", 4, &a1));
    num("a1_enable_rc", enable(a1, "secret"));
    report("a1", a1, "D1", "M1", NULL, NULL);
    num("a2_open_rc", open_table(c2, "aes.adt", 4, &a2));
    if (a2) {
        report("a2", a2, "D1", "M1", NULL, NULL);
        try_writes("a2", a2, "D1");
        FN1("AdsCloseTable")(a2);
        /* What the handle with the password sees after those writes. */
        report("a1_after", a1, "D1", "M1", NULL, NULL);
    }
    num("a3_open_rc", open_table(c2, "aes.adt", 4, &a3));
    if (a3) {
        num("a3_enable_rc", enable(a3, "wrong"));
        report("a3", a3, "D1", "M1", NULL, NULL);
        FN1("AdsCloseTable")(a3);
    }
    FN1("AdsCloseTable")(a1);

    /* 4. Record-level RC4: the password enabled and record 1 enciphered, the
     * table not; a handle without the password on record 1 (enciphered) and
     * record 2 (clear). */
    H r1, r2;
    snprintf(path, sizeof path, "%s\\rec.adt", folder);
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, U32, H *))F(
        "AdsCreateTable71"))(c1, (U8 *)path, NULL, 3, 1, 1, 1, 0,
                             (U8 *)"D1,Character,20;N1,Integer;M1,Memo", 0, &t);
    num("rec_create_rc", rc);
    append(t, "alpha", "memo-alpha");
    append(t, "bravo", "memo-bravo");
    num("rec_enable_rc", enable(t, "secret"));
    FN1("AdsGotoTop")(t);
    num("rec_encryptrecord_rc", FN1("AdsEncryptRecord")(t));
    FN1("AdsCloseTable")(t);
    num("r1_open_rc", open_table(c1, "rec.adt", 4, &r1));
    num("r1_enable_rc", enable(r1, "secret"));
    num("r2_open_rc", open_table(c2, "rec.adt", 4, &r2));
    report("r2_rec1", r2, "D1", "M1", NULL, NULL);
    at_record = 2;
    report("r2_rec2", r2, "D1", "M1", NULL, NULL);
    try_writes("r2_rec2", r2, "D1");
    at_record = 1;
    try_writes("r2_rec1", r2, "D1");
    at_record = 0;
    report("r1_after", r1, "D1", "M1", NULL, NULL);
    at_record = 2;
    report("r1_after_rec2", r1, "D1", "M1", NULL, NULL);
    at_record = 0;
    FN1("AdsCloseTable")(r1);
    FN1("AdsCloseTable")(r2);

    printf("\n}\n");
    return 0;
}
