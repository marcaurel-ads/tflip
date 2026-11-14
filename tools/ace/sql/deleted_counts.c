/* Records what SQL statements of an Advantage Client Engine read of tables
 * with deleted records, and how they fail on a table of another type than
 * theirs, for fixtures/sql/deleted_counts.json.
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o deleted_counts.exe deleted_counts.c
 *     cd <empty folder> && wine deleted_counts.exe <ace32.dll> <the folder as a Windows path>
 *
 * The original resolves relative table names against the current directory,
 * so the folder must be the current one too. For each table type (NTX, CDX,
 * ADT, VFP) and each table type a statement is set to with
 * AdsStmtSetTableType (0 for none set, then 1 to 4), the probe makes the
 * table d<type>_<statement's>.<dbf or adt> of the fields ID (Integer, N(4)
 * for the DBF types) and PAD (Char 200, so that a DBF file is longer than
 * an ADT header) with the IDs 1 to 4, deletes records 1 and 3 and closes
 * it, then makes a statement of that type and, with AdsShowDeleted(1) and
 * then (0), runs
 *
 *     SELECT COUNT(*) AS n FROM "<name>"
 *     SELECT COUNT(*) AS n FROM "<name>" WHERE ID >= 1
 *     SELECT ID FROM "<name>" ORDER BY ID DESC
 *
 * Each row is {"type", "stmt", "show", "sql", "rc", "error", "rows",
 * "first"}: the AdsExecuteSQLDirect result and the text AdsGetLastError
 * gives for it ("" where it succeeded; the folder in it written as
 * <folder>), how many rows AdsGotoTop and AdsSkip walk in the cursor and the
 * first's first field as text ("" where none). */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;
static HMODULE ace;
#define F(name) ((void *)GetProcAddress(ace, name))

/* `len` bytes at `text` as a JSON string, `folder` in it as <folder>. */
static void string(const U8 *text, size_t len, const char *folder) {
    size_t skip = strlen(folder);
    putchar('"');
    for (size_t i = 0; i < len && text[i]; i++) {
        if (skip && i + skip <= len && !_strnicmp((const char *)text + i, folder, skip)) {
            printf("<folder>");
            i += skip - 1;
        } else if (text[i] == '"' || text[i] == '\\') printf("\\%c", text[i]);
        else if (text[i] < 0x20) printf("\\u%04x", text[i]);
        else putchar(text[i]);
    }
    putchar('"');
}

int main(int argc, char **argv) {
    if (argc != 3 || !argv[2][0]) return 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) return 1;
    H conn = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
        (U8 *)argv[2], 1, NULL, NULL, 0, &conn);
    if (rc) return printf("connect: %lu\n", rc), 1;
    char folder[MAX_PATH];
    snprintf(folder, sizeof folder, "%s", argv[2]);
    for (size_t n = strlen(folder); n && folder[n - 1] == '\\'; n--) folder[n - 1] = 0;
    static const struct { const char *name, *ext, *fields; U16 type; } types[] = {
        {"NTX", "dbf", "ID,N,4,0;PAD,C,200,0", 1},
        {"CDX", "dbf", "ID,N,4,0;PAD,C,200,0", 2},
        {"ADT", "adt", "ID,Integer,4;PAD,Char,200", 3},
        {"VFP", "dbf", "ID,N,4,0;PAD,C,200,0", 4}};
    static const char *queries[] = {"SELECT COUNT(*) AS n FROM \"%s\"",
                                    "SELECT COUNT(*) AS n FROM \"%s\" WHERE ID >= 1",
                                    "SELECT ID FROM \"%s\" ORDER BY ID DESC"};
    int first = 1;
    printf("[\n");
    for (int t = 0; t < 4; t++) {
        for (U16 type = 0; type <= 4; type++) {
            char name[16];
            H table = 0, stmt = 0;
            snprintf(name, sizeof name, "d%u_%u.%s", types[t].type, type, types[t].ext);
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *))F(
                "AdsCreateTable"))(conn, (U8 *)name, NULL, types[t].type, 1, 0, 1, 0,
                                   (U8 *)types[t].fields, &table);
            if (rc) return printf("create %s: %lu\n", name, rc), 1;
            for (int r = 1; r <= 4; r++) {
                ((U32(WINAPI *)(H))F("AdsAppendRecord"))(table);
                ((U32(WINAPI *)(H, U8 *, double))F("AdsSetDouble"))(table, (U8 *)"ID", r);
                ((U32(WINAPI *)(H))F("AdsWriteRecord"))(table);
            }
            for (U32 r = 1; r <= 3; r += 2) {
                ((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(table, r);
                ((U32(WINAPI *)(H))F("AdsDeleteRecord"))(table);
            }
            ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
            ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(conn, &stmt);
            if (type) ((U32(WINAPI *)(H, U16))F("AdsStmtSetTableType"))(stmt, type);
            for (int show = 1; show >= 0; show--) {
                ((U32(WINAPI *)(U16))F("AdsShowDeleted"))((U16)show);
                for (int q = 0; q < 3; q++) {
                    char sql[96];
                    H cursor = 0;
                    U32 rows = 0, len = 32, code = 0;
                    U8 text[32] = "", error[600] = "";
                    U16 error_len = sizeof error;
                    snprintf(sql, sizeof sql, queries[q], name);
                    rc = ((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(
                        stmt, (U8 *)sql, &cursor);
                    if (rc == 0 || ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(
                                       &code, error, &error_len))
                        error_len = 0;
                    if (rc == 0 && cursor) {
                        ((U32(WINAPI *)(H))F("AdsGotoTop"))(cursor);
                        if (((U32(WINAPI *)(H, U8 *, U8 *, U32 *, U16))F("AdsGetField"))(
                                cursor, (U8 *)(ULONG_PTR)1, text, &len, 0))
                            len = 0;
                        for (int i = 0; i < 10; i++) {
                            U16 eof = 1;
                            if (((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(cursor, &eof) || eof)
                                break;
                            rows++;
                            ((U32(WINAPI *)(H, U32))F("AdsSkip"))(cursor, 1);
                        }
                        ((U32(WINAPI *)(H))F("AdsCloseTable"))(cursor);
                    } else len = 0;
                    while (len && text[len - 1] == ' ') len--;
                    printf("%s{\"type\": \"%s\", \"stmt\": %u, \"show\": %d, \"sql\": ",
                           first ? "" : ",\n", types[t].name, type, show);
                    string((U8 *)sql, strlen(sql), "");
                    printf(", \"rc\": %lu, \"error\": ", rc);
                    string(error, error_len, folder);
                    printf(", \"rows\": %lu, \"first\": ", rows);
                    string(text, len, "");
                    printf("}");
                    first = 0;
                }
            }
            ((U32(WINAPI *)(U16))F("AdsShowDeleted"))(1);
            ((U32(WINAPI *)(H))F("AdsCloseSQLStatement"))(stmt);
        }
    }
    printf("\n]\n");
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
    return 0;
}
