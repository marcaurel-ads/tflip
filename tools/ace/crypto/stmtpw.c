/* Black-box probe: RC4 tables of a folder opened and created through SQL
 * statements, with AdsStmtSetTablePassword, AdsStmtEnableEncryption and
 * AdsStmtDisableEncryption .
 *
 *     stmtpw.exe <ace dll> <data folder> <script>  > stmtpw.json
 *
 * The script is read line by line:
 *
 *     connect <connect string>    AdsConnect101; "%D" is the data folder
 *     disconnect                  AdsDisconnect
 *     sql <statement>             AdsExecuteSQLDirect on a statement of its own
 *     stmt0 [conn]                no statement, or the connection's handle in its place
 *     stmt                        a statement kept for the s. steps (the old one is closed)
 *     s.exec <statement>          AdsExecuteSQLDirect on it; a cursor's record count and
 *                                 first record's first field are reported
 *     s.prepare <statement>       AdsPrepareSQL
 *     s.run                       AdsExecuteSQL of what it prepared
 *     s.setpw <table> <password>  AdsStmtSetTablePassword ("NULL" for a null pointer)
 *     s.clear                     AdsStmtClearTablePasswords
 *     s.enable <password>         AdsStmtEnableEncryption
 *     s.disable                   AdsStmtDisableEncryption
 *     topen <table> [<password>]  AdsOpenTable on the connection (kept as the table), then
 *                                 AdsEnableEncryption with the password
 *     t.enable <password>         AdsEnableEncryption on the table
 *     t.disable                   AdsDisableEncryption on the table
 *     tencrypt, tdecrypt          AdsEncryptTable, AdsDecryptTable
 *     tclose                      AdsCloseTable
 *     # ...                       a comment
 *
 * Every step reports its return code and, when it failed, AdsGetLastError's
 * code and text. The files it leaves are the probe's result. Run it from a
 * folder with the patched engine's DLLs (see LOCAL.md).
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

/* `text` as the text of an argument, "NULL" being a null pointer. */
static U8 *text_arg(const char *text) { return strcmp(text, "NULL") ? (U8 *)text : NULL; }

/* What a cursor holds: its record count and its first field of the first record. */
static void cursor(const char *tag, H cur) {
    char k[96];
    U32 count = 0;
    ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(cur, 1, &count);
    snprintf(k, sizeof k, "%s_count", tag);
    num(k, (long)count);
    if (count) {
        U8 buf[256]; U32 len = sizeof buf - 1;
        ((U32(WINAPI *)(H))F("AdsGotoTop"))(cur);
        if (!((U32(WINAPI *)(H, U8 *, U8 *, U32 *, U16))F("AdsGetField"))(cur, (U8 *)1, buf, &len, 0)) {
            snprintf(k, sizeof k, "%s_first", tag);
            text(k, buf, len);
        }
    }
}

