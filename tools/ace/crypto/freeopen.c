/* Black-box probe: a dictionary's encrypted table opened as a free table.
 *
 *     freeopen.exe <ace dll> <data folder>  > freeopen.json
 *
 * The data folder must hold copies of fixtures/adt/dictionary_encrypted (rc4.add,
 * ADSSYS password "adssyspw") in rc4\, fixtures/aes/aes256 (dictionary
 * password "ddsecret", given as DDPassword; ADSSYS has none) in aes256\,
 * fixtures/aes/free in free\, and an empty mk\. Run it from a folder with
 * the patched engine's 64-bit DLLs, whose OpenSSL the AES tables need.
 *
 * Each table is opened by its file, without a dictionary: with
 * AdsOpenTable on a folder connection (shared, exclusive), without a
 * connection, on a folder connection given a password, with AdsOpenTable101,
 * and in SQL (SELECT, DELETE); then while its dictionary is only connected,
 * while it has the table open (also by the dictionary's connection, by the
 * file's path), and after it closed it. Every failure reports its code and
 * AdsGetLastError's text. The RC4 tables are also opened from a folder
 * without their dictionary (lone\). A dictionary that encrypts its
 * structure (AdsDDCreate with encryption) is made here with a CDX and an
 * ADT table encrypted in it, which are opened the same way.
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

/* Reports rc as "<tag>_rc" and, when it failed, the last error as
 * "<tag>_err" / "<tag>_text". */
static void result(const char *tag, U32 rc) {
    char k[128];
    snprintf(k, sizeof k, "%s_rc", tag); num(k, rc);
    if (!rc) return;
    U32 code = 0; U8 msg[1100]; U16 n = sizeof msg;
    ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, msg, &n);
    snprintf(k, sizeof k, "%s_err", tag); num(k, code);
    snprintf(k, sizeof k, "%s_text", tag); text(k, msg, n);
}

static const char *folder;

static U32 open_as(H conn, const char *path, U16 type, U32 options, H *t) {
    *t = 0;
    return ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
        conn, (U8 *)path, NULL, type, 1, 1, 1, options, t);
}

/* A field of the first record, when the table opened. */
static void peek(const char *tag, H t) {
    char k[128];
    U16 n = 0;
    ((U32(WINAPI *)(H, U16 *))F("AdsGetNumFields"))(t, &n);
    snprintf(k, sizeof k, "%s_fields", tag); num(k, n);
    U16 e = 0x55;
    ((U32(WINAPI *)(H, U16 *))F("AdsIsEncryptionEnabled"))(t, &e);
    snprintf(k, sizeof k, "%s_enabled", tag); num(k, e);
    e = 0x55;
    ((U32(WINAPI *)(H, U16 *))F("AdsIsTableEncrypted"))(t, &e);
    snprintf(k, sizeof k, "%s_encrypted", tag); num(k, e);
}

static void close_table(H t) { if (t) FN1("AdsCloseTable")(t); }

