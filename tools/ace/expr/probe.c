/* Records what an Advantage Client Engine makes of expressions, for
 * fixtures/expr/expressions.json.
 *
 *     probe.exe <ace32.dll> <data folder> <cases file>  > expressions.json
 *
 * Built with i686-w64-mingw32-gcc and run under Wine. The cases file has one
 * expression per line, evaluated on every record of the current table and at
 * EOF. Other lines:
 *
 *     # comment
 *     @table <file>      open <file> (relative to the data folder)
 *     !exact <0|1>       AdsSetExact
 *     !date <format>     AdsSetDateFormat
 *     ?<expression>      only checked and typed: its value changes (DATE())
 *
 * Every result is a string: "E<code>" for an error, "T"/"F" for
 * AdsEvalLogicalExpr, the double (%.17g) for AdsEvalNumericExpr, and the
 * bytes in hex for AdsEvalStringExpr. */
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

static U32 last_error(void) {
    U32 code = 0;
    U8 text[600];
    U16 len = sizeof text;
    ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
    return code;
}

static void json_string(const char *s) {
    putchar('"');
    for (; *s; s++) {
        unsigned char c = *s;
        if (c == '"' || c == '\\') printf("\\%c", c);
        else if (c < 32 || c >= 127) printf("\\u%04x", c);
        else putchar(c);
    }
    putchar('"');
}

static void results(H table, const char *expr) {
    U16 b = 0;
    U32 rc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsEvalLogicalExpr"))(table, (U8 *)expr, &b);
    if (rc) printf("\"L\": \"E%lu\", ", rc);
    else printf("\"L\": \"%s\", ", b ? "T" : "F");
    double d = 0;
    rc = ((U32(WINAPI *)(H, U8 *, double *))F("AdsEvalNumericExpr"))(table, (U8 *)expr, &d);
    if (rc) printf("\"N\": \"E%lu\", ", rc);
    else printf("\"N\": \"%.17g\", ", d);
    static U8 s[70000];
    U16 len = 65535;
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16 *))F("AdsEvalStringExpr"))(table, (U8 *)expr, s, &len);
    if (rc) printf("\"S\": \"E%lu\"", rc);
    else {
        printf("\"S\": \"");
        for (U16 i = 0; i < len; i++) printf("%02x", s[i]);
        putchar('"');
    }
}

int main(int argc, char **argv) {
    if (argc != 4) {
        fprintf(stderr, "usage: probe <ace32.dll> <data folder> <cases>\n");
        return 2;
    }
    ace = LoadLibraryA(argv[1]);
    if (!ace) return fprintf(stderr, "cannot load %s\n", argv[1]), 1;
    FILE *cases = fopen(argv[3], "r");
    if (!cases) return fprintf(stderr, "cannot open %s\n", argv[3]), 1;
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    H connection = 0, table = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, H *))F("AdsConnect26"))((U8 *)argv[2], 1, &connection);
    if (rc) return fprintf(stderr, "connect: %lu\n", rc), 1;

    char line[4096], name[1024] = "", date_format[64] = "MM/DD/CCYY";
    int exact = 0, first = 1;
    printf("[\n");
    while (fgets(line, sizeof line, cases)) {
        line[strcspn(line, "\r\n")] = 0;
        if (!*line || *line == '#') continue;
        if (!strncmp(line, "@table ", 7)) {
            if (table) ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
            snprintf(name, sizeof name, "%s", line + 7);
            char path[2048];
            snprintf(path, sizeof path, "%s\\%s", argv[2], name);
            for (char *p = path; *p; p++) if (*p == '/') *p = '\\';
            typedef U32(WINAPI * Open)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *);
            /* ADS_ADT, ADS_ANSI, ADS_PROPRIETARY_LOCKING, ADS_CHECKRIGHTS, ADS_READONLY */
            rc = ((Open)F("AdsOpenTable"))(connection, (U8 *)path, NULL, 3, 1, 1, 1, 4, &table);
            if (rc) return fprintf(stderr, "open %s: %lu\n", path, rc), 1;
            continue;
        }
        if (!strncmp(line, "!exact ", 7)) {
            exact = atoi(line + 7);
            ((U32(WINAPI *)(U16))F("AdsSetExact"))(exact);
            continue;
        }
        if (!strncmp(line, "!date ", 6)) {
            snprintf(date_format, sizeof date_format, "%s", line + 6);
            ((U32(WINAPI *)(U8 *))F("AdsSetDateFormat"))((U8 *)date_format);
            continue;
        }
        int volatile_ = *line == '?';
        const char *expr = line + volatile_;

        printf("%s{\"table\": ", first ? "" : ",\n");
        first = 0;
        json_string(name);
        printf(", \"expr\": ");
        json_string(expr);
        printf(", \"exact\": %s, \"date_format\": ", exact ? "true" : "false");
        json_string(date_format);
        U16 valid = 0;
        rc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsIsExprValid"))(table, (U8 *)expr, &valid);
        printf(", \"valid\": \"%s\"", rc ? "E" : valid ? "T" : "F");
        printf(", \"valid_error\": %lu", rc ? rc : valid ? 0 : last_error());
        U16 type = 0;
        rc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsEvalTestExpr"))(table, (U8 *)expr, &type);
        if (rc) printf(", \"type\": \"E%lu\"", rc);
        else printf(", \"type\": %u", type);
        printf(", \"records\": [");
        if (!volatile_ && !rc) {
            ((U32(WINAPI *)(H))F("AdsGotoTop"))(table);
            for (int n = 0;; n++) {
                U16 eof = 0;
                ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(table, &eof);
                U32 recno = 0;
                /* ADS_IGNOREFILTERS */
                ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordNum"))(table, 2, &recno);
                printf("%s\n  {\"recno\": %lu, \"eof\": %s, ", n ? "," : "", recno,
                       eof ? "true" : "false");
                results(table, expr);
                putchar('}');
                if (eof) break;
                ((U32(WINAPI *)(H, U32))F("AdsSkip"))(table, 1);
            }
        }
        printf("]}");
    }
    printf("\n]\n");
    return 0;
}
