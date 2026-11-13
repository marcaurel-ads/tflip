/* Black-box probe: AES data dictionaries made with known secrets.
 *
 *     aesdict.exe <ace dll> <data folder> <script>  > aesdict.json
 *
 * Run it from a folder with the patched engine's DLLs (the OpenSSL ones AES
 * needs). The script is read line by line:
 *
 *     connect <connect string>   AdsConnect101; "%D" is the data folder
 *     sql <statement>            AdsExecuteSQLDirect on the connection
 *     ddcreate <flag> <path>     AdsDDCreate (the connection becomes the dictionary's)
 *     ddcreate101 <connect string>   AdsDDCreate101
 *     ddlink <alias> <path> <user> <password> <options>   AdsDDCreateLink
 *     ddpub <name> <comment> <options>                    AdsDDCreatePublication
 *     ddsub <name> <publication> <target> <user> <password> <queue> <forward> <comment> <options>
 *                                AdsDDCreateSubscription (in these a ~ is a space and a - none)
 *     query <statement>          AdsExecuteSQLDirect, then every row of its cursor as text
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
        } else if (!strncmp(expanded, "ddlink ", 7) || !strncmp(expanded, "ddpub ", 6) ||
                   !strncmp(expanded, "ddsub ", 6)) {
            char *w[9];
            char *rest = strchr(expanded, ' ') + 1;
            int n = !strncmp(expanded, "ddlink ", 7) ? 5 : !strncmp(expanded, "ddpub ", 6) ? 3 : 9;
            for (int i = 0; i < n; i++) {
                w[i] = rest;
                char *sp = strchr(rest, ' ');
                if (sp) { *sp = 0; rest = sp + 1; } else rest += strlen(rest);
                for (char *c = w[i]; *c; c++) if (*c == '~') *c = ' ';
            }
#define OPT(i) (!strcmp(w[i], "-") ? NULL : (U8 *)w[i])
            if (!strncmp(expanded, "ddlink", 6)) {
                result(tag, ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U32))F("AdsDDCreateLink"))(
                                conn, (U8 *)w[0], (U8 *)w[1], OPT(2), OPT(3),
                                (U32)strtoul(w[4], NULL, 10)));
            } else if (!strncmp(expanded, "ddpub", 5)) {
                result(tag, ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsDDCreatePublication"))(
                                conn, (U8 *)w[0], OPT(1), (U32)strtoul(w[2], NULL, 10)));
            } else {
                result(tag, ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U8 *, U16, U8 *,
                                            U32))F("AdsDDCreateSubscription"))(
                                conn, (U8 *)w[0], (U8 *)w[1], (U8 *)w[2], OPT(3), OPT(4), OPT(5),
                                (U16)atoi(w[6]), OPT(7), (U32)strtoul(w[8], NULL, 10)));
            }
#undef OPT
        } else if (!strncmp(expanded, "query ", 6)) {
            H stmt = 0, cur = 0;
            ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(conn, &stmt);
            U32 rc = ((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(stmt, (U8 *)expanded + 6,
                                                                              &cur);
            result(tag, rc);
            if (!rc && cur) {
                U16 nfields = 0;
                ((U32(WINAPI *)(H, U16 *))F("AdsGetNumFields"))(cur, &nfields);
                char rows[60000];
                size_t o = 0;
                U32 eof = 0;
                ((U32(WINAPI *)(H))F("AdsGotoTop"))(cur);
                for (;;) {
                    ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(cur, (U16 *)&eof);
                    if ((U16)eof) break;
                    for (U16 f = 1; f <= nfields && o + 600 < sizeof rows; f++) {
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
                text("rows", (U8 *)rows, (long)o);
            }
            if (cur) ((U32(WINAPI *)(H))F("AdsCloseTable"))(cur);
            ((U32(WINAPI *)(H))F("AdsCloseSQLStatement"))(stmt);
        } else if (!strcmp(expanded, "disconnect")) {
            if (conn) result(tag, ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn));
            conn = 0;
        } else if (!strncmp(expanded, "sql ", 4)) {
            H stmt = 0, cur = 0;
            ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(conn, &stmt);
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
