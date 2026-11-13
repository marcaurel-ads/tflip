/* Black-box probe: the RC4 encryption calls on a free DBF table (Clipper,
 * FoxPro, Visual FoxPro), through the documented API only.
 *
 *     dbfenc.exe <ace dll> <data folder> <password> <script>  > out.json
 *
 * The script is read line by line; the data folder is the free tables' folder
 * and is where the files are left (the probe's result, compared byte for byte
 * by the replay in crates/tflip-ace/src/dbf_encryption.rs).
 *
 *     create <type> <name> [nomemo]  AdsCreateTable71 of D1 Character(20), N1
 *                                    Numeric(8,0) and M1 Memo; <type> is 1 (NTX),
 *                                    2 (CDX) or 4 (VFP); the table is the current one
 *     open <name> [x]                AdsOpenTable (exclusive with x) of <name>.dbf
 *     close                          AdsCloseTable
 *     index <file> <tag> <expr>      AdsCreateIndex on the current table
 *     append <d1> <n> <memo>         a record (an empty memo is not set)
 *     set <recno> <d1> <memo>        goto and change D1 and M1 (an empty memo is not set)
 *     delete <recno> / recall <recno>
 *     enable / disable               AdsEnableEncryption (the password) / AdsDisableEncryption
 *     encrypt / decrypt              AdsEncryptTable / AdsDecryptTable
 *     encrec <recno> / decrec <recno>   AdsEncryptRecord / AdsDecryptRecord
 *     pack / zap / reindex           AdsPackTable / AdsZapTable / AdsReindex
 *     restructure <fields>           AdsRestructureTable (field list as AdsCreateTable's)
 *     state                          AdsIsEncryptionEnabled / AdsIsTableEncrypted / count
 *     read <recno>                   what D1, N1 and M1 read as, AdsIsRecordEncrypted
 *     seek <text>                    AdsSeek on the first order (found, recno)
 *     # ...                          a comment
 *
 * Every step reports its return code and, when it failed, AdsGetLastError's
 * code. Run it from a folder with the patched engine's DLLs. Only the
 * documented API is driven; nothing reads the engine's code.
 */
#include <windows.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;

static HMODULE ace;
#define F(name) GetProcAddress(ace, name)
#define FIELD(n) ((U8 *)(ULONG_PTR)(n))
#define CALL(ret, args, name) ((ret(WINAPI *) args)F(name))

static int first = 1;
static void key(const char *k, int step) {
    printf("%s\n \"%03d_%s\": ", first ? "" : ",", step, k);
    first = 0;
}
static void num(int step, const char *k, long v) { key(k, step); printf("%ld", v); }
static void text(int step, const char *k, const U8 *s, long n) {
    key(k, step);
    putchar('"');
    for (long i = 0; i < n; i++) {
        if (s[i] == '"' || s[i] == '\\') printf("\\%c", s[i]);
        else if (s[i] < 32 || s[i] > 126) printf("\\u%04x", s[i]);
        else putchar(s[i]);
    }
    putchar('"');
}
static U32 report(int step, U32 rc) {
    num(step, "rc", rc);
    if (rc) {
        U32 code = 0; U8 msg[600]; U16 len = sizeof msg;
        CALL(U32, (U32 *, U8 *, U16 *), "AdsGetLastError")(&code, msg, &len);
        num(step, "err", code);
    }
    return rc;
}

static H conn, table;
static const char *folder, *password;

static U32 set_text(H t, int field, const char *v) {
    return CALL(U32, (H, U8 *, U8 *, U32), "AdsSetField")(t, FIELD(field), (U8 *)v, strlen(v));
}
static U32 fill(H t, const char *d1, const char *n, const char *memo) {
    U32 rc = set_text(t, 1, d1);
    if (!rc) rc = set_text(t, 2, n);
    if (!rc && *memo) rc = set_text(t, 3, memo);
    return rc;
}

