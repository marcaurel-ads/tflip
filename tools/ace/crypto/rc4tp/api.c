/* Black-box probe: what the AdsDD* and table calls do in a dictionary of RC4
 * tables with a table password; aesdict.c with more commands:
 *
 *     dbset <id> <0|1>           AdsDDSetDatabaseProperty (a 2-byte boolean)
 *     dbget <id>                 AdsDDGetDatabaseProperty, the value reported
 *     tset <table> <id> <0|1>    AdsDDSetTableProperty
 *     tget <table> <id>          AdsDDGetTableProperty, the value reported
 *     create <name>              AdsCreateTable (an Integer and a Memo field)
 *     createt <type> <name>      the same of the table type 1 (NTX), 2 (CDX), 3 (ADT) or 4 (VFP)
 *     createf <type> <name> <fields>   AdsCreateTable of the table type with the field list as AdsCreateTable's
 *     addtable <name> <file>     AdsDDAddTable of an ADT table
 *     addtablet <type> <name> <file> [<index file>]   the same of a table type
 *     stmttype <type>            the table type of the statements that follow (AdsStmtSetTableType)
 *     enc <name> / dec <name>    AdsEncryptTable / AdsDecryptTable on the table opened exclusively
 *     begin / commit / rollback  AdsBeginTransaction / AdsCommitTransaction / AdsRollbackTransaction of the connection
 *     copy <name> <file>         AdsCopyTable of the table opened (exclusively) to <file>
 *     copyc <name> <to>          AdsCopyTableContents of the table <name> to the table <to>
 *     restr <path> <fields>      AdsRestructureTable120 of the table <path> (type of stmttype) to add <fields>
 *     zap <name> / pack <name>   AdsZapTable / AdsPackTable on the table opened exclusively
 *     encpw <name> <password> [x]  AdsEnableEncryption, then AdsEncryptTable (not with x), of a free table opened exclusively
 *
 *     api.exe <ace dll> <data folder> <script>  > api.json
 *
 * Run it from a folder with the patched engine's DLLs (the OpenSSL ones AES
 * needs). The script is read line by line:
 *
 *     connect <connect string>   AdsConnect101; "%D" is the data folder
 *     sql <statement>            AdsExecuteSQLDirect on the connection
 *     query <statement>          the same, then every row of the cursor as text ("<step>_rows")
 *     ddcreate <flag> <path>     AdsDDCreate (the connection becomes the dictionary's)
 *     ddcreate101 <connect string>   AdsDDCreate101
 *     disconnect                 AdsDisconnect of the connection
 *     # ...                      a comment
 *
 * Every step reports its return code and, when it failed, AdsGetLastError's
 * code and text. The dictionaries it leaves are the probe's result.
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

static void result(const char *tag, U32 rc) {
    char k[128];
    snprintf(k, sizeof k, "%s_rc", tag); num(k, rc);
    if (!rc) return;
    U32 code = 0; U8 msg[1100]; U16 n = sizeof msg;
    ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, msg, &n);
    snprintf(k, sizeof k, "%s_err", tag); num(k, code);
    snprintf(k, sizeof k, "%s_text", tag); text(k, msg, n);
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

int main(int argc, char **argv) {
    if (argc < 4) return fprintf(stderr, "usage: aesdict <dll> <folder> <script>\n"), 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) return fprintf(stderr, "cannot load %s\n", argv[1]), 1;
    FILE *script = fopen(argv[3], "r");
    if (!script) return fprintf(stderr, "cannot read %s\n", argv[3]), 1;
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    printf("{");
    H conn = 0;
    char line[32768], expanded[65536], tag[64];
    int step = 0;
    U16 stmt_type = 0;
    while (fgets(line, sizeof line, script)) {
        line[strcspn(line, "\r\n")] = 0;
        if (!line[0] || line[0] == '#') continue;
        expand(expanded, sizeof expanded, line, argv[2]);
        snprintf(tag, sizeof tag, "%03d", ++step);
        char k[80];
        snprintf(k, sizeof k, "%s_cmd", tag);
        text(k, (U8 *)line, strlen(line));
        if (!strncmp(expanded, "connect ", 8)) {
            conn = 0;
            result(tag, ((U32(WINAPI *)(U8 *, H *, H *))F("AdsConnect101"))((U8 *)expanded + 8,
                                                                            NULL, &conn));
        } else if (!strncmp(expanded, "ddcreate101 ", 12)) {
            conn = 0;
            result(tag, ((U32(WINAPI *)(U8 *, H *, H *))F("AdsDDCreate101"))((U8 *)expanded + 12,
                                                                             NULL, &conn));
        } else if (!strncmp(expanded, "ddcreate ", 9)) {
            /* ddcreate <flag> <path> */
            conn = 0;
            U16 flag = (U16)atoi(expanded + 9);
            result(tag, ((U32(WINAPI *)(U8 *, U16, U8 *, H *))F("AdsDDCreate"))(
                            (U8 *)strchr(expanded + 9, ' ') + 1, flag, NULL, &conn));
        } else if (!strncmp(expanded, "dbset ", 6)) {
            U16 id = (U16)atoi(expanded + 6), v = (U16)atoi(strchr(expanded + 6, ' ') + 1);
            result(tag, ((U32(WINAPI *)(H, U16, void *, U16))F("AdsDDSetDatabaseProperty"))(conn, id, &v, 2));
        } else if (!strncmp(expanded, "dbget ", 6)) {
            U16 id = (U16)atoi(expanded + 6), v = 0xFFFF, n = 2;
            U32 rc = ((U32(WINAPI *)(H, U16, void *, U16 *))F("AdsDDGetDatabaseProperty"))(conn, id, &v, &n);
            result(tag, rc);
            if (!rc) { snprintf(k, sizeof k, "%s_val", tag); num(k, v); }
        } else if (!strncmp(expanded, "tset ", 5)) {
            char name[64]; int id, v;
            sscanf(expanded + 5, "%63s %d %d", name, &id, &v);
            U16 value = (U16)v;
            result(tag, ((U32(WINAPI *)(H, U8 *, U16, void *, U16, U16, U8 *))F("AdsDDSetTableProperty"))(
                            conn, (U8 *)name, (U16)id, &value, 2, 0, NULL));
        } else if (!strncmp(expanded, "tget ", 5)) {
            char name[64]; int id;
            sscanf(expanded + 5, "%63s %d", name, &id);
            U16 v = 0xFFFF, n = 2;
            U32 rc = ((U32(WINAPI *)(H, U8 *, U16, void *, U16 *))F("AdsDDGetTableProperty"))(
                            conn, (U8 *)name, (U16)id, &v, &n);
            result(tag, rc);
            if (!rc) { snprintf(k, sizeof k, "%s_val", tag); num(k, v); }
        } else if (!strncmp(expanded, "create ", 7) || !strncmp(expanded, "createt ", 8)) {
            H t = 0;
            int type = 3;
            const char *name = expanded + 7;
            if (expanded[6] == 't') type = atoi(expanded + 8), name = strchr(expanded + 8, ' ') + 1;
            result(tag, ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *))F("AdsCreateTable"))(
                            conn, (U8 *)name, NULL, type, 1, 1, 1, 0, (U8 *)"id, Integer; note, Memo;", &t));
            if (t) ((U32(WINAPI *)(H))F("AdsCloseTable"))(t);
        } else if (!strncmp(expanded, "createf ", 8)) {
            /* createf <type> <name> <fields as AdsCreateTable's> */
            H t = 0;
            char name[64];
            int type = atoi(expanded + 8), skip = 0;
            const char *p = strchr(expanded + 8, ' ') + 1;
            sscanf(p, "%63s%n", name, &skip);
            result(tag, ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *))F("AdsCreateTable"))(
                            conn, (U8 *)name, NULL, type, 1, 1, 1, 0, (U8 *)p + skip + 1, &t));
            if (t) ((U32(WINAPI *)(H))F("AdsCloseTable"))(t);
        } else if (!strncmp(expanded, "addtable ", 9) || !strncmp(expanded, "addtablet ", 10)) {
            char name[64], file[256], index[256] = "";
            int type = 3, skip = 9;
            if (expanded[8] == 't') type = atoi(expanded + 10), skip = (int)(strchr(expanded + 10, ' ') - expanded) + 1;
            sscanf(expanded + skip, "%63s %255s %255s", name, file, index);
            result(tag, ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U8 *, U8 *))F("AdsDDAddTable"))(
                            conn, (U8 *)name, (U8 *)file, (U16)type, 1, (U8 *)index, (U8 *)""));
        } else if (!strncmp(expanded, "stmttype ", 9)) {
            stmt_type = (U16)atoi(expanded + 9);
        } else if (!strncmp(expanded, "enc ", 4) || !strncmp(expanded, "dec ", 4)) {
            H t = 0;
            U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
                            conn, (U8 *)expanded + 4, NULL, 0, 1, 1, 1, 1, &t);
            if (!rc) rc = ((U32(WINAPI *)(H))F(expanded[0] == 'e' ? "AdsEncryptTable" : "AdsDecryptTable"))(t);
            result(tag, rc);
            if (t) ((U32(WINAPI *)(H))F("AdsCloseTable"))(t);
        } else if (!strcmp(expanded, "begin") || !strcmp(expanded, "commit") || !strcmp(expanded, "rollback")) {
            result(tag, ((U32(WINAPI *)(H))F(expanded[0] == 'b' ? "AdsBeginTransaction"
                                              : expanded[0] == 'c' ? "AdsCommitTransaction"
                                                                   : "AdsRollbackTransaction"))(conn));
        } else if (!strncmp(expanded, "copy ", 5) || !strncmp(expanded, "copyc ", 6)) {
            H t = 0, u = 0;
            int c = expanded[4] == 'c';
            char name[256], to[256];
            sscanf(expanded + (c ? 6 : 5), "%255s %255s", name, to);
            U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
                            conn, (U8 *)name, NULL, strchr(name, 0x5C) ? stmt_type : 0, 1, 1, 1, 1, &t);
            if (!rc && c)
                rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
                            conn, (U8 *)to, NULL, strchr(to, 0x5C) ? stmt_type : 0, 1, 1, 1, 1, &u);
            if (!rc)
                rc = c ? ((U32(WINAPI *)(H, H, U16))F("AdsCopyTableContents"))(t, u, 2)
                       : ((U32(WINAPI *)(H, U16, U8 *))F("AdsCopyTable"))(t, 2, (U8 *)to);
            result(tag, rc);
            if (u) ((U32(WINAPI *)(H))F("AdsCloseTable"))(u);
            if (t) ((U32(WINAPI *)(H))F("AdsCloseTable"))(t);
        } else if (!strncmp(expanded, "restr ", 6)) {
            char path[256];
            sscanf(expanded + 6, "%255s", path);
            const char *add = strchr(expanded + 6, 0x20) + 1;
            result(tag, ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U8 *, U8 *, U8 *, U8 *, U32, U32))
                             F("AdsRestructureTable120"))(conn, (U8 *)path, NULL, stmt_type, 1, 1, 1,
                                                          (U8 *)add, (U8 *)"", (U8 *)"", (U8 *)"", 0, 0));
        } else if (!strncmp(expanded, "zap ", 4) || !strncmp(expanded, "pack ", 5)) {
            H t = 0;
            const char *name = expanded + (expanded[0] == 'z' ? 4 : 5);
            U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
                            conn, (U8 *)name, NULL, strchr(name, 0x5C) ? stmt_type : 0, 1, 1, 1, 1, &t);
            if (!rc) rc = ((U32(WINAPI *)(H))F(expanded[0] == 'z' ? "AdsZapTable" : "AdsPackTable"))(t);
            result(tag, rc);
            if (t) ((U32(WINAPI *)(H))F("AdsCloseTable"))(t);
        } else if (!strncmp(expanded, "encpw ", 6)) {
            H t = 0;
            char name[256], pw[64], only[8] = "";
            sscanf(expanded + 6, "%255s %63s %7s", name, pw, only);
            U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
                            conn, (U8 *)name, NULL, strchr(name, 0x5C) ? stmt_type : 0, 1, 1, 1, 1, &t);
            if (!rc) rc = ((U32(WINAPI *)(H, U8 *))F("AdsEnableEncryption"))(t, (U8 *)pw);
            if (!rc && !only[0]) rc = ((U32(WINAPI *)(H))F("AdsEncryptTable"))(t);
            result(tag, rc);
            if (t) ((U32(WINAPI *)(H))F("AdsCloseTable"))(t);
        } else if (!strcmp(expanded, "disconnect")) {
            if (conn) result(tag, ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn));
            conn = 0;
        } else if (!strncmp(expanded, "query ", 6)) {
            H stmt = 0, cur = 0;
            ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(conn, &stmt);
            if (stmt_type) ((U32(WINAPI *)(H, U16))F("AdsStmtSetTableType"))(stmt, stmt_type);
            U32 rc = ((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(stmt, (U8 *)expanded + 6, &cur);
            result(tag, rc);
            if (!rc && cur) {
                U16 nfields = 0;
                ((U32(WINAPI *)(H, U16 *))F("AdsGetNumFields"))(cur, &nfields);
                static char rows[60000];
                size_t o = 0;
                U16 eof = 0;
                ((U32(WINAPI *)(H))F("AdsGotoTop"))(cur);
                for (;;) {
                    ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(cur, &eof);
                    if (eof) break;
                    for (U16 f = 1; f <= nfields && o + 600 < sizeof rows; f++) {
                        U8 name[128], value[512];
                        U16 nl = sizeof name;
                        U32 vl = sizeof value - 1;
                        ((U32(WINAPI *)(H, U16, U8 *, U16 *))F("AdsGetFieldName"))(cur, f, name, &nl);
                        name[nl] = 0;
                        U32 r = ((U32(WINAPI *)(H, ULONG_PTR, U8 *, U32 *, U16))F("AdsGetString"))(cur, f, value, &vl, 1);
                        value[r ? 0 : vl] = 0;
                        if (r) {
                            /* AdsGetString refuses an Integer field. */
                            long n = 0;
                            if (!((U32(WINAPI *)(H, ULONG_PTR, long *))F("AdsGetLong"))(cur, f, &n))
                                snprintf((char *)value, sizeof value, "%ld", n);
                        }
                        o += snprintf(rows + o, sizeof rows - o, "%s=%s;", name, value);
                    }
                    rows[o++] = '|';
                    ((U32(WINAPI *)(H, long))F("AdsSkip"))(cur, 1);
                }
                rows[o] = 0;
                snprintf(k, sizeof k, "%s_rows", tag);
                text(k, (U8 *)rows, (long)o);
            }
            if (cur) ((U32(WINAPI *)(H))F("AdsCloseTable"))(cur);
            ((U32(WINAPI *)(H))F("AdsCloseSQLStatement"))(stmt);
        } else if (!strncmp(expanded, "sql ", 4)) {
            H stmt = 0, cur = 0;
            ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(conn, &stmt);
            if (stmt_type) ((U32(WINAPI *)(H, U16))F("AdsStmtSetTableType"))(stmt, stmt_type);
            result(tag, ((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(
                            stmt, (U8 *)expanded + 4, &cur));
            if (cur) ((U32(WINAPI *)(H))F("AdsCloseTable"))(cur);
            ((U32(WINAPI *)(H))F("AdsCloseSQLStatement"))(stmt);
        } else {
            fprintf(stderr, "unknown command: %s\n", line);
        }
    }
    if (conn) ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
    printf("\n}\n");
    return 0;
}
