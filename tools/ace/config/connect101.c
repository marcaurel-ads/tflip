/* What the options of AdsConnect101's connect string do, and what
 * AdsOpenTable101 takes from them:
 *
 *     connect101.exe <ace32.dll> <data folder> <connect string suffix> <table>...
 *
 * Built with i686-w64-mingw32-gcc, run under Wine by run_connect101.sh on a
 * folder of tables (see there). Connects with "Data Source=<folder>;
 * ServerType=LOCAL;<suffix>" and prints one JSON object: the connect's return
 * code and error text, the connection's settings (AdsGetCollation,
 * AdsGetDateFormat60, AdsGetEpoch, AdsGetDecimals, AdsGetExact22), then for each
 * table AdsOpenTable101's return code and error text and, if it opened, its
 * type, character type, lock type, open options, record count (filters
 * ignored, with deleted records counted unless the connection hides them), the
 * return code of appending a record, and of a second open of the same table
 * from another connection with ADS_EXCLUSIVE (which tells whether the first
 * is open shared). With PREP set in the environment it deletes the second
 * record of each table instead (AdsConnect26), for the counts to tell whether
 * deleted records show. */
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

static void json_text(const U8 *text, int len) {
    putchar('"');
    for (int i = 0; i < len; i++) {
        if (text[i] == '\\' || text[i] == '"') putchar('\\');
        if (text[i] >= 0x20 && text[i] < 0x7f) putchar(text[i]);
    }
    putchar('"');
}

static void last_error(void) {
    U32 code = 0;
    U8 text[600];
    U16 len = sizeof text;
    ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
    printf(", \"error\": %lu, \"text\": ", code);
    json_text(text, len);
}

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IONBF, 0);
    if (argc < 4) return fprintf(stderr, "usage: see the header\n"), 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) return fprintf(stderr, "cannot load %s\n", argv[1]), 1;
    if (getenv("PREP")) {
        H connection = 0;
        ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
        if (((U32(WINAPI *)(U8 *, U16, H *))F("AdsConnect26"))((U8 *)argv[2], 1, &connection))
            return fprintf(stderr, "connect failed\n"), 1;
        for (int i = 4; i < argc; i++) {
            char path[1200];
            H table = 0;
            snprintf(path, sizeof path, "%s\\%s", argv[2], argv[i]);
            /* ADT for .adt, else CDX, ANSI */
            U16 type = strstr(argv[i], ".adt") ? 3 : 2;
            if (((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
                    connection, (U8 *)path, NULL, type, 1, 1, 2, 1, &table))
                continue;
            ((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(table, 2);
            ((U32(WINAPI *)(H))F("AdsDeleteRecord"))(table);
            ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
        }
        return 0;
    }
    char string[2048];
    snprintf(string, sizeof string, "Data Source=%s;ServerType=LOCAL;%s", argv[2], argv[3]);
    H connection = 0, options = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, H *, H *))F("AdsConnect101"))((U8 *)string, &options,
                                                                  &connection);
    printf("{\"connect\": %lu, \"options\": %s", rc, options ? "true" : "false");
    if (rc) {
        last_error();
        printf("}\n");
        return 0;
    }
    U8 text[200];
    U16 len = sizeof text;
    rc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsGetCollation"))(connection, text, &len);
    printf(", \"collation\": ");
    json_text(text, rc ? 0 : len);
    len = sizeof text;
    rc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsGetDateFormat60"))(connection, text, &len);
    printf(", \"date_format\": ");
    json_text(text, rc ? 0 : len);
    U16 value = 99;
    ((U32(WINAPI *)(U16 *))F("AdsGetEpoch"))(&value);
    printf(", \"epoch\": %u", value);
    value = 99;
    ((U32(WINAPI *)(U16 *))F("AdsGetDecimals"))(&value);
    printf(", \"decimals\": %u", value);
    value = 99;
    rc = ((U32(WINAPI *)(H, U16 *))F("AdsGetExact22"))(connection, &value);
    printf(", \"exact\": [%lu, %u]", rc, value);
    printf(", \"tables\": {");
    for (int i = 4; i < argc; i++) {
        H table = 0;
        rc = ((U32(WINAPI *)(H, U8 *, H *))F("AdsOpenTable101"))(connection, (U8 *)argv[i],
                                                                  &table);
        printf("%s\"%s\": {\"open\": %lu", i > 4 ? ", " : "", argv[i], rc);
        if (rc) {
            last_error();
        } else {
            U16 type = 99, chars = 99, lock = 99;
            U32 opened = 0, count = 0;
            ((U32(WINAPI *)(H, U16 *))F("AdsGetTableType"))(table, &type);
            ((U32(WINAPI *)(H, U16 *))F("AdsGetTableCharType"))(table, &chars);
            ((U32(WINAPI *)(H, U16 *))F("AdsGetTableLockType"))(table, &lock);
            ((U32(WINAPI *)(H, U32 *))F("AdsGetTableOpenOptions"))(table, &opened);
            ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(table, 2, &count);
            printf(", \"type\": %u, \"char_type\": %u, \"lock_type\": %u, \"open_options\": %lu, "
                   "\"count\": %lu",
                   type, chars, lock, opened, count);
            /* Record 2 was deleted (see PREP): whether it can be gone to, a date
             * given as text (the epoch and the date format) and a text field
             * read back (trailing spaces). */
            int adt = strstr(argv[i], ".adt") != NULL;
            printf(", \"goto_deleted\": %lu", ((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(table, 2));
            ((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(table, 1);
            {
                const char *date = adt ? "born" : "C_DATE", *text = adt ? "name" : "C_CHAR";
                U8 got[100];
                U32 length = sizeof got;
                U32 src = ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetString"))(
                    table, (U8 *)date, (U8 *)"01/02/50", 8);
                length = sizeof got;
                U32 grc = ((U32(WINAPI *)(H, U8 *, U8 *, U32 *, U16))F("AdsGetString"))(
                    table, (U8 *)date, got, &length, 0);
                printf(", \"date\": [%lu, %lu, ", src, grc);
                json_text(got, grc ? 0 : (int)length);
                src = ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetString"))(
                    table, (U8 *)text, (U8 *)"ab  ", 4);
                length = sizeof got;
                grc = ((U32(WINAPI *)(H, U8 *, U8 *, U32 *, U16))F("AdsGetString"))(
                    table, (U8 *)text, got, &length, 0);
                printf("], \"text\": [%lu, %lu, ", src, grc);
                json_text(got, grc ? 0 : (int)length);
                printf("]");
            }
            U32 arc = ((U32(WINAPI *)(H))F("AdsAppendRecord"))(table);
            printf(", \"append\": %lu", arc);
            if (arc) {
                U32 code = 0;
                U8 message[600];
                U16 length = sizeof message;
                ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, message, &length);
                printf(", \"append_error\": %lu", code);
            }
            H other = 0, held = 0;
            ((U32(WINAPI *)(U8 *, U16, H *))F("AdsConnect26"))((U8 *)argv[2], 1, &other);
            char path[1200];
            snprintf(path, sizeof path, "%s\\%s", argv[2], argv[i]);
            U32 erc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F(
                "AdsOpenTable"))(other, (U8 *)path, NULL, type == 1 || type == 2 || type == 3
                                                               ? type
                                                               : 3,
                                 chars ? chars : 1, 1, 2, 1, &held);
            printf(", \"exclusive_elsewhere\": %lu", erc);
            ((U32(WINAPI *)(H))F("AdsDisconnect"))(other);
            ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
        }
        printf("}");
    }
    printf("}}\n");
    return 0;
}
