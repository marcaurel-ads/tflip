/* Records where an Advantage Client Engine's seeks, seek-lasts and scopes
 * land, for the seek goldens (fixtures/seek/).
 *
 *     seek.exe <ace32.dll> <data folder> <cases file>  > out.json
 *
 * Built with i686-w64-mingw32-gcc and run under Wine (see run.sh). The cases
 * file has one command per line ('#' starts a comment; hex "-" is empty):
 *
 *     open <type> <char type> <table> [<index file> ...]
 *     seek <order> <soft|hard|after> <hex>      ADS_STRINGKEY, from record 3
 *     wseek <order> <soft|hard|after> <hex>     ADS_WSTRINGKEY (UTF-16LE hex)
 *     rseek <order> <soft|hard|after> <hex>     ADS_RAWKEY
 *     last <order> <hex>                        AdsSeekLast, ADS_STRINGKEY
 *     wlast <order> <hex>                       AdsSeekLast, ADS_WSTRINGKEY
 *     scope <order> <top|bottom> <hex>          AdsSetScope, ADS_STRINGKEY
 *     wscope <order> <top|bottom> <hex>         AdsSetScope, ADS_WSTRINGKEY
 *     clear <order> <top|bottom>                AdsClearScope
 *     walk <order>                              top to EOF, bottom to BOF
 *     key <order> <record>                      AdsExtractKey of a record
 *
 * A seek's result is where the table is: the record, or E at EOF, B at BOF
 * (BE at both), with * if the key was found -- or "rc <error code>". Each
 * command gives one JSON object {"case": "<line>", "out": "<result>"}. */
#include <windows.h>
#include <stdio.h>
#include <string.h>
#include <stdlib.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;

static HMODULE ace;
#define F(name) GetProcAddress(ace, name)
static H conn, table;
static int first = 1;
static char folder[MAX_PATH];

static int unhex(const char *s, U8 *out) {
    int n = 0;
    if (strcmp(s, "-") == 0) return 0;
    while (s[0] && s[1]) {
        unsigned v;
        sscanf(s, "%2x", &v);
        out[n++] = (U8)v;
        s += 2;
    }
    return n;
}

static void out(const char *line, const char *result) {
    printf("%s\n{\"case\": \"", first ? "[" : ",");
    for (const char *p = line; *p; p++) printf(*p == '\\' ? "/" : "%c", *p);
    printf("\", \"out\": \"%s\"}", result);
    first = 0;
}

static void where(char *buf) {
    U16 eof = 0, bof = 0;
    ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(table, &eof);
    ((U32(WINAPI *)(H, U16 *))F("AdsAtBOF"))(table, &bof);
    if (eof && bof) strcpy(buf, "BE");
    else if (eof) strcpy(buf, "E");
    else if (bof) strcpy(buf, "B");
    else {
        U32 rec = 0;
        ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordNum"))(table, 2, &rec);
        sprintf(buf, "%lu", rec);
    }
}

static H order(const char *name) {
    H ord = 0;
    ((U32(WINAPI *)(H, U8 *, H *))F("AdsGetIndexHandle"))(table, (U8 *)name, &ord);
    return ord;
}

static U16 how(const char *s) {
    if (strcmp(s, "soft") == 0) return 1;
    if (strcmp(s, "hard") == 0) return 2;
    return 4;
}