/* The ways of opening `file` (a path relative to the data folder) free. */
static void free_opens(const char *tag, const char *file, U16 type, H c, H cpw) {
    char path[MAX_PATH], k[128], sql[MAX_PATH + 64];
    snprintf(path, sizeof path, "%s\\%s", folder, file);
    H t;
    snprintf(k, sizeof k, "%s_shared", tag);
    result(k, open_as(c, path, type, 4, &t));
    if (t) { peek(k, t); close_table(t); }
    snprintf(k, sizeof k, "%s_exclusive", tag);
    result(k, open_as(c, path, type, 1, &t));
    close_table(t);
    snprintf(k, sizeof k, "%s_noconn", tag);
    result(k, open_as(0, path, type, 4, &t));
    close_table(t);
    if (cpw) {
        snprintf(k, sizeof k, "%s_pwconn", tag);
        result(k, open_as(cpw, path, type, 4, &t));
        if (t) { peek(k, t); close_table(t); }
    }
    if (type == 3) {
        snprintf(k, sizeof k, "%s_open101", tag);
        t = 0;
        result(k, ((U32(WINAPI *)(H, U8 *, H *))F("AdsOpenTable101"))(c, (U8 *)path, &t));
        close_table(t);
    }
    H stmt = 0;
    ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(c, &stmt);
    if (type != 3) ((U32(WINAPI *)(H, U16))F("AdsStmtSetTableType"))(stmt, type);
    snprintf(sql, sizeof sql, "SELECT * FROM \"%s\"", path);
    H cur = 0;
    snprintf(k, sizeof k, "%s_sql", tag);
    result(k, ((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(stmt, (U8 *)sql, &cur));
    close_table(cur);
    snprintf(sql, sizeof sql, "DELETE FROM \"%s\" WHERE 1 = 0", path);
    cur = 0;
    snprintf(k, sizeof k, "%s_sqldelete", tag);
    result(k, ((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(stmt, (U8 *)sql, &cur));
    close_table(cur);
    FN1("AdsCloseSQLStatement")(stmt);
}

static U32 connect60(const char *path, const char *user, const char *pw, H *c) {
    *c = 0;
    return ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
        (U8 *)path, 1, (U8 *)user, (U8 *)pw, 0, c);
}

/* A dictionary's table: free alone, then while the dictionary has it open
 * (and only connected). */
static void dict_table(const char *tag, const char *add, const char *pw, const char *extra,
                       const char *name, const char *file, U16 type, H c, H cpw) {
    char k[128], path[MAX_PATH], addp[MAX_PATH], cs[2 * MAX_PATH];
    free_opens(tag, file, type, c, cpw);
    snprintf(addp, sizeof addp, "%s\\%s", folder, add);
    snprintf(path, sizeof path, "%s\\%s", folder, file);
    H d = 0, b = 0, t;
    snprintf(k, sizeof k, "%s_dict_connect", tag);
    snprintf(cs, sizeof cs, "Data Source=%s;ServerType=LOCAL;User ID=ADSSYS;Password=%s;%s", addp,
             pw, extra);
    result(k, ((U32(WINAPI *)(U8 *, H *, H *))F("AdsConnect101"))((U8 *)cs, NULL, &d));
    snprintf(k, sizeof k, "%s_dictonly", tag);
    result(k, open_as(c, path, type, 4, &t));
    close_table(t);
    snprintf(k, sizeof k, "%s_bound", tag);
    result(k, open_as(d, name, 0, 4, &b));
    if (b) peek(k, b);
    snprintf(k, sizeof k, "%s_whilebound", tag);
    result(k, open_as(c, path, type, 4, &t));
    if (t) { peek(k, t); close_table(t); }
    snprintf(k, sizeof k, "%s_whilebound_noconn", tag);
    result(k, open_as(0, path, type, 4, &t));
    close_table(t);
    /* The dictionary's connection opening the file by its path. */
    snprintf(k, sizeof k, "%s_whilebound_dictpath", tag);
    result(k, open_as(d, path, type, 4, &t));
    close_table(t);
    H stmt = 0, cur = 0;
    char sql[MAX_PATH + 64];
    ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(c, &stmt);
    if (type != 3) ((U32(WINAPI *)(H, U16))F("AdsStmtSetTableType"))(stmt, type);
    snprintf(sql, sizeof sql, "SELECT * FROM \"%s\"", path);
    snprintf(k, sizeof k, "%s_whilebound_sql", tag);
    result(k, ((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(stmt, (U8 *)sql, &cur));
    close_table(cur);
    FN1("AdsCloseSQLStatement")(stmt);
    close_table(b);
    snprintf(k, sizeof k, "%s_afterbound", tag);
    result(k, open_as(c, path, type, 4, &t));
    close_table(t);
    /* AdsDisconnect(0) would close every connection. */
    if (d) FN1("AdsDisconnect")(d);
}

int main(int argc, char **argv) {
    if (argc < 3) return fprintf(stderr, "usage: freeopen <dll> <folder>\n"), 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) return fprintf(stderr, "cannot load %s\n", argv[1]), 1;
    folder = argv[2];
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    printf("{");

    char dir[MAX_PATH];
    snprintf(dir, sizeof dir, "%s\\", folder);
    H c = 0, cpw = 0, caes = 0, caespw = 0;
    result("connect", ((U32(WINAPI *)(U8 *, U16, H *))F("AdsConnect26"))((U8 *)dir, 1, &c));
    result("connect_pw", connect60(dir, NULL, "adssyspw", &cpw));
    /* Folder connections that allow AES (one with the AES dictionary's
     * password). */
    char cs[2 * MAX_PATH];
    const char *aes = "EncryptionType=AES256;FIPS=FALSE;DDPassword=ddsecret;";
    snprintf(cs, sizeof cs, "Data Source=%s;ServerType=LOCAL;%s", dir, aes);
    result("connect_aes", ((U32(WINAPI *)(U8 *, H *, H *))F("AdsConnect101"))((U8 *)cs, NULL, &caes));
    snprintf(cs, sizeof cs, "Data Source=%s;ServerType=LOCAL;Password=ddsecret;%s", dir, aes);
    result("connect_aespw",
           ((U32(WINAPI *)(U8 *, H *, H *))F("AdsConnect101"))((U8 *)cs, NULL, &caespw));

    /* The RC4 dictionary: rc4_secret (encrypted), rc4_memos (encrypted),
     * rc4_plain (not). */
    dict_table("rc4_secret", "rc4\\rc4.add", "adssyspw", "", "rc4_secret", "rc4\\rc4_secret.adt", 3,
               c, cpw);
    dict_table("rc4_memos", "rc4\\rc4.add", "adssyspw", "", "rc4_memos", "rc4\\rc4_memos.adt", 3,
               c, cpw);
    dict_table("rc4_plain", "rc4\\rc4.add", "adssyspw", "", "rc4_plain", "rc4\\rc4_plain.adt", 3,
               c, cpw);
    /* The AES dictionary, from a plain connection and an AES one. */
    dict_table("aes_t", "aes256\\aes256.add", "", aes, "t", "aes256\\t.adt", 3, c, caespw);
    dict_table("aesc_t", "aes256\\aes256.add", "", aes, "t", "aes256\\t.adt", 3, caes,
               caespw);
    dict_table("aesc_m", "aes256\\aes256.add", "", aes, "m", "aes256\\m.adt", 3, caes,
               caespw);
    /* Free AES tables. */
    free_opens("free_aes", "free\\free.adt", 3, c, 0);
    free_opens("free_aesc", "free\\free.adt", 3, caes, caespw);
    free_opens("free_unarmed", "free\\unarmed.adt", 3, caes, 0);

    /* The RC4 dictionary's tables copied to a folder without their
     * dictionary. */
    static const char *lone[] = {"rc4_plain.adt", "rc4_plain.adm", "rc4_secret.adt",
                                 "rc4_secret.adm", "rc4_secret.adi"};
    char from[MAX_PATH], to[MAX_PATH];
    snprintf(to, sizeof to, "%s\\lone", folder);
    CreateDirectoryA(to, NULL);
    for (int i = 0; i < 5; i++) {
        snprintf(from, sizeof from, "%s\\rc4\\%s", folder, lone[i]);
        snprintf(to, sizeof to, "%s\\lone\\%s", folder, lone[i]);
        CopyFileA(from, to, FALSE);
    }
    free_opens("lone_plain", "lone\\rc4_plain.adt", 3, c, cpw);
    free_opens("lone_secret", "lone\\rc4_secret.adt", 3, c, cpw);

    /* A dictionary made here, encrypted, with an encrypted CDX and ADT table. */
    char add[MAX_PATH];
    snprintf(add, sizeof add, "%s\\mk\\mk.add", folder);
    H d = 0, t = 0;
    result("mk_create", ((U32(WINAPI *)(U8 *, U16, U8 *, H *))F("AdsDDCreate"))((U8 *)add, 1, NULL,
                                                                               &d));
    result("mk_password", ((U32(WINAPI *)(H, U16, void *, U16))F("AdsDDSetDatabaseProperty"))(
                              d, 105, (void *)"tablepw", 8));
    U16 on = 1;
    result("mk_encrypt_new", ((U32(WINAPI *)(H, U16, void *, U16))F("AdsDDSetDatabaseProperty"))(
                                 d, 106, &on, 2));
    static const struct { const char *name; U16 type; } made[] = {{"mcdx", 2}, {"madt", 3}};
    for (int i = 0; i < 2; i++) {
        char k[64];
        t = 0;
        snprintf(k, sizeof k, "%s_create", made[i].name);
        result(k, ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, U32, H *))F(
                       "AdsCreateTable71"))(d, (U8 *)made[i].name, NULL, made[i].type, 1, 1, 1,
                                            0, (U8 *)"D1,Character,20;N1,Integer;M1,Memo", 0,
                                            &t));
        FN1("AdsAppendRecord")(t);
        ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetField"))(t, (U8 *)"D1", (U8 *)"alpha", 5);
        FN1("AdsWriteRecord")(t);
        U16 e = 0x55;
        ((U32(WINAPI *)(H, U16 *))F("AdsIsTableEncrypted"))(t, &e);
        snprintf(k, sizeof k, "%s_encrypted", made[i].name); num(k, e);
        if (!e) {
            snprintf(k, sizeof k, "%s_encrypttable", made[i].name);
            result(k, FN1("AdsEncryptTable")(t));
        }
        close_table(t);
    }
    if (d) FN1("AdsDisconnect")(d);
    dict_table("mcdx", "mk\\mk.add", "", "", "mcdx", "mk\\mcdx.dbf", 2, c, cpw);
    dict_table("madt", "mk\\mk.add", "", "", "madt", "mk\\madt.adt", 3, c, cpw);

    printf("\n}\n");
    return 0;
}
