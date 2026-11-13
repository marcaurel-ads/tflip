/* Records how an Advantage Client Engine converts the text a client gives in
 * filters and expressions against an OEM table, for fixtures/expr/oem.json.
 *
 *     oem.exe <ace32.dll> <data folder> <script>  > oem.json
 *
 * Built with i686-w64-mingw32-gcc and run under Wine. Text arguments are hex
 * (bytes, or UTF-16LE for the W and UTF-16 commands; "-" is empty), so that
 * the script is plain ASCII. Commands, each on the open table:
 *
 *     open <type> <file>        the table as an OEM table, relative to the
 *                               data folder; <type> is ADS_NTX 1 or ADS_CDX 2
 *     goto <n>
 *     eval <hex>                AdsEvalLogicalExpr
 *     weval <hex>               AdsEvalLogicalExprW
 *     str <hex>                 AdsEvalStringExpr (the bytes it returns)
 *     valid <hex>               AdsIsExprValid
 *     filter <hex>              AdsSetFilter, then the record count
 *     wfilter <hex>             AdsSetFilter100 with ADS_ENCODE_UTF16
 *     ufilter <hex>             AdsSetFilter100 with ADS_ENCODE_UTF8
 *     aof <hex>                 AdsSetAOF (resolved at once), then the count
 *     getfilter | getaof
 *     clear                     AdsClearFilter and AdsClearAOF
 *     inaof <n>                 AdsIsRecordInAOF
 *     locate <hex>              AdsLocate forward; then the record number
 *     index <file> <expr hex> <condition hex>
 *                               AdsCreateIndex (the tag is named after the
 *                               file), then the order's expression,
 *                               condition and key count, and the key of
 *                               record 197 */
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

static U32 last_error(void) {
    U32 code = 0;
    U8 text[600];
    U16 len = sizeof text;
    CALL("AdsGetLastError", (U32 *, U8 *, U16 *), &code, text, &len);
    return code;
}

static int unhex(const char *s, U8 *out) {
    int n = 0;
    if (strcmp(s, "-"))
        for (; s[0] && s[1]; s += 2) {
            unsigned v;
            sscanf(s, "%2x", &v);
            out[n++] = (U8)v;
        }
    out[n] = 0;
    out[n + 1] = 0;
    return n;
}

static void hex_out(const U8 *s, int len) {
    putchar('"');
    for (int i = 0; i < len; i++) printf("%02x", s[i]);
    putchar('"');
}

