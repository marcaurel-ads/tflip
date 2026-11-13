/* Records how an Advantage Client Engine applies filters, for
 * fixtures/expr/filters.json.
 *
 *     filters.exe <ace32.dll> <data folder> <script>  > filters.json
 *
 * Built with i686-w64-mingw32-gcc and run under Wine. The script has one
 * command per line; after each, the probe records its result and the
 * table's state. Commands:
 *
 *     # comment
 *     @table <file>             open <file> (relative to the data folder)
 *     order <name> | order -    move in an index order, or in record order
 *     aof <expression>          AdsSetAOF (ADS_RESOLVE_IMMEDIATE); with no
 *                               expression, an empty one
 *     filter <expression>       AdsSetFilter, likewise
 *     clearaof | clearfilter
 *     getaof | getfilter
 *     top | bottom | skip <n> | goto <n>
 *     seek <hard|soft|gt> <text>    AdsSeek with a string key, in the order
 *     locate <1|0> <expression>     AdsLocate forward or back
 *     custom <1|2|3> <n> ...        AdsCustomizeAOF add, remove or toggle
 *     inaof <n>                     AdsIsRecordInAOF
 *     !exact <0|1>
 *     !number <0|1>             leave out or record the record number with
 *                               filters respected, which moves a table on
 *                               a hidden record to EOF
 *
 * The state is the record number (ignoring filters), BOF, EOF, whether the
 * record is visible, and with filters respected: the record count, the
 * record number and, in an index order, the key number and key count. */
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
#define CALL(name, type, ...) ((U32(WINAPI *) type)F(name))(__VA_ARGS__)

/* ADS_RESPECTFILTERS, ADS_IGNOREFILTERS */
#define RESPECT_FILTERS 1
#define IGNORE_FILTERS 2

static U32 last_error(void) {
    U32 code = 0;
    U8 text[600];
    U16 len = sizeof text;
    CALL("AdsGetLastError", (U32 *, U8 *, U16 *), &code, text, &len);
    return code;
}

static void json_string(const char *s, size_t len) {
    putchar('"');
    for (size_t i = 0; i < len; i++) {
        unsigned char c = s[i];
        if (c == '"' || c == '\\') printf("\\%c", c);
        else if (c < 32 || c >= 127) printf("\\u%04x", c);
        else putchar(c);
    }
    putchar('"');
}

static void result(const char *name, U32 rc, U32 value) {
    if (rc) printf(", \"%s\": \"E%lu\"", name, rc);
    else printf(", \"%s\": %lu", name, value);
}

static int with_number = 1;

static void state(H table, H order) {
    U32 recno = 0, value = 0, rc;
    U16 flag = 0;
    CALL("AdsGetRecordNum", (H, U16, U32 *), table, IGNORE_FILTERS, &recno);
    printf(", \"recno\": %lu", recno);
    CALL("AdsAtBOF", (H, U16 *), table, &flag);
    printf(", \"bof\": %s", flag ? "true" : "false");
    CALL("AdsAtEOF", (H, U16 *), table, &flag);
    printf(", \"eof\": %s", flag ? "true" : "false");
    rc = CALL("AdsIsRecordVisible", (H, U16 *), table, &flag);
    result("visible", rc, flag);
    rc = CALL("AdsGetRecordCount", (H, U16, U32 *), table, RESPECT_FILTERS, &value);
    result("count", rc, value);
    if (with_number) {
        rc = CALL("AdsGetRecordNum", (H, U16, U32 *), table, RESPECT_FILTERS, &value);
        result("number", rc, value);
    }
    if (order) {
        rc = CALL("AdsGetKeyNum", (H, U16, U32 *), order, RESPECT_FILTERS, &value);
        result("key_number", rc, value);
        rc = CALL("AdsGetKeyCount", (H, U16, U32 *), order, RESPECT_FILTERS, &value);
        result("key_count", rc, value);
    }
}