int main(int argc, char **argv) {
    if (argc < 4) { fprintf(stderr, "usage: seek.exe <ace32.dll> <folder> <cases>\n"); return 2; }
    ace = LoadLibraryA(argv[1]);
    if (!ace) { fprintf(stderr, "cannot load %s\n", argv[1]); return 2; }
    strcpy(folder, argv[2]);
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    if (((U32(WINAPI *)(U8 *, U16, H *))F("AdsConnect26"))((U8 *)folder, 1, &conn)) {
        fprintf(stderr, "connect failed\n");
        return 1;
    }
    FILE *cases = fopen(argv[3], "r");
    if (!cases) { fprintf(stderr, "cannot open %s\n", argv[3]); return 2; }
    char line[1024];
    while (fgets(line, sizeof line, cases)) {
        line[strcspn(line, "\r\n")] = 0;
        if (!line[0] || line[0] == '#') continue;
        char cmd[32] = "", a[256] = "", b[256] = "", c[512] = "";
        sscanf(line, "%31s %255s %255s %511s", cmd, a, b, c);
        static char result[16384];
        result[0] = 0;
        U8 key[512];
        U32 rc = 0;
        if (strcmp(cmd, "open") == 0) {
            if (table) ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
            table = 0;
            char path[MAX_PATH];
            snprintf(path, sizeof path, "%s\\%s", folder, c);
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
                conn, (U8 *)path, NULL, (U16)atoi(a), (U16)atoi(b), 0, 2, 0, &table);
            sprintf(result, "%lu", rc);
            /* Index files after the table name. */
            char *rest = line;
            for (int i = 0; i < 4 && rest; i++) { rest = strchr(rest, ' '); if (rest) rest++; }
            while (rc == 0 && rest && *rest) {
                char file[MAX_PATH];
                sscanf(rest, "%259s", file);
                snprintf(path, sizeof path, "%s\\%s", folder, file);
                H orders[50];
                U16 n = 50;
                U32 irc = ((U32(WINAPI *)(H, U8 *, H *, U16 *))F("AdsOpenIndex"))(
                    table, (U8 *)path, orders, &n);
                sprintf(result + strlen(result), " %lu", irc);
                rest = strchr(rest, ' ');
                if (rest) rest++;
            }
        } else if (!strcmp(cmd, "seek") || !strcmp(cmd, "wseek") || !strcmp(cmd, "rseek")) {
            U16 type = cmd[0] == 'w' ? 8 : cmd[0] == 'r' ? 1 : 2;
            int n = unhex(c, key);
            ((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(table, 3);
            U16 found = 0;
            rc = ((U32(WINAPI *)(H, U8 *, U16, U16, U16, U16 *))F("AdsSeek"))(
                order(a), key, (U16)n, type, how(b), &found);
            if (rc) sprintf(result, "rc %lu", rc);
            else { where(result); if (found) strcat(result, "*"); }
        } else if (!strcmp(cmd, "last") || !strcmp(cmd, "wlast")) {
            U16 type = cmd[0] == 'w' ? 8 : 2;
            int n = unhex(b, key);
            ((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(table, 3);
            U16 found = 0;
            rc = ((U32(WINAPI *)(H, U8 *, U16, U16, U16 *))F("AdsSeekLast"))(
                order(a), key, (U16)n, type, &found);
            if (rc) sprintf(result, "rc %lu", rc);
            else { where(result); if (found) strcat(result, "*"); }
        } else if (!strcmp(cmd, "scope") || !strcmp(cmd, "wscope")) {
            U16 type = cmd[0] == 'w' ? 8 : 2;
            int n = unhex(c, key);
            rc = ((U32(WINAPI *)(H, U16, U8 *, U16, U16))F("AdsSetScope"))(
                order(a), strcmp(b, "top") == 0 ? 1 : 2, key, (U16)n, type);
            sprintf(result, "%lu", rc);
        } else if (!strcmp(cmd, "clear")) {
            rc = ((U32(WINAPI *)(H, U16))F("AdsClearScope"))(order(a), strcmp(b, "top") == 0 ? 1 : 2);
            sprintf(result, "%lu", rc);
        } else if (!strcmp(cmd, "walk")) {
            H ord = order(a);
            char at[32];
            ((U32(WINAPI *)(H))F("AdsGotoTop"))(ord);
            for (int i = 0; i < 400; i++) {
                where(at);
                strcat(result, at);
                if (strchr(at, 'E')) break;
                strcat(result, " ");
                ((U32(WINAPI *)(H, long))F("AdsSkip"))(ord, 1);
            }
            strcat(result, " /");
            ((U32(WINAPI *)(H))F("AdsGotoBottom"))(ord);
            for (int i = 0; i < 400; i++) {
                where(at);
                strcat(result, " ");
                strcat(result, at);
                if (strchr(at, 'B')) break;
                ((U32(WINAPI *)(H, long))F("AdsSkip"))(ord, -1);
            }
        } else if (!strcmp(cmd, "key")) {
            ((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(table, (U32)atoi(b));
            U16 len = sizeof key;
            rc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsExtractKey"))(order(a), key, &len);
            if (rc) sprintf(result, "rc %lu", rc);
            else for (int i = 0; i < len; i++) sprintf(result + 2 * i, "%02x", key[i]);
        } else {
            strcpy(result, "?");
        }
        out(line, result);
    }
    printf("%s\n]\n", first ? "[" : "");
    if (table) ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
    return 0;
}
