/* Black-box probe: what handles do on AES tables, driven by a script.
 *
 *     aesapi.exe <ace dll> <data folder> <script>  > aesapi.json
 *
 * Run it from a folder with the patched engine's DLLs (the OpenSSL ones AES
 * needs). Objects (connections, tables, orders) have names the script
 * gives them; "%D" in a line is the data folder. The script is read line by
 * line:
 *
 *     connect <name> <connect string>      AdsConnect101
 *     create <name> <conn> <file> <fields> AdsCreateTable71 (ADT, the fields as "N,Type,len;...")
 *     open <name> <conn> <file> <options>  AdsOpenTable (ADT; options 4 shared, 1 exclusive)
 *     openn <name> <conn> <table> <options>  AdsOpenTable by name (a dictionary's)
 *     close <name> | disconnect <name>
 *     enable <name> <password> | encrypt <name> | decrypt <name>
 *     index <name> <table> <file> <tag> <options> <expr>   AdsCreateIndex (name the order)
 *     order <name> <table> <tag>           AdsGetIndexHandle
 *     sql <conn> <statement>               AdsExecuteSQLDirect (its rows are listed)
 *     sqlc <conn> <statement>              the same, with the number of rows only
 *     append|write|cancel|delete|recall|top|bottom|pack|zap|reindex|dropindex|tablecheck <name>
 *     goto <name> <record> | skip <name> <n> | set <name> <field> <text>
 *     setbin <name> <field> <type> <text> | setempty <name> <field>
 *     seek <order> <key> <soft 1|0> | seekd <order> <double> <soft 1|0> | keycount|keynum|keylen|extract|recno|eof|count|opts|reclen <name>
 *     dump <name>                          every field of the current record, and its bytes
 *     setrecord <name> <hex>               AdsSetRecord with these bytes
 *     # ...                                a comment
 *
 * Every step reports its return code and, when it failed, AdsGetLastError's
 * code and text.
 *
 * Only the documented API is driven; nothing reads the engine's code.
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
static void hex(const char *k, const U8 *s, long n) {
    key(k);
    putchar('"');
    for (long i = 0; i < n; i++) printf("%02x", s[i]);
    putchar('"');
}

static char tag[64];
static void result(U32 rc) {
    char k[128];
    snprintf(k, sizeof k, "%s_rc", tag); num(k, rc);
    if (!rc) return;
    U32 code = 0; U8 msg[1100]; U16 n = sizeof msg;
    ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, msg, &n);
    snprintf(k, sizeof k, "%s_err", tag); num(k, code);
    snprintf(k, sizeof k, "%s_text", tag); text(k, msg, n);
}
static void field(const char *what, long v) {
    char k[160];
    snprintf(k, sizeof k, "%s_%s", tag, what);
    num(k, v);
}

#define MAXOBJ 64
static struct { char name[32]; H h; H table; } objs[MAXOBJ];
static int nobjs;

static H lookup(const char *name) {
    for (int i = 0; i < nobjs; i++) if (!strcmp(objs[i].name, name)) return objs[i].h;
    fprintf(stderr, "no object %s\n", name);
    return 0;
}
/* The table an order belongs to (the object itself for a table). */
static H table_of(const char *name) {
    for (int i = 0; i < nobjs; i++) if (!strcmp(objs[i].name, name)) return objs[i].table;
    return 0;
}
static void remember(const char *name, H h, H table) {
    for (int i = 0; i < nobjs; i++)
        if (!strcmp(objs[i].name, name)) { objs[i].h = h; objs[i].table = table; return; }
    snprintf(objs[nobjs].name, sizeof objs[nobjs].name, "%s", name);
    objs[nobjs].h = h;
    objs[nobjs].table = table;
    nobjs++;
}

/* `line` with every "%D" replaced by `folder`. */
static void expand(char *out, size_t size, const char *line, const char *folder) {
    size_t o = 0;
    for (const char *p = line; *p && o + 1 < size; p++) {
        if (p[0] == '%' && p[1] == 'D') {
            o += snprintf(out + o, size - o, "%s", folder);
            p++;
        } else {
            out[o++] = *p;
        }
    }
    out[o < size ? o : size - 1] = 0;
}