int main(int argc, char **argv) {
    if (argc != 4) {
        fprintf(stderr, "usage: filters <ace32.dll> <data folder> <script>\n");
        return 2;
    }
    ace = LoadLibraryA(argv[1]);
    if (!ace) return fprintf(stderr, "cannot load %s\n", argv[1]), 1;
    FILE *script = fopen(argv[3], "r");
    if (!script) return fprintf(stderr, "cannot open %s\n", argv[3]), 1;
    CALL("AdsSetServerType", (U16), 1);
    H connection = 0, table = 0, order = 0;
    U32 rc = CALL("AdsConnect26", (U8 *, U16, H *), (U8 *)argv[2], 1, &connection);
    if (rc) return fprintf(stderr, "connect: %lu\n", rc), 1;

    char line[4096], name[1024] = "";
    int first = 1;
    printf("[\n");
    while (fgets(line, sizeof line, script)) {
        line[strcspn(line, "\r\n")] = 0;
        if (!*line || *line == '#') continue;
        if (!strncmp(line, "@table ", 7)) {
            if (table) CALL("AdsCloseTable", (H), table);
            snprintf(name, sizeof name, "%s", line + 7);
            char path[2048];
            snprintf(path, sizeof path, "%s\\%s", argv[2], name);
            for (char *p = path; *p; p++) if (*p == '/') *p = '\\';
            /* ADS_ADT, ADS_ANSI, ADS_PROPRIETARY_LOCKING, ADS_CHECKRIGHTS, ADS_READONLY */
            rc = CALL("AdsOpenTable", (H, U8 *, U8 *, U16, U16, U16, U16, U32, H *), connection,
                      (U8 *)path, NULL, 3, 1, 1, 1, 4, &table);
            if (rc) return fprintf(stderr, "open %s: %lu\n", path, rc), 1;
            order = 0;
            continue;
        }
        char *arg = strchr(line, ' ');
        arg = arg ? arg + 1 : line + strlen(line);
        H moving = order ? order : table;
        U16 flag = 0;
        int with_flag = 0;
        static U8 text[70000];
        U16 len = 65535;
        int with_text = 0;

        if (!strncmp(line, "order ", 6)) {
            order = 0;
            rc = strcmp(arg, "-") ? CALL("AdsGetIndexHandle", (H, U8 *, H *), table, (U8 *)arg, &order) : 0;
        } else if (!strcmp(line, "aof") || !strncmp(line, "aof ", 4)) {
            rc = CALL("AdsSetAOF", (H, U8 *, U16), table, (U8 *)arg, 1);
        } else if (!strcmp(line, "filter") || !strncmp(line, "filter ", 7)) {
            rc = CALL("AdsSetFilter", (H, U8 *), table, (U8 *)arg);
        } else if (!strcmp(line, "clearaof")) {
            rc = CALL("AdsClearAOF", (H), table);
        } else if (!strcmp(line, "clearfilter")) {
            rc = CALL("AdsClearFilter", (H), table);
        } else if (!strcmp(line, "getaof")) {
            rc = CALL("AdsGetAOF", (H, U8 *, U16 *), table, text, &len);
            with_text = 1;
        } else if (!strcmp(line, "getfilter")) {
            rc = CALL("AdsGetFilter", (H, U8 *, U16 *), table, text, &len);
            with_text = 1;
        } else if (!strcmp(line, "top")) {
            rc = CALL("AdsGotoTop", (H), moving);
        } else if (!strcmp(line, "bottom")) {
            rc = CALL("AdsGotoBottom", (H), moving);
        } else if (!strncmp(line, "skip ", 5)) {
            rc = CALL("AdsSkip", (H, long), moving, atol(arg));
        } else if (!strncmp(line, "goto ", 5)) {
            rc = CALL("AdsGotoRecord", (H, U32), table, strtoul(arg, NULL, 10));
        } else if (!strncmp(line, "seek ", 5)) {
            /* ADS_SOFTSEEK, ADS_HARDSEEK, ADS_SEEKGT; ADS_STRINGKEY */
            U16 how = !strncmp(arg, "soft", 4) ? 1 : !strncmp(arg, "hard", 4) ? 2 : 4;
            char *key = strchr(arg, ' ');
            key = key ? key + 1 : "";
            rc = CALL("AdsSeek", (H, U8 *, U16, U16, U16, U16 *), moving, (U8 *)key,
                      (U16)strlen(key), 2, how, &flag);
            with_flag = 1;
        } else if (!strncmp(line, "locate ", 7)) {
            U16 forward = (U16)atoi(arg);
            char *expr = strchr(arg, ' ');
            expr = expr ? expr + 1 : "";
            rc = CALL("AdsLocate", (H, U8 *, U16, U16 *), table, (U8 *)expr, forward, &flag);
            with_flag = 1;
        } else if (!strncmp(line, "custom ", 7)) {
            U16 option = (U16)strtoul(arg, &arg, 10);
            U32 records[64], count = 0;
            while (*arg && count < 64) {
                char *end;
                U32 n = strtoul(arg, &end, 10);
                if (end == arg) break;
                records[count++] = n;
                arg = end;
            }
            rc = CALL("AdsCustomizeAOF", (H, U32, U32 *, U16), table, count, records, option);
        } else if (!strncmp(line, "inaof ", 6)) {
            rc = CALL("AdsIsRecordInAOF", (H, U32, U16 *), table, strtoul(arg, NULL, 10), &flag);
            with_flag = 1;
        } else if (!strncmp(line, "!number ", 8)) {
            with_number = atoi(arg);
            rc = 0;
        } else if (!strncmp(line, "!exact ", 7)) {
            rc = CALL("AdsSetExact", (U16), (U16)atoi(arg));
        } else {
            return fprintf(stderr, "unknown command: %s\n", line), 1;
        }

        printf("%s{\"table\": ", first ? "" : ",\n");
        first = 0;
        json_string(name, strlen(name));
        printf(", \"command\": ");
        json_string(line, strlen(line));
        printf(", \"rc\": %lu", rc);
        if (rc) printf(", \"error\": %lu", last_error());
        if (with_flag && !rc) printf(", \"flag\": %s", flag ? "true" : "false");
        if (with_text && !rc) {
            printf(", \"text\": ");
            json_string((char *)text, len);
        }
        state(table, order);
        printf("}");
    }
    printf("\n]\n");
    return 0;
}