int main(int argc, char **argv) {
    if (argc < 5) return fprintf(stderr, "usage: dbfenc <dll> <folder> <pw> <script>\n"), 2;
    folder = argv[2]; password = argv[3];
    ace = LoadLibraryA(argv[1]);
    if (!ace) return fprintf(stderr, "cannot load %s\n", argv[1]), 1;
    CALL(U32, (U16), "AdsSetServerType")(1);
    FILE *script = fopen(argv[4], "r");
    if (!script) return fprintf(stderr, "cannot read %s\n", argv[4]), 1;
    printf("{");
    U32 rc = CALL(U32, (U8 *, U16, H *), "AdsConnect26")((U8 *)folder, 1, &conn);
    num(0, "connect_rc", rc);
    if (rc) return printf("\n}\n"), 1;

    char line[1024];
    int step = 0;
    while (fgets(line, sizeof line, script)) {
        line[strcspn(line, "\r\n")] = 0;
        if (!*line || line[0] == '#') continue;
        step++;
        char *w[8] = {0};
        int n = 0;
        char *p = line;
        /* words are separated by single blanks; values hold none */
        while (n < 7 && *p) {
            w[n++] = p;
            char *sp = strchr(p, ' ');
            if (!sp) break;
            *sp = 0;
            p = sp + 1;
        }
        char path[MAX_PATH];
        const char *cmd = w[0];
        if (!strcmp(cmd, "create")) {
            snprintf(path, sizeof path, "%s\\%s.dbf", folder, w[2]);
            int type = atoi(w[1]);
            const char *fields = w[3] && !strcmp(w[3], "nomemo")
                ? "D1,Character,20;N1,Numeric,8,0"
                : "D1,Character,20;N1,Numeric,8,0;M1,Memo";
            report(step, CALL(U32, (H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, U32, H *),
                "AdsCreateTable71")(conn, (U8 *)path, NULL, type, 1, 1, 1, 0, (U8 *)fields, 0, &table));
        } else if (!strcmp(cmd, "open")) {
            snprintf(path, sizeof path, "%s\\%s.dbf", folder, w[1]);
            U32 options = w[2] ? 0x0004 /* ADS_EXCLUSIVE */ : 0;
            report(step, CALL(U32, (H, U8 *, U8 *, U16, U16, U16, U16, U32, H *),
                "AdsOpenTable")(conn, (U8 *)path, NULL, 0, 1, 1, 0, options, &table));
        } else if (!strcmp(cmd, "close")) {
            report(step, CALL(U32, (H), "AdsCloseTable")(table));
        } else if (!strcmp(cmd, "index")) {
            snprintf(path, sizeof path, "%s\\%s", folder, w[1]);
            H idx = 0;
            report(step, CALL(U32, (H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, H *),
                "AdsCreateIndex")(table, (U8 *)path, (U8 *)w[2], (U8 *)w[3], NULL, NULL, 0, &idx));
        } else if (!strcmp(cmd, "append")) {
            U32 r = CALL(U32, (H), "AdsAppendRecord")(table);
            if (!r) r = fill(table, w[1], w[2], w[3] ? w[3] : "");
            if (!r) r = CALL(U32, (H), "AdsWriteRecord")(table);
            report(step, r);
        } else if (!strcmp(cmd, "set")) {
            U32 r = CALL(U32, (H, U32), "AdsGotoRecord")(table, atoi(w[1]));
            if (!r) r = fill(table, w[2], "1", w[3] ? w[3] : "");
            if (!r) r = CALL(U32, (H), "AdsWriteRecord")(table);
            report(step, r);
        } else if (!strcmp(cmd, "delete") || !strcmp(cmd, "recall")
                   || !strcmp(cmd, "encrec") || !strcmp(cmd, "decrec")) {
            U32 r = CALL(U32, (H, U32), "AdsGotoRecord")(table, atoi(w[1]));
            if (!r) r = CALL(U32, (H), !strcmp(cmd, "delete") ? "AdsDeleteRecord"
                : !strcmp(cmd, "recall") ? "AdsRecallRecord"
                : !strcmp(cmd, "encrec") ? "AdsEncryptRecord" : "AdsDecryptRecord")(table);
            report(step, r);
        } else if (!strcmp(cmd, "enable")) {
            report(step, CALL(U32, (H, U8 *), "AdsEnableEncryption")(table, (U8 *)password));
        } else if (!strcmp(cmd, "disable")) {
            report(step, CALL(U32, (H), "AdsDisableEncryption")(table));
        } else if (!strcmp(cmd, "encrypt")) {
            report(step, CALL(U32, (H), "AdsEncryptTable")(table));
        } else if (!strcmp(cmd, "decrypt")) {
            report(step, CALL(U32, (H), "AdsDecryptTable")(table));
        } else if (!strcmp(cmd, "pack")) {
            report(step, CALL(U32, (H), "AdsPackTable")(table));
        } else if (!strcmp(cmd, "zap")) {
            report(step, CALL(U32, (H), "AdsZapTable")(table));
        } else if (!strcmp(cmd, "reindex")) {
            report(step, CALL(U32, (H), "AdsReindex")(table));
        } else if (!strcmp(cmd, "restructure")) {
            report(step, CALL(U32, (H, U8 *, U8 *, U16, U16, U16, U16, U8 *, U32, U32),
                "AdsRestructureTable")(table, NULL, NULL, 0, 1, 1, 0, (U8 *)w[1], 0, 0));
        } else if (!strcmp(cmd, "state")) {
            U16 e;
            e = 0x55; CALL(U32, (H, U16 *), "AdsIsEncryptionEnabled")(table, &e); num(step, "enabled", e);
            e = 0x55; CALL(U32, (H, U16 *), "AdsIsTableEncrypted")(table, &e); num(step, "table_encrypted", e);
            U32 count = 0; CALL(U32, (H, U16, U32 *), "AdsGetRecordCount")(table, 2, &count);
            num(step, "count", count);
        } else if (!strcmp(cmd, "read")) {
            U32 r = CALL(U32, (H, U32), "AdsGotoRecord")(table, atoi(w[1]));
            num(step, "goto", r);
            if (!r) {
                const char *names[] = {"D1", "N1", "M1"};
                for (int i = 0; i < 3; i++) {
                    U8 buf[300]; U32 len = sizeof buf;
                    U32 g = CALL(U32, (H, U8 *, U8 *, U32 *, U16), "AdsGetField")(table, (U8 *)names[i], buf, &len, 0);
                    if (g) num(step, names[i], -(long)g); else text(step, names[i], buf, len);
                }
                U16 e = 0x55; CALL(U32, (H, U16 *), "AdsIsRecordEncrypted")(table, &e); num(step, "encrypted", e);
                U16 d = 0x55; CALL(U32, (H, U16 *), "AdsIsRecordDeleted")(table, &d); num(step, "deleted", d);
            }
        } else if (!strcmp(cmd, "seek")) {
            H order = 0;
            CALL(U32, (H, U16, H *), "AdsGetIndexHandleByOrder")(table, 1, &order);
            U16 found = 0;
            U32 s = CALL(U32, (H, U8 *, U16, U16, U16, U16 *), "AdsSeek")(order, (U8 *)w[1], strlen(w[1]), 2, 2, &found);
            num(step, "seek_rc", s); num(step, "found", found);
            U32 recno = 0; CALL(U32, (H, U16, U32 *), "AdsGetRecordNum")(table, 2, &recno);
            num(step, "recno", recno);
        } else {
            fprintf(stderr, "unknown step: %s\n", cmd);
        }
    }
    CALL(U32, (H), "AdsDisconnect")(conn);
    printf("\n}\n");
    return 0;
}