/* The next space-separated word of *rest (which moves past it). */
static char *word(char **rest) {
    char *w = *rest;
    char *sp = strchr(w, ' ');
    if (sp) { *sp = 0; *rest = sp + 1; } else *rest = w + strlen(w);
    return w;
}

static void dump(H t) {
    U16 n = 0;
    ((U32(WINAPI *)(H, U16 *))F("AdsGetNumFields"))(t, &n);
    U16 e = 0x55;
    ((U32(WINAPI *)(H, U16 *))F("AdsIsEncryptionEnabled"))(t, &e);
    field("enabled", e);
    e = 0x55;
    ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(t, &e);
    field("eof", e);
    U32 rec = 0;
    ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordNum"))(t, 1, &rec);
    field("recno", rec);
    e = 0x55;
    ((U32(WINAPI *)(H, U16 *))F("AdsIsRecordDeleted"))(t, &e);
    field("deleted", e);
    for (U16 f = 1; f <= n; f++) {
        U8 name[128], buf[4096];
        U16 nl = sizeof name, type = 0;
        ((U32(WINAPI *)(H, U16, U8 *, U16 *))F("AdsGetFieldName"))(t, f, name, &nl);
        name[nl] = 0;
        ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsGetFieldType"))(t, name, &type);
        U32 len = sizeof buf;
        U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U32 *, U16))F("AdsGetField"))(t, name, buf, &len, 0);
        char k[200];
        snprintf(k, sizeof k, "%s_%s_type", tag, name); num(k, type);
        snprintf(k, sizeof k, "%s_%s_rc", tag, name); num(k, rc);
        if (!rc) {
            snprintf(k, sizeof k, "%s_%s", tag, name);
            hex(k, buf, len);
        } else {
            U32 code = 0; U8 msg[300]; U16 ml = sizeof msg;
            ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, msg, &ml);
            snprintf(k, sizeof k, "%s_%s_err", tag, name); num(k, code);
        }
        e = 0x55;
        ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsIsEmpty"))(t, name, &e);
        snprintf(k, sizeof k, "%s_%s_empty", tag, name); num(k, e);
        U32 ml = 0xdead;
        rc = ((U32(WINAPI *)(H, U8 *, U32 *))F("AdsGetFieldLength"))(t, name, &ml);
        snprintf(k, sizeof k, "%s_%s_len", tag, name); num(k, rc ? -1 : (long)ml);
        ml = 0xdead;
        rc = ((U32(WINAPI *)(H, U8 *, U32 *))F("AdsGetFieldOffset"))(t, name, &ml);
        snprintf(k, sizeof k, "%s_%s_off", tag, name); num(k, rc ? -1 : (long)ml);
        if (type == 5 || type == 6 || type == 7 || type == 28) {
            ml = 0xdead;
            rc = ((U32(WINAPI *)(H, U8 *, U32 *))F("AdsGetMemoLength"))(t, name, &ml);
            snprintf(k, sizeof k, "%s_%s_memolen", tag, name); num(k, rc ? -1 : (long)ml);
            U8 big[4096];
            ml = sizeof big;
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U32 *, U16))F("AdsGetString"))(t, name, big, &ml, 1);
            snprintf(k, sizeof k, "%s_%s_string_rc", tag, name); num(k, rc);
            if (!rc) { snprintf(k, sizeof k, "%s_%s_string", tag, name); hex(k, big, ml); }
        }
    }
    U8 rb[4096];
    U32 rl = sizeof rb;
    U32 rc = ((U32(WINAPI *)(H, U8 *, U32 *))F("AdsGetRecord"))(t, rb, &rl);
    field("getrecord_rc", rc);
    if (!rc) {
        char k[200];
        snprintf(k, sizeof k, "%s_record", tag);
        hex(k, rb, rl);
    }
}

