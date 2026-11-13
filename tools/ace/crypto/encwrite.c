/* Black-box probe of the Advantage RC4 encryption WRITE lifecycle.
 *
 *     encwrite.exe <ace64.dll> <data folder> <password> <stage>  > out.json
 *
 * Each stage creates its own fresh table (enc_<stage>.adt + .adm memo), drives
 * the documented encryption API up to that point, closes, and dumps the raw
 * on-disk files so the byte-level effect of each call can be diffed.  It also
 * reports what the query functions return.  No disassembly: documented API
 * and the files the engine wrote only.
 *
 * Fields: D1 Character(20), N1 Integer, M1 Memo.  Three records appended with
 * known content (so ciphertext XOR plaintext reveals the keystream).
 */
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

static int first = 1;
static void hex(const char *key, const U8 *bytes, long n) {
    printf("%s\"%s\": \"", first ? "" : ", ", key);
    for (long i = 0; i < n; i++) printf("%02x", bytes[i]);
    printf("\"");
    first = 0;
}
static void num(const char *key, long v) {
    printf("%s\"%s\": %ld", first ? "" : ", ", key, v);
    first = 0;
}
static void str(const char *key, const char *s) {
    printf("%s\"%s\": \"", first ? "" : ", ", key);
    for (; *s; s++) {
        if (*s == '"' || *s == '\\') printf("\\%c", *s);
        else if ((U8)*s < 32 || (U8)*s > 126) printf("\\u%04x", (U8)*s);
        else putchar(*s);
    }
    printf("\"");
    first = 0;
}
static void last_error(const char *key) {
    U32 code = 0; U8 text[600]; U16 len = sizeof text;
    ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
    num(key, code);
}
static long read_file(const char *path, U8 *buf, long max) {
    FILE *f = fopen(path, "rb");
    if (!f) return -1;
    long n = (long)fread(buf, 1, max, f);
    fclose(f);
    return n;
}
static U8 filebuf[1 << 16];
static void dump_file(const char *key, const char *path) {
    long n = read_file(path, filebuf, sizeof filebuf);
    char k[64]; snprintf(k, sizeof k, "%s_len", key); num(k, n);
    hex(key, filebuf, n < 0 ? 0 : n);
}

typedef U32(WINAPI *OpenTable)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *);

