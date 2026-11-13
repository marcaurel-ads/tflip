/* Records what a dictionary connection gets when it opens, creates or drops a
 * free table (with and without the connection option ADS_CHECK_FREE_TABLE_ACCESS
 * and the user property ADS_DD_FREE_TABLES_RIGHTS), what a user without ALTER
 * gets from AdsCreateIndex on an open table, and what the engine does about a
 * dictionary table whose files are missing, for fixtures/dictionary/freetables.json.
 *
 *     freetables.exe <ace32.dll> <data folder> <steps file>  > freetables.json
 *
 * Each step of the file is one line; the output has one JSON line per step with
 * the return code and, when it is not 0, the error code and text:
 *
 *   conn <name> <dict|dict:file|free> <user|-> <password> <options>
 *                                                           AdsConnect60 (the dictionary is props.add
 *                                                           in the folder, or the file named)
 *   disc <name>
 *   open <name> <table name> <type> <options>               AdsOpenTable, handle kept as <table name>
 *   close <table name>
 *   sql <name> <statement>                                  AdsExecuteSQLDirect on a new statement
 *   setuser <name> <user> <id> <number>                     AdsDDSetUserProperty (a 4-byte number)
 *   getuser <name> <user> <id>                              AdsDDGetUserProperty (a 4-byte number)
 *   setgroup <name> <group> <id> <number>, getgroup ...     the same for a group
 *   index <table name> <file> <tag> <expr> <options>        AdsCreateIndex
 *   exists <file>, remove <file>, copy <file> <new name>    the data folder's files
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
#define F(name) ((void *)GetProcAddress(ace, name))

/* `folder` is where the files are; `served` is what a connection is given when a
 * remote server serves them (its path as the server names it), else the same. */
static char folder[MAX_PATH], served[MAX_PATH];
static U16 server_type = 1;
static struct {
    char name[32];
    H h;
} slots[32];
static int nslots;

static H *slot(const char *name) {
    for (int i = 0; i < nslots; i++)
        if (!strcmp(slots[i].name, name)) return &slots[i].h;
    snprintf(slots[nslots].name, 32, "%s", name);
    slots[nslots].h = 0;
    return &slots[nslots++].h;
}

static void jstr(const char *s) {
    putchar('"');
    for (; *s; s++) {
        /* The folder differs between runs and between the original and tflip. */
        if (!strncmp(s, folder, strlen(folder))) {
            printf("<folder>");
            s += strlen(folder) - 1;
            continue;
        }
        if (!strncmp(s, served, strlen(served))) {
            printf("<folder>");
            s += strlen(served) - 1;
            continue;
        }
        if (*s == '"' || *s == '\\') printf("\\%c", *s);
        else if ((U8)*s < 32 || (U8)*s > 126) printf("\\u%04x", (U8)*s);
        else putchar(*s);
    }
    putchar('"');
}

static int first = 1;
static void report(const char *line, U32 rc, const char *extra) {
    printf("%s\n{\"op\": ", first ? "[" : ",");
    first = 0;
    jstr(line);
    printf(", \"rc\": %lu", rc);
    if (rc) {
        U32 code = 0;
        U8 text[1200] = "";
        U16 len = sizeof text;
        ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
        printf(", \"code\": %lu, \"text\": ", code);
        jstr((char *)text);
    }
    if (extra) printf(", %s", extra);
    putchar('}');
}

static char *next(char **p) {
    while (**p == ' ') (*p)++;
    char *s = *p;
    while (**p && **p != ' ') (*p)++;
    if (**p) *(*p)++ = 0;
    return s;
}