int main(int argc, char **argv) {
    if (argc < 4) return fprintf(stderr, "usage: aesapi <dll> <folder> <script>\n"), 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) return fprintf(stderr, "cannot load %s\n", argv[1]), 1;
    FILE *script = fopen(argv[3], "r");
    if (!script) return fprintf(stderr, "cannot read %s\n", argv[3]), 1;
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    printf("{");
    char line[32768], expanded[65536];
    int step = 0;
    while (fgets(line, sizeof line, script)) {
        line[strcspn(line, "\r\n")] = 0;
        if (!line[0] || line[0] == '#') continue;
        expand(expanded, sizeof expanded, line, argv[2]);
        snprintf(tag, sizeof tag, "%03d", ++step);
        char k[80];
        snprintf(k, sizeof k, "%s_cmd", tag);
        text(k, (U8 *)line, strlen(line));
        char *rest = expanded;
        char *cmd = word(&rest);
        char *a = word(&rest);
        H h = lookup(a), t = table_of(a);
        if (!strcmp(cmd, "connect")) {
            H c = 0;
            result(((U32(WINAPI *)(U8 *, H *, H *))F("AdsConnect101"))((U8 *)rest, NULL, &c));
            remember(a, c, 0);
        } else if (!strcmp(cmd, "disconnect")) {
            result(FN1("AdsDisconnect")(h));
        } else if (!strcmp(cmd, "create")) {
            char *conn = word(&rest), *file = word(&rest);
            H table = 0;
            result(((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, U32, H *))F(
                "AdsCreateTable71"))(lookup(conn), (U8 *)file, NULL, 3, 1, 1, 1, 0, (U8 *)rest, 0,
                                    &table));
            remember(a, table, table);
        } else if (!strcmp(cmd, "open") || !strcmp(cmd, "openn")) {
            char *conn = word(&rest), *file = word(&rest);
            U32 options = (U32)atoi(rest);
            H table = 0;
            result(((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
                lookup(conn), (U8 *)file, NULL, !strcmp(cmd, "open") ? 3 : 0, 1, 1, 1, options,
                &table));
            remember(a, table, table);
        } else if (!strcmp(cmd, "close")) {
            result(FN1("AdsCloseTable")(h));
        } else if (!strcmp(cmd, "enable")) {
            result(((U32(WINAPI *)(H, U8 *))F("AdsEnableEncryption"))(h, (U8 *)rest));
        } else if (!strcmp(cmd, "encrypt")) {
            result(FN1("AdsEncryptTable")(h));
        } else if (!strcmp(cmd, "decrypt")) {
            result(FN1("AdsDecryptTable")(h));
        } else if (!strcmp(cmd, "index")) {
            /* index <name> <table> <file> <tag> <options> <expr> */
            char *table = word(&rest), *file = word(&rest), *itag = word(&rest);
            U32 options = (U32)strtoul(word(&rest), NULL, 10);
            H order = 0;
            result(((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, H *))F("AdsCreateIndex"))(
                lookup(table), (U8 *)file, (U8 *)itag, (U8 *)rest, NULL, NULL, options, &order));
            remember(a, order, lookup(table));
        } else if (!strcmp(cmd, "order")) {
            char *table = word(&rest);
            H order = 0;
            result(((U32(WINAPI *)(H, U8 *, H *))F("AdsGetIndexHandle"))(lookup(table), (U8 *)rest,
                                                                         &order));
            remember(a, order, lookup(table));
        } else if (!strcmp(cmd, "sql")) {
            H stmt = 0, cur = 0;
            ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(h, &stmt);
            U32 rc = ((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(stmt, (U8 *)rest, &cur);
            result(rc);
            if (!rc && cur) {
                U16 ty = 0;
                ((U32(WINAPI *)(H, U16 *))F("AdsGetTableType"))(cur, &ty);
                field("tabletype", ty);
                U16 nf = 0;
                ((U32(WINAPI *)(H, U16 *))F("AdsGetNumFields"))(cur, &nf);
                char rows[20000];
                size_t o = 0;
                ((U32(WINAPI *)(H))F("AdsGotoTop"))(cur);
                for (;;) {
                    U16 eof = 0;
                    ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(cur, &eof);
                    if (eof) break;
                    for (U16 f = 1; f <= nf && o + 600 < sizeof rows; f++) {
                        U8 name[128], value[512];
                        U16 nl = sizeof name;
                        U32 vl = sizeof value - 1;
                        ((U32(WINAPI *)(H, U16, U8 *, U16 *))F("AdsGetFieldName"))(cur, f, name, &nl);
                        name[nl] = 0;
                        U32 r = ((U32(WINAPI *)(H, U8 *, U8 *, U32 *, U16))F("AdsGetString"))(
                            cur, name, value, &vl, 1);
                        value[r ? 0 : vl] = 0;
                        o += snprintf(rows + o, sizeof rows - o, "%s=%s;", name, value);
                    }
                    rows[o++] = '|';
                    ((U32(WINAPI *)(H, long))F("AdsSkip"))(cur, 1);
                }
                rows[o] = 0;
                snprintf(k, sizeof k, "%s_rows", tag);
                text(k, (U8 *)rows, (long)o);
                /* The cursor's table: encrypted, and its file. */
                U16 en = 0x55;
                ((U32(WINAPI *)(H, U16 *))F("AdsIsTableEncrypted"))(cur, &en);
                field("encrypted", en);
                U32 rl = 0;
                ((U32(WINAPI *)(H, U32 *))F("AdsGetRecordLength"))(cur, &rl);
                field("length", rl);
                FN1("AdsCloseTable")(cur);
            }
            FN1("AdsCloseSQLStatement")(stmt);
        } else if (!strcmp(cmd, "sqlc")) {
            /* As sql, but only the number of rows: those of the query log hold times. */
            H stmt = 0, cur = 0;
            ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(h, &stmt);
            U32 rc = ((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(stmt, (U8 *)rest, &cur);
            result(rc);
            if (!rc && cur) {
                U32 n = 0;
                ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(cur, 2, &n);
                field("count", n);
                FN1("AdsCloseTable")(cur);
            }
            FN1("AdsCloseSQLStatement")(stmt);
        } else if (!strcmp(cmd, "append")) {
            result(FN1("AdsAppendRecord")(h));
        } else if (!strcmp(cmd, "write")) {
            result(FN1("AdsWriteRecord")(h));
        } else if (!strcmp(cmd, "delete")) {
            result(FN1("AdsDeleteRecord")(h));
        } else if (!strcmp(cmd, "cancel")) {
            result(FN1("AdsCancelUpdate")(h));
        } else if (!strcmp(cmd, "recall")) {
            result(FN1("AdsRecallRecord")(h));
        } else if (!strcmp(cmd, "top")) {
            result(FN1("AdsGotoTop")(h));
        } else if (!strcmp(cmd, "bottom")) {
            result(FN1("AdsGotoBottom")(h));
        } else if (!strcmp(cmd, "pack")) {
            result(FN1("AdsPackTable")(h));
        } else if (!strcmp(cmd, "zap")) {
            result(FN1("AdsZapTable")(h));
        } else if (!strcmp(cmd, "reindex")) {
            result(FN1("AdsReindex")(h));
        } else if (!strcmp(cmd, "dropindex")) {
            result(FN1("AdsDeleteIndex")(h));
        } else if (!strcmp(cmd, "goto")) {
            result(((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(t, (U32)atoi(rest)));
        } else if (!strcmp(cmd, "skip")) {
            result(((U32(WINAPI *)(H, long))F("AdsSkip"))(h, atoi(rest)));
        } else if (!strcmp(cmd, "set")) {
            char *name = word(&rest);
            result(((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetField"))(t, (U8 *)name, (U8 *)rest,
                                                                          (U32)strlen(rest)));
        } else if (!strcmp(cmd, "setbin")) {
            char *name = word(&rest);
            U16 type = (U16)atoi(word(&rest));
            result(((U32(WINAPI *)(H, U8 *, U16, U32, U32, U8 *, U32))F("AdsSetBinary"))(
                t, (U8 *)name, type, (U32)strlen(rest), 0, (U8 *)rest, (U32)strlen(rest)));
        } else if (!strcmp(cmd, "setempty")) {
            result(((U32(WINAPI *)(H, U8 *))F("AdsSetEmpty"))(t, (U8 *)rest));
        } else if (!strcmp(cmd, "setrecord")) {
            U8 bytes[4096];
            U32 n = 0;
            for (; rest[0] && rest[1]; rest += 2) {
                char pair[3] = {rest[0], rest[1], 0};
                bytes[n++] = (U8)strtoul(pair, NULL, 16);
            }
            result(((U32(WINAPI *)(H, U8 *, U32))F("AdsSetRecord"))(t, bytes, n));
        } else if (!strcmp(cmd, "seek")) {
            char *keytext = word(&rest);
            U16 soft = (U16)atoi(rest), found = 0x55;
            result(((U32(WINAPI *)(H, U8 *, U16, U16, U16, U16 *))F("AdsSeek"))(
                h, (U8 *)keytext, (U16)strlen(keytext), 2, soft ? 1 : 2, &found));
            field("found", found);
            U32 rec = 0;
            ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordNum"))(t, 1, &rec);
            field("recno", rec);
        } else if (!strcmp(cmd, "seekd")) {
            double d = atof(word(&rest));
            U16 soft = (U16)atoi(rest), found = 0x55;
            result(((U32(WINAPI *)(H, U8 *, U16, U16, U16, U16 *))F("AdsSeek"))(
                h, (U8 *)&d, 8, 4, soft ? 1 : 2, &found));
            field("found", found);
            U32 rec = 0;
            ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordNum"))(t, 1, &rec);
            field("recno", rec);
        } else if (!strcmp(cmd, "keycount")) {
            U32 n = 0;
            result(((U32(WINAPI *)(H, U16, U32 *))F("AdsGetKeyCount"))(h, 1, &n));
            field("count", n);
        } else if (!strcmp(cmd, "keynum")) {
            U32 n = 0;
            result(((U32(WINAPI *)(H, U16, U32 *))F("AdsGetKeyNum"))(h, 1, &n));
            field("num", n);
        } else if (!strcmp(cmd, "keylen")) {
            U16 n = 0;
            result(((U32(WINAPI *)(H, U16 *))F("AdsGetKeyLength"))(h, &n));
            field("len", n);
        } else if (!strcmp(cmd, "extract")) {
            U8 buf[512];
            U16 n = sizeof buf;
            U32 rc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsExtractKey"))(h, buf, &n);
            result(rc);
            if (!rc) { snprintf(k, sizeof k, "%s_key", tag); hex(k, buf, n); }
        } else if (!strcmp(cmd, "recno")) {
            U32 n = 0;
            result(((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordNum"))(t, 1, &n));
            field("recno", n);
            U16 e = 0x55;
            ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(t, &e);
            field("eof", e);
        } else if (!strcmp(cmd, "eof")) {
            U16 e = 0x55;
            result(((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(t, &e));
            field("eof", e);
        } else if (!strcmp(cmd, "count")) {
            U32 n = 0;
            result(((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(t, 2, &n));
            field("count", n);
        } else if (!strcmp(cmd, "dump")) {
            dump(t);
        } else if (!strcmp(cmd, "reclen")) {
            U32 n = 0;
            result(((U32(WINAPI *)(H, U32 *))F("AdsGetRecordLength"))(t, &n));
            field("length", n);
        } else if (!strcmp(cmd, "opts")) {
            U32 o = 0;
            result(((U32(WINAPI *)(H, U32 *))F("AdsGetTableOpenOptions"))(t, &o));
            field("options", o);
        } else if (!strcmp(cmd, "tablecheck")) {
            U16 e = 0x55;
            ((U32(WINAPI *)(H, U16 *))F("AdsIsTableEncrypted"))(t, &e);
            field("encrypted", e);
            e = 0x55;
            ((U32(WINAPI *)(H, U16 *))F("AdsIsEncryptionEnabled"))(t, &e);
            field("enabled", e);
        } else {
            fprintf(stderr, "unknown command: %s\n", line);
        }
    }
    printf("\n}\n");
    return 0;
}