int main(int argc, char **argv) {
    if (argc != 4) return fprintf(stderr, "usage: oem <ace32.dll> <data folder> <script>\n"), 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) return fprintf(stderr, "cannot load %s\n", argv[1]), 1;
    FILE *script = fopen(argv[3], "r");
    if (!script) return fprintf(stderr, "cannot open %s\n", argv[3]), 1;
    CALL("AdsSetServerType", (U16), 1);
    H connection = 0, table = 0;
    U32 rc = CALL("AdsConnect26", (U8 *, U16, H *), (U8 *)argv[2], 1, &connection);
    if (rc) return fprintf(stderr, "connect: %lu\n", rc), 1;

    char line[4096];
    int first = 1;
    printf("[\n");
    while (fgets(line, sizeof line, script)) {
        line[strcspn(line, "\r\n")] = 0;
        if (!*line || *line == '#') continue;
        char cmd[32] = "", a[2048] = "", b[2048] = "", c[2048] = "";
        sscanf(line, "%31s %2047s %2047s %2047s", cmd, a, b, c);
        static U8 text[8192], out[8192];
        int with_flag = 0, with_text = 0, with_count = 0, with_recno = 0;
        U16 flag = 0, len = 4000;
        U32 n = 0;
        if (!strcmp(cmd, "open")) {
            if (table) CALL("AdsCloseTable", (H), table);
            table = 0;
            char path[2048];
            snprintf(path, sizeof path, "%s\\%s", argv[2], b);
            /* ADS_OEM */
            rc = CALL("AdsOpenTable", (H, U8 *, U8 *, U16, U16, U16, U16, U32, H *), connection,
                      (U8 *)path, NULL, (U16)atoi(a), 2, 0, 1, 1, &table);
            if (rc) return fprintf(stderr, "open %s: %lu\n", path, rc), 1;
            CALL("AdsGotoTop", (H), table);
            continue;
        }
        unhex(!strcmp(cmd, "index") ? b : a, text);
        if (!strcmp(cmd, "goto")) {
            rc = CALL("AdsGotoRecord", (H, U32), table, strtoul(a, NULL, 10));
        } else if (!strcmp(cmd, "eval")) {
            rc = CALL("AdsEvalLogicalExpr", (H, U8 *, U16 *), table, text, &flag);
            with_flag = 1;
        } else if (!strcmp(cmd, "weval")) {
            rc = CALL("AdsEvalLogicalExprW", (H, U8 *, U16 *), table, text, &flag);
            with_flag = 1;
        } else if (!strcmp(cmd, "str")) {
            rc = CALL("AdsEvalStringExpr", (H, U8 *, U8 *, U16 *), table, text, out, &len);
            with_text = 1;
        } else if (!strcmp(cmd, "valid")) {
            rc = CALL("AdsIsExprValid", (H, U8 *, U16 *), table, text, &flag);
            with_flag = 1;
        } else if (!strcmp(cmd, "filter")) {
            rc = CALL("AdsSetFilter", (H, U8 *), table, text);
            with_count = 1;
        } else if (!strcmp(cmd, "wfilter")) {
            rc = CALL("AdsSetFilter100", (H, void *, U32), table, text, 0x2000);
            with_count = 1;
        } else if (!strcmp(cmd, "ufilter")) {
            rc = CALL("AdsSetFilter100", (H, void *, U32), table, text, 0x4000);
            with_count = 1;
        } else if (!strcmp(cmd, "aof")) {
            rc = CALL("AdsSetAOF", (H, U8 *, U16), table, text, 1);
            with_count = 1;
        } else if (!strcmp(cmd, "getfilter")) {
            rc = CALL("AdsGetFilter", (H, U8 *, U16 *), table, out, &len);
            with_text = 1;
        } else if (!strcmp(cmd, "getaof")) {
            rc = CALL("AdsGetAOF", (H, U8 *, U16 *), table, out, &len);
            with_text = 1;
        } else if (!strcmp(cmd, "clear")) {
            CALL("AdsClearFilter", (H), table);
            rc = CALL("AdsClearAOF", (H), table);
            with_count = 1;
        } else if (!strcmp(cmd, "inaof")) {
            rc = CALL("AdsIsRecordInAOF", (H, U32, U16 *), table, strtoul(a, NULL, 10), &flag);
            with_flag = 1;
        } else if (!strcmp(cmd, "locate")) {
            rc = CALL("AdsLocate", (H, U8 *, U16, U16 *), table, text, 1, &flag);
            with_flag = with_recno = 1;
        } else if (!strcmp(cmd, "index")) {
            static U8 condition[2048];
            unhex(c, condition);
            H order = 0;
            rc = CALL("AdsCreateIndex", (H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, H *), table,
                      (U8 *)a, (U8 *)a, text, strcmp(c, "-") ? condition : NULL, NULL, 0, &order);
            if (!rc) {
                U16 elen = 2000;
                CALL("AdsGetIndexExpr", (H, U8 *, U16 *), order, out, &elen);
                printf("%s{\"command\": \"%s\", \"rc\": 0, \"expr\": ", first ? "" : ",\n", line);
                first = 0;
                hex_out(out, elen);
                elen = 2000;
                CALL("AdsGetIndexCondition", (H, U8 *, U16 *), order, out, &elen);
                printf(", \"condition\": ");
                hex_out(out, elen);
                CALL("AdsGotoRecord", (H, U32), table, 197);
                elen = 2000;
                CALL("AdsExtractKey", (H, U8 *, U16 *), order, out, &elen);
                printf(", \"key\": ");
                hex_out(out, elen);
                CALL("AdsGetKeyCount", (H, U16, U32 *), order, 1, &n);
                printf(", \"keys\": %lu", n);
                CALL("AdsCloseIndex", (H), order);
                H orders[8];
                U16 count = 8;
                if (!CALL("AdsOpenIndex", (H, U8 *, H *, U16 *), table, (U8 *)a, orders, &count)) {
                    elen = 2000;
                    CALL("AdsGetIndexExpr", (H, U8 *, U16 *), orders[0], out, &elen);
                    printf(", \"reopened\": ");
                    hex_out(out, elen);
                    elen = 2000;
                    CALL("AdsGetIndexCondition", (H, U8 *, U16 *), orders[0], out, &elen);
                    printf(", \"reopened_condition\": ");
                    hex_out(out, elen);
                    CALL("AdsCloseIndex", (H), orders[0]);
                }
                printf("}");
                continue;
            }
        } else {
            return fprintf(stderr, "unknown command: %s\n", line), 1;
        }
        printf("%s{\"command\": \"%s\", \"rc\": %lu", first ? "" : ",\n", line, rc);
        first = 0;
        if (rc) printf(", \"error\": %lu", last_error());
        if (with_flag && !rc) printf(", \"flag\": %s", flag ? "true" : "false");
        if (with_text && !rc) {
            printf(", \"bytes\": ");
            hex_out(out, len);
        }
        if (with_count) {
            CALL("AdsGotoTop", (H), table);
            CALL("AdsGetRecordCount", (H, U16, U32 *), table, 1, &n);
            printf(", \"count\": %lu", n);
            CALL("AdsGetRecordNum", (H, U16, U32 *), table, 2, &n);
            printf(", \"first\": %lu", n);
            U8 stored[16] = "";
            U16 stored_len = sizeof stored;
            if (!CALL("AdsEvalStringExpr", (H, U8 *, U8 *, U16 *), table, (U8 *)"C", stored,
                      &stored_len)) {
                printf(", \"c\": ");
                hex_out(stored, stored_len);
            }
        }
        if (with_recno) {
            CALL("AdsGetRecordNum", (H, U16, U32 *), table, 2, &n);
            printf(", \"recno\": %lu", n);
        }
        printf("}");
    }
    printf("\n]\n");
    return 0;
}