int main(int argc, char **argv) {
    if (argc != 4 && argc != 5)
        return fprintf(stderr, "usage: freetables.exe <ace32.dll> <folder> <steps> [<server's path of the folder>]\n"), 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) return fprintf(stderr, "cannot load %s\n", argv[1]), 1;
    snprintf(folder, sizeof folder, "%s", argv[2]);
    snprintf(served, sizeof served, "%s", argc == 5 ? argv[4] : argv[2]);
    if (argc == 5) server_type = 2;
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(server_type);
    FILE *f = fopen(argv[3], "r");
    char line[1024], work[1024];
    while (fgets(line, sizeof line, f)) {
        line[strcspn(line, "\r\n")] = 0;
        if (!line[0] || line[0] == '#') continue;
        strcpy(work, line);
        char *p = work, *op = next(&p);
        U32 rc = 0;
        char extra[256] = "";
        if (!strcmp(op, "conn")) {
            char *name = next(&p), *kind = next(&p), *user = next(&p), *pw = next(&p), *opts = next(&p);
            char path[MAX_PATH];
            if (!strncmp(kind, "dict", 4))
                snprintf(path, sizeof path, "%s\\%s", served, kind[4] == ':' ? kind + 5 : "props.add");
            else snprintf(path, sizeof path, "%s\\", served);
            H *h = slot(name);
            *h = 0;
            rc = ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
                (U8 *)path, server_type, !strcmp(user, "-") ? NULL : (U8 *)user,
                !strcmp(pw, "-") ? NULL : (U8 *)pw, strtoul(opts, NULL, 0) | (server_type == 2 ? 0x80 : 0), h);
        } else if (!strcmp(op, "conn101")) {
            /* The rest of the line is the connect string, "%D" the dictionary and "%F" the folder. */
            char *name = next(&p), string[600] = "";
            for (char *q = p; *q; q++) {
                if (q[0] == '%' && q[1] == 'D') strcat(string, served), strcat(string, "\\props.add"), q++;
                else if (q[0] == '%' && q[1] == 'F') strcat(string, served), q++;
                else strncat(string, q, 1);
            }
            H *h = slot(name), options = 0;
            *h = 0;
            rc = ((U32(WINAPI *)(U8 *, H *, H *))F("AdsConnect101"))((U8 *)string, &options, h);
        } else if (!strcmp(op, "rows")) {
            H c = *slot(next(&p)), s = 0, cur = 0;
            U32 n = 0;
            ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(c, &s);
            rc = ((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(s, (U8 *)p, &cur);
            if (!rc && cur) ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(cur, 1, &n);
            snprintf(extra, sizeof extra, "\"rows\": %lu", n);
            report(line, rc, extra);
            if (cur) ((U32(WINAPI *)(H))F("AdsCloseTable"))(cur);
            ((U32(WINAPI *)(H))F("AdsCloseSQLStatement"))(s);
            continue;
        } else if (!strcmp(op, "disc")) {
            rc = ((U32(WINAPI *)(H))F("AdsDisconnect"))(*slot(next(&p)));
        } else if (!strcmp(op, "open")) {
            char *name = next(&p), *table = next(&p), *type = next(&p), *opts = next(&p);
            H *t = slot(table);
            char full[MAX_PATH];
            /* A free table is given with its folder: the connection's own is not looked in. */
            snprintf(full, sizeof full, strtoul(type, NULL, 0) ? "%s\\%s" : "%.0s%s", served, table);
            *t = 0;
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
                *slot(name), (U8 *)full, NULL, (U16)strtoul(type, NULL, 0), 1, 0, 1, strtoul(opts, NULL, 0), t);
        } else if (!strcmp(op, "close")) {
            rc = ((U32(WINAPI *)(H))F("AdsCloseTable"))(*slot(next(&p)));
        } else if (!strcmp(op, "sql")) {
            H c = *slot(next(&p)), s = 0, cur = 0;
            ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(c, &s);
            rc = ((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(s, (U8 *)p, &cur);
            /* The error is read before the statement's closing replaces it. */
            report(line, rc, NULL);
            if (cur) ((U32(WINAPI *)(H))F("AdsCloseTable"))(cur);
            ((U32(WINAPI *)(H))F("AdsCloseSQLStatement"))(s);
            continue;
        } else if (!strcmp(op, "setuser") || !strcmp(op, "setgroup")) {
            char *name = next(&p), *who = next(&p), *id = next(&p), *val = next(&p), *size = next(&p);
            U32 v = strtoul(val, NULL, 0);
            U16 vlen = *size ? (U16)strtoul(size, NULL, 0) : 4;
            rc = ((U32(WINAPI *)(H, U8 *, U16, void *, U16))F(op[3] == 'u' ? "AdsDDSetUserProperty"
                                                                         : "AdsDDSetUserGroupProperty"))(
                *slot(name), (U8 *)who, (U16)strtoul(id, NULL, 0), &v, vlen);
        } else if (!strcmp(op, "getuser") || !strcmp(op, "getgroup")) {
            char *name = next(&p), *who = next(&p), *id = next(&p);
            U32 v = 0;
            U16 len = 4;
            rc = ((U32(WINAPI *)(H, U8 *, U16, void *, U16 *))F(op[3] == 'u' ? "AdsDDGetUserProperty"
                                                                           : "AdsDDGetUserGroupProperty"))(
                *slot(name), (U8 *)who, (U16)strtoul(id, NULL, 0), &v, &len);
            snprintf(extra, sizeof extra, "\"value\": %lu, \"len\": %u", v, len);
        } else if (!strcmp(op, "index")) {
            char *table = next(&p), *file = next(&p), *tag = next(&p), *expr = next(&p), *opts = next(&p);
            H ix = 0;
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, H *))F("AdsCreateIndex"))(
                *slot(table), !strcmp(file, "-") ? NULL : (U8 *)file, (U8 *)tag, (U8 *)expr, NULL, NULL,
                strtoul(opts, NULL, 0), &ix);
        } else if (!strcmp(op, "path")) {
            U8 buf[400] = "";
            U16 len = sizeof buf;
            rc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsGetConnectionPath"))(*slot(next(&p)), buf, &len);
            snprintf(extra, sizeof extra, "\"path\": \"%s\"", buf);
        } else if (!strcmp(op, "copy")) {
            /* The files the engine made, kept for the golden (after the connections are closed). */
            char from[MAX_PATH], to[MAX_PATH];
            snprintf(from, sizeof from, "%s\\%s", folder, next(&p));
            snprintf(to, sizeof to, "%s\\%s", folder, next(&p));
            snprintf(extra, sizeof extra, "\"copied\": %d", CopyFileA(from, to, FALSE) != 0);
        } else if (!strcmp(op, "exists") || !strcmp(op, "remove")) {
            char path[MAX_PATH];
            snprintf(path, sizeof path, "%s\\%s", folder, next(&p));
            if (op[0] == 'r') DeleteFileA(path);
            snprintf(extra, sizeof extra, "\"exists\": %d", GetFileAttributesA(path) != INVALID_FILE_ATTRIBUTES);
        } else {
            return fprintf(stderr, "bad step %s\n", line), 2;
        }
        report(line, rc, extra[0] ? extra : NULL);
    }
    printf("\n]\n");
    return 0;
}