/* Append one record: D1=name (space padded in engine), N1=n, M1=memo text. */
static U32 append_record(H t, const char *d1, int n, const char *memo) {
    U32 rc = ((U32(WINAPI *)(H))F("AdsAppendRecord"))(t);
    if (!rc) rc = ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetField"))(t, FIELD(1), (U8 *)d1, strlen(d1));
    if (!rc) { char nb[16]; snprintf(nb, sizeof nb, "%d", n);
        rc = ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetField"))(t, FIELD(2), (U8 *)nb, strlen(nb)); }
    if (!rc && memo && *memo) rc = ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetField"))(t, FIELD(3), (U8 *)memo, strlen(memo));
    if (!rc) rc = ((U32(WINAPI *)(H))F("AdsWriteRecord"))(t);
    return rc;
}

int main(int argc, char **argv) {
    if (argc < 5) { fprintf(stderr, "usage: encwrite <dll> <folder> <pw> <stage>\n"); return 2; }
    const char *folder = argv[2], *password = argv[3], *stage = argv[4];
    ace = LoadLibraryA(argv[1]);
    if (!ace) return fprintf(stderr, "cannot load %s\n", argv[1]), 1;
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);

    char file[MAX_PATH], path[MAX_PATH], adm[MAX_PATH], adi[MAX_PATH];
    snprintf(file, sizeof file, "enc_%s.adt", stage);
    snprintf(path, sizeof path, "%s\\%s", folder, file);
    snprintf(adm, sizeof adm, "%s\\enc_%s.adm", folder, stage);
    snprintf(adi, sizeof adi, "%s\\enc_%s.adi", folder, stage);

    printf("{");
    str("stage", stage); str("password", password);

    H conn = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, H *))F("AdsConnect26"))((U8 *)folder, 1, &conn);
    num("connect_rc", rc);
    if (rc) return printf("}\n"), 1;

    H t = 0;
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, U32, H *))F("AdsCreateTable71"))(
        conn, (U8 *)path, NULL, 3, 1, 1, 1, 0,
        (U8 *)"D1,Character,20;N1,Integer;M1,Memo", 0, &t);
    num("create_rc", rc);
    if (rc) { last_error("create_err"); return printf("}\n"), 1; }

    /* An index on D1, to see whether/how the index file is encrypted. */
    H idx = 0;
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, H *))F("AdsCreateIndex"))(
        t, (U8 *)adi, (U8 *)"IDX1", (U8 *)"D1", NULL, NULL, 0, &idx);
    num("createindex_rc", rc);

    num("append0_rc", append_record(t, "alpha", 111, "memo-alpha"));
    num("append1_rc", append_record(t, "bravo", 222, "memo-bravo-longer-value"));
    num("append2_rc", append_record(t, "gamma", 333, ""));
    num("append_extra_rc", append_record(t, "epsilon", 555, "memo-eps"));
    /* For the 'deleted' stages: delete record 4 so a deleted slot is present. */
    if (strncmp(stage, "del", 3) == 0) {
        ((U32(WINAPI *)(H))F("AdsGotoBottom"))(t);
        rc = ((U32(WINAPI *)(H))F("AdsDeleteRecord"))(t);
        num("delete_rc", rc);
    }

    /* ---- the staged encryption calls ---- */
    int did_enable = 0;
    if (strcmp(stage, "base") != 0) {
        rc = ((U32(WINAPI *)(H, U8 *))F("AdsEnableEncryption"))(t, (U8 *)password);
        num("enable_rc", rc); if (rc) last_error("enable_err"); else did_enable = 1;
    }
    if (strcmp(stage, "encrec") == 0 || strcmp(stage, "decrec") == 0) {
        ((U32(WINAPI *)(H))F("AdsGotoTop"))(t);
        rc = ((U32(WINAPI *)(H))F("AdsEncryptRecord"))(t);
        num("encryptrecord_rc", rc); if (rc) last_error("encryptrecord_err");
        U16 e = 0x55;
        ((U32(WINAPI *)(H, U16 *))F("AdsIsRecordEncrypted"))(t, &e); num("rec0_encrypted", e);
        if (strcmp(stage, "decrec") == 0) {
            rc = ((U32(WINAPI *)(H))F("AdsDecryptRecord"))(t);
            num("decryptrecord_rc", rc); if (rc) last_error("decryptrecord_err");
        }
    }
    if (strcmp(stage, "enctable") == 0 || strcmp(stage, "enctable_append") == 0
        || strcmp(stage, "dectable") == 0 || strcmp(stage, "disable") == 0
        || strcmp(stage, "deltab") == 0) {
        rc = ((U32(WINAPI *)(H))F("AdsEncryptTable"))(t);
        num("encrypttable_rc", rc); if (rc) last_error("encrypttable_err");
    }
    if (strcmp(stage, "enctable_append") == 0) {
        num("append3_rc", append_record(t, "delta", 444, "memo-delta"));
    }
    if (strcmp(stage, "dectable") == 0) {
        rc = ((U32(WINAPI *)(H))F("AdsDecryptTable"))(t);
        num("decrypttable_rc", rc); if (rc) last_error("decrypttable_err");
    }
    if (strcmp(stage, "disable") == 0) {
        rc = ((U32(WINAPI *)(H))F("AdsDisableEncryption"))(t);
        num("disable_rc", rc); if (rc) last_error("disable_err");
    }

    /* Query state before closing. */
    if (did_enable || strcmp(stage, "base") != 0) {
        U16 e;
        e = 0x55; ((U32(WINAPI *)(H, U16 *))F("AdsIsEncryptionEnabled"))(t, &e); num("is_enc_enabled", e);
        e = 0x55; ((U32(WINAPI *)(H, U16 *))F("AdsIsTableEncrypted"))(t, &e); num("is_table_encrypted", e);
        e = 0x55; ((U32(WINAPI *)(H, U16 *))F("AdsVerifyPassword"))(t, &e); num("verify_password", e);
    }

    if (idx) ((U32(WINAPI *)(H))F("AdsCloseTable"))(t); else ((U32(WINAPI *)(H))F("AdsCloseTable"))(t);
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);

    dump_file("adt", path);
    dump_file("adm", adm);
    dump_file("adi", adi);

    printf("}\n");
    return 0;
}