int main(int argc, char **argv) {
    if (argc < 4) return fprintf(stderr, "usage: stmtpw <dll> <folder> <script>\n"), 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) return fprintf(stderr, "cannot load %s\n", argv[1]), 1;
    FILE *script = fopen(argv[3], "r");
    if (!script) return fprintf(stderr, "cannot read %s\n", argv[3]), 1;
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    printf("{");
    H conn = 0, stmt = 0, table = 0;
    char line[32768], expanded[65536], tag[64];
    int step = 0;
    while (fgets(line, sizeof line, script)) {
        line[strcspn(line, "\r\n")] = 0;
        if (!line[0] || line[0] == '#') continue;
        expand(expanded, sizeof expanded, line, argv[2]);
        snprintf(tag, sizeof tag, "%03d", ++step);
        char k[80];
        snprintf(k, sizeof k, "%s_cmd", tag);
        text(k, (U8 *)line, strlen(line));
        char *arg = strchr(expanded, ' ');
        if (arg) *arg++ = 0;
        const char *op = expanded;
        if (!strcmp(op, "connect")) {
            conn = 0;
            result(tag, ((U32(WINAPI *)(U8 *, H *, H *))F("AdsConnect101"))((U8 *)arg, NULL, &conn));
        } else if (!strcmp(op, "disconnect")) {
            if (stmt) ((U32(WINAPI *)(H))F("AdsCloseSQLStatement"))(stmt);
            stmt = 0;
            if (conn) result(tag, ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn));
            conn = 0;
        } else if (!strcmp(op, "sql")) {
            H one = 0, cur = 0;
            ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(conn, &one);
            result(tag, ((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(one, (U8 *)arg, &cur));
            if (cur) {
                cursor(tag, cur);
                ((U32(WINAPI *)(H))F("AdsCloseTable"))(cur);
            }
            ((U32(WINAPI *)(H))F("AdsCloseSQLStatement"))(one);
        } else if (!strcmp(op, "stmt")) {
            if (stmt) ((U32(WINAPI *)(H))F("AdsCloseSQLStatement"))(stmt);
            stmt = 0;
            result(tag, ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(conn, &stmt));
        } else if (!strcmp(op, "stmt0")) {
            /* A handle that is none, or a connection's, in place of the statement. */
            if (stmt) ((U32(WINAPI *)(H))F("AdsCloseSQLStatement"))(stmt);
            stmt = arg && !strcmp(arg, "conn") ? conn : 0;
            result(tag, 0);
        } else if (!strcmp(op, "s.exec") || !strcmp(op, "s.run") || !strcmp(op, "s.prepare")) {
            H cur = 0;
            U32 rc;
            if (!strcmp(op, "s.exec"))
                rc = ((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(stmt, (U8 *)arg, &cur);
            else if (!strcmp(op, "s.run"))
                rc = ((U32(WINAPI *)(H, H *))F("AdsExecuteSQL"))(stmt, &cur);
            else
                rc = ((U32(WINAPI *)(H, U8 *))F("AdsPrepareSQL"))(stmt, (U8 *)arg);
            result(tag, rc);
            if (cur) {
                cursor(tag, cur);
                ((U32(WINAPI *)(H))F("AdsCloseTable"))(cur);
            }
        } else if (!strcmp(op, "s.setpw")) {
            char *pw = strchr(arg, ' ');
            if (pw) *pw++ = 0;
            result(tag, ((U32(WINAPI *)(H, U8 *, U8 *))F("AdsStmtSetTablePassword"))(stmt, text_arg(arg), text_arg(pw ? pw : "")));
        } else if (!strcmp(op, "s.clear")) {
            result(tag, ((U32(WINAPI *)(H))F("AdsStmtClearTablePasswords"))(stmt));
        } else if (!strcmp(op, "s.enable")) {
            result(tag, ((U32(WINAPI *)(H, U8 *))F("AdsStmtEnableEncryption"))(stmt, text_arg(arg ? arg : "")));
        } else if (!strcmp(op, "s.disable")) {
            result(tag, ((U32(WINAPI *)(H))F("AdsStmtDisableEncryption"))(stmt));
        } else if (!strcmp(op, "t.enable")) {
            result(tag, ((U32(WINAPI *)(H, U8 *))F("AdsEnableEncryption"))(table, (U8 *)(arg ? arg : "")));
        } else if (!strcmp(op, "t.disable")) {
            result(tag, ((U32(WINAPI *)(H))F("AdsDisableEncryption"))(table));
        } else if (!strcmp(op, "topen")) {
            char *pw = strchr(arg, ' ');
            if (pw) *pw++ = 0;
            table = 0;
            result(tag, ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(conn, (U8 *)arg, NULL, 3, 1, 1, 1, 1, &table));
            if (pw) ((U32(WINAPI *)(H, U8 *))F("AdsEnableEncryption"))(table, (U8 *)pw);
        } else if (!strcmp(op, "tencrypt")) {
            result(tag, ((U32(WINAPI *)(H))F("AdsEncryptTable"))(table));
        } else if (!strcmp(op, "tdecrypt")) {
            result(tag, ((U32(WINAPI *)(H))F("AdsDecryptTable"))(table));
        } else if (!strcmp(op, "tclose")) {
            result(tag, ((U32(WINAPI *)(H))F("AdsCloseTable"))(table));
            table = 0;
        } else {
            fprintf(stderr, "unknown command: %s\n", line);
        }
    }
    if (conn) ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
    printf("\n}\n");
    return 0;
}
