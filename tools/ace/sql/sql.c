/* Records what an Advantage Client Engine does with SQL statements, for
 * fixtures/sql/statements.json.
 *
 *     sql.exe <ace32.dll> <data folder> <script>  > statements.json
 *
 * Built with i686-w64-mingw32-gcc and run under Wine. The data folder is
 * connected to as a free-table folder (local server); the script has one
 * command per line, and each one's result is a JSON object:
 *
 *     # comment
 *     stmt                      AdsCreateSQLStatement (closing nothing)
 *     direct <sql>              AdsExecuteSQLDirect, with a cursor
 *     query <sql>               direct, then AdsCloseTable on the cursor
 *     expr <expression>         query SELECT <expression> FROM system.iota
 *     directnull <sql>          AdsExecuteSQLDirect, with no cursor pointer
 *     prepare <sql>             AdsPrepareSQL
 *     verify <sql>              AdsVerifySQL
 *     exec                      AdsExecuteSQL
 *     setprop <stmt|conn|cursor|0|n> <id> <v>
 *                               AdsSetProperty (unpublished) on the statement,
 *                               the connection, handle 0 or the handle n, with
 *                               the value v
 *     getprop <stmt|conn|cursor|0|n> <id>
 *                               AdsSetProperty with a pointer to an UNSIGNED32
 *                               (0xdeadbeef) as the value, which is printed
 *                               after the call (the getter form of id 87)
 *     param <n|name> <kind> [v] AdsSet<kind> (string long short double
 *                               logical date money null empty) on the
 *                               statement; a number is a parameter number
 *     clearparams               AdsClearSQLParams
 *     numparams                 AdsGetNumParams ("n")
 *     count                     AdsGetRecordCount on the statement
 *     closestmt                 AdsCloseSQLStatement
 *     closecursor               AdsCloseTable on the last cursor
 *     setstring|setfield <n> <text>, write   AdsSetString / AdsSetField, AdsWriteRecord on it
 *     keep <name>               AdsOpenTable, kept as the cursor edit/append/delrec/row use
 *     info                      handle types, AdsGetSQLStatementHandle,
 *                               AdsGetSQLStatement, filters of the cursor
 *     stmtparams <sql>          AdsGetSQLStmtParams on a copy of <sql>
 *     peek [stmt|table|0]       AdsSqlPeekStatement on the last cursor, or on
 *                               the statement, a table opened on the
 *                               connection, or handle 0
 *     tabletype <n>             AdsStmtSetTableType (1 ntx, 2 cdx, 3 adt, 4
 *                               vfp)
 *     chartype <n>              AdsStmtSetTableCharType (1 ansi, 2 oem)
 *     locktype <n>              AdsStmtSetTableLockType
 *     readonly <n>              AdsStmtSetTableReadOnly
 *     rights <n>                AdsStmtSetTableRights
 *     collation <name>          AdsStmtSetTableCollation
 *     setcollation <name>       AdsSetCollation on the connection
 *     getcollation              AdsGetCollation of the connection
 *     connect <dd> [user] [pw]  AdsConnect60 to the data dictionary <dd>, a
 *                               path in the data folder, which the commands
 *                               after it use (statements of the connection
 *                               before stay open)
 *     ddcreate <dd>             AdsDDCreate of the data dictionary <dd>, a
 *                               path in the data folder (not encrypted, no
 *                               description), whose connection the
 *                               commands after it use
 *     cachecursors <n>          AdsCacheOpenCursors
 *     sleep <ms>                Sleep, for the probes of what depends on the clock
 *     cap <n>                   how many rows of a cursor are recorded (200 to begin with)
 *     txcount                   AdsGetTransactionCount on the connection
 *     ddview <name> <sql>       AdsDDAddView on the connection
 *     dduser <name> <password>  AdsDDCreateUser on the connection, in no group
 *     dddbprop <id> <length> <text>
 *                               AdsDDSetDatabaseProperty of the property <id>
 *                               with the bytes of <text> and a length of <length>
 *                               (-1 for the text and its NUL); a text of - is
 *                               no bytes
 *     ddlink <alias> <path> <user> <password> <options>
 *                               AdsDDCreateLink; a user or password of -
 *                               is none (NULL)
 *     ddpub <name> <comment> <options>
 *     ddart <publication> <table> <identifying columns> <options>
 *     ddsub <name> <publication> <target> <user> <password> <queue> <forward>
 *           <comment> <options>
 *     ddpubcomment <name> <text>, ddsubcomment <name> <text>
 *     dddelpub <name>, dddelsub <name>
 *                               AdsDDCreatePublication, AdsDDCreateArticle,
 *                               AdsDDCreateSubscription, the comment (property
 *                               1) of either, and their deletes; a word of - is
 *                               none (NULL) and ~ stands for a space
 *     droplink <alias> <global> AdsDDDropLink
 *     numlinks                  AdsGetNumActiveLinks
 *     linkinfo <n> <size>       AdsGetActiveLinkInfo into a <size>-byte buffer
 *                               (the length written back counting the data
 *                               folder as the 6 bytes of <data>)
 *     find62 [mask]             the whole AdsFindFirstTable62 listing
 *     find [mask]               the whole AdsFindFirstTable listing
 *     objects <type> [parent]   the whole AdsDDFindFirstObject listing
 *     otype <type> <name>       AdsOpenTable of <name> as given (a #name too) as
 *                               table type <type>, recorded as a cursor
 *     ctable <type> <name> <fields>
 *                               AdsCreateTable of <name>, the table closed again
 *     open <name>               AdsOpenTable on the connection, recorded as
 *                               a cursor, then closed
 *     transfree <type> <name>   AdsOpenTable as table type <type> (0 on a
 *                               dictionary connection, else the table in the
 *                               data folder), and its
 *                               AdsIsTableTransactionFree
 *     disconnect                AdsDisconnect of the connection
 *
 * In <sql>, \r, \n and \t stand for those characters, and <data> for the data
 * folder.
 *
 * A cursor is recorded in full: file name, open options, table type,
 * indexes, record count, fields (name, type, length, decimals) and every
 * record (record number and each field's AdsGetField text, null as null).
 * "error" is AdsGetLastError's text when a call fails. The data folder is
 * written as <data> wherever it appears. */
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
#define FIELD(n) ((U8 *)(ULONG_PTR)(n))

/* The data folder, written as <data> wherever it appears in results. */
static const char *data_folder;

static void json_bytes(const U8 *s, size_t n) {
    size_t folder = strlen(data_folder);
    putchar('"');
    for (size_t i = 0; i < n; i++) {
        if (folder && i + folder <= n && !_strnicmp((const char *)s + i, data_folder, folder)) {
            printf("<data>");
            i += folder - 1;
            continue;
        }
        U8 c = s[i];
        if (c == '"' || c == '\\') printf("\\%c", c);
        else if (c < 32 || c >= 127) printf("\\u%04x", c);
        else putchar(c);
    }
    putchar('"');
}

static void json_string(const char *s) { json_bytes((const U8 *)s, strlen(s)); }

static void result(U32 rc) {
    printf("\"rc\": %lu", rc);
    if (rc) {
        U32 code = 0;
        U8 text[1200];
        U16 len = sizeof text;
        ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
        printf(", \"error\": ");
        json_bytes(text, len);
    }
}

/* How many rows of a cursor are dumped; the script's `cap` command changes it. */
static int dump_cap = 200;

static void cursor(H c) {
    U8 buf[70000];
    U16 len = 1024;
    U32 rc = ((U32(WINAPI *)(H, U16, U8 *, U16 *))F("AdsGetTableFilename"))(c, 2, buf, &len);
    printf(", \"cursor\": {\"file\": ");
    if (rc) printf("\"E%lu\"", rc);
    else json_bytes(buf, len);
    U32 options = 0;
    ((U32(WINAPI *)(H, U32 *))F("AdsGetTableOpenOptions"))(c, &options);
    U16 type = 0, indexes = 0, kind = 0, fields = 0;
    ((U32(WINAPI *)(H, U16 *))F("AdsGetTableType"))(c, &type);
    ((U32(WINAPI *)(H, U16 *))F("AdsGetNumIndexes"))(c, &indexes);
    ((U32(WINAPI *)(H, U16 *))F("AdsGetHandleType"))(c, &kind);
    U32 count = 0;
    rc = ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(c, 2, &count);
    printf(", \"options\": %lu, \"table_type\": %u, \"handle_type\": %u, \"indexes\": %u, "
           "\"count\": %lu",
           options, type, kind, indexes, rc ? 0 : count);
    ((U32(WINAPI *)(H, U16 *))F("AdsGetNumFields"))(c, &fields);
    printf(", \"fields\": [");
    for (U16 f = 1; f <= fields; f++) {
        U8 name[260];
        U16 nlen = sizeof name;
        ((U32(WINAPI *)(H, U16, U8 *, U16 *))F("AdsGetFieldName"))(c, f, name, &nlen);
        U16 ftype = 0, dec = 0;
        U32 flen = 0;
        ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsGetFieldType"))(c, FIELD(f), &ftype);
        ((U32(WINAPI *)(H, U8 *, U32 *))F("AdsGetFieldLength"))(c, FIELD(f), &flen);
        ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsGetFieldDecimals"))(c, FIELD(f), &dec);
        printf("%s[", f > 1 ? ", " : "");
        json_bytes(name, nlen);
        printf(", %u, %lu, %u]", ftype, flen, dec);
    }
    printf("], \"rows\": [");
    ((U32(WINAPI *)(H))F("AdsGotoTop"))(c);
    for (int n = 0; n < dump_cap; n++) {
        U16 eof = 0;
        ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(c, &eof);
        if (eof) break;
        U32 recno = 0;
        ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordNum"))(c, 2, &recno);
        printf("%s\n  [%lu, [", n ? "," : "", recno);
        for (U16 f = 1; f <= fields; f++) {
            if (f > 1) printf(", ");
            U16 null = 0;
            ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsIsNull"))(c, FIELD(f), &null);
            if (null) {
                printf("null");
                continue;
            }
            U32 flen = sizeof buf;
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U32 *, U16))F("AdsGetField"))(c, FIELD(f), buf,
                                                                              &flen, 0);
            if (rc) printf("\"E%lu\"", rc);
            else json_bytes(buf, flen);
        }
        printf("]]");
        ((U32(WINAPI *)(H, U32))F("AdsSkip"))(c, 1);
    }
    printf("]}");
}

/* Replaces \r, \n and \t in s by those characters. */
static void unescape(char *s) {
    char *out = s;
    for (; *s; s++) {
        if (*s == '\\' && (s[1] == 'r' || s[1] == 'n' || s[1] == 't')) {
            s++;
            *out++ = *s == 'r' ? '\r' : *s == 'n' ? '\n' : '\t';
        } else *out++ = *s;
    }
    *out = 0;
}

/* The next space-separated word of *s, which moves past it. */
static char *word(char **s) {
    while (**s == ' ') (*s)++;
    char *w = *s;
    while (**s && **s != ' ') (*s)++;
    if (**s) *(*s)++ = 0;
    return w;
}

int main(int argc, char **argv) {
    if (argc != 4) {
        fprintf(stderr, "usage: sql <ace32.dll> <data folder> <script>\n");
        return 2;
    }
    data_folder = argv[2];
    ace = LoadLibraryA(argv[1]);
    if (!ace) return fprintf(stderr, "cannot load %s\n", argv[1]), 1;
    FILE *script = fopen(argv[3], "r");
    if (!script) return fprintf(stderr, "cannot open %s\n", argv[3]), 1;
    // SQL_SERVER_TYPE=2 connects to a remote server instead, over TCP/IP
    // (the data folder then a \\host:port\share path).
    U16 server = getenv("SQL_SERVER_TYPE") ? (U16)atoi(getenv("SQL_SERVER_TYPE")) : 1;
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(server);
    H connection = 0, stmt = 0, cur = 0;
    U32 rc;
    if (server == 1) {
        rc = ((U32(WINAPI *)(U8 *, U16, H *))F("AdsConnect26"))((U8 *)argv[2], 1, &connection);
    } else {
        char cs[2048];
        snprintf(cs, sizeof cs, "Data Source=%s;ServerType=REMOTE;CommType=TCP_IP", argv[2]);
        rc = ((U32(WINAPI *)(U8 *, H *, H *))F("AdsConnect101"))((U8 *)cs, NULL, &connection);
    }
    if (rc) return fprintf(stderr, "connect: %lu\n", rc), 1;

    static char line[70000];
    int first = 1;
    printf("[\n");
    while (fgets(line, sizeof line, script)) {
        line[strcspn(line, "\r\n")] = 0;
        if (!*line || *line == '#') continue;
        printf("%s{\"cmd\": ", first ? "" : ",\n");
        first = 0;
        json_string(line);
        printf(", ");
        char *rest = line;
        char *cmd = word(&rest);
        unescape(rest);
        // <data> is the data folder, put in after the escapes are read (its
        // path has backslashes of its own, such as the \t of Z:\tmp).
        for (char *at; (at = strstr(rest, "<data>")) != NULL;) {
            size_t folder = strlen(data_folder);
            memmove(at + folder, at + 6, strlen(at + 6) + 1);
            memcpy(at, data_folder, folder);
        }
        int query = !strcmp(cmd, "query") || !strcmp(cmd, "expr");
        static char select[70100];
        if (!strcmp(cmd, "expr")) {
            snprintf(select, sizeof select, "SELECT %s FROM system.iota", rest);
            rest = select;
        }
        if (query) cmd = "direct";
        if (!strcmp(cmd, "stmt")) {
            rc = ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(connection, &stmt);
            result(rc);
        } else if (!strcmp(cmd, "direct") || !strcmp(cmd, "exec")) {
            H fresh = 0;
            if (*cmd == 'd')
                rc = ((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(stmt, (U8 *)rest,
                                                                            &fresh);
            else rc = ((U32(WINAPI *)(H, H *))F("AdsExecuteSQL"))(stmt, &fresh);
            result(rc);
            printf(", \"has_cursor\": %s", fresh ? "true" : "false");
            /* A failed execution leaves the last cursor to close. */
            if (!rc && fresh) cur = fresh;
            U32 affected = 0;
            U32 crc = ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(stmt, 2, &affected);
            printf(", \"affected\": %lu", crc ? 0 : affected);
            if (!rc && fresh) cursor(fresh);
            if (query && !rc && fresh) ((U32(WINAPI *)(H))F("AdsCloseTable"))(fresh);
        } else if (!strcmp(cmd, "directnull")) {
            rc = ((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(stmt, (U8 *)rest, NULL);
            result(rc);
        } else if (!strcmp(cmd, "prepare")) {
            rc = ((U32(WINAPI *)(H, U8 *))F("AdsPrepareSQL"))(stmt, (U8 *)rest);
            result(rc);
        } else if (!strcmp(cmd, "setprop") || !strcmp(cmd, "getprop")) {
            char *target = word(&rest);
            H obj = !strcmp(target, "stmt") ? stmt : !strcmp(target, "conn") ? connection
                    : !strcmp(target, "cursor") ? cur
                    : (H)strtoul(target, NULL, 0);
            U32 id = (U32)strtoul(word(&rest), NULL, 0);
            U32 out = 0xdeadbeef;
            U32 value = *cmd == 'g' ? (U32)(ULONG_PTR)&out : (U32)strtoul(rest, NULL, 0);
            rc = ((U32(WINAPI *)(H, U32, U32))F("AdsSetProperty"))(obj, id, value);
            result(rc);
            if (*cmd == 'g') printf(", \"out\": %lu", out);
        } else if (!strcmp(cmd, "verify")) {
            rc = ((U32(WINAPI *)(H, U8 *))F("AdsVerifySQL"))(stmt, (U8 *)rest);
            result(rc);
        } else if (!strcmp(cmd, "param")) {
            char *target = word(&rest);
            char *kind = word(&rest);
            U8 *field = (*target >= '0' && *target <= '9') ? FIELD(atoi(target)) : (U8 *)target;
            if (!strcmp(kind, "string"))
                rc = ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetString"))(stmt, field, (U8 *)rest,
                                                                          strlen(rest));
            else if (!strcmp(kind, "long"))
                rc = ((U32(WINAPI *)(H, U8 *, LONG))F("AdsSetLong"))(stmt, field, atol(rest));
            else if (!strcmp(kind, "short"))
                rc = ((U32(WINAPI *)(H, U8 *, short))F("AdsSetShort"))(stmt, field, atoi(rest));
            else if (!strcmp(kind, "double"))
                rc = ((U32(WINAPI *)(H, U8 *, double))F("AdsSetDouble"))(stmt, field, atof(rest));
            else if (!strcmp(kind, "logical"))
                rc = ((U32(WINAPI *)(H, U8 *, U16))F("AdsSetLogical"))(stmt, field, atoi(rest));
            else if (!strcmp(kind, "date"))
                rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16))F("AdsSetDate"))(stmt, field, (U8 *)rest,
                                                                          strlen(rest));
            else if (!strcmp(kind, "money"))
                rc = ((U32(WINAPI *)(H, U8 *, LONGLONG))F("AdsSetMoney"))(stmt, field,
                                                                         _atoi64(rest));
            else if (!strcmp(kind, "null"))
                rc = ((U32(WINAPI *)(H, U8 *))F("AdsSetNull"))(stmt, field);
            else if (!strcmp(kind, "empty"))
                rc = ((U32(WINAPI *)(H, U8 *))F("AdsSetEmpty"))(stmt, field);
            else rc = 99999;
            result(rc);
        } else if (!strcmp(cmd, "clearparams")) {
            result(((U32(WINAPI *)(H))F("AdsClearSQLParams"))(stmt));
        } else if (!strcmp(cmd, "numparams")) {
            U16 n = 77;
            rc = ((U32(WINAPI *)(H, U16 *))F("AdsGetNumParams"))(stmt, &n);
            result(rc);
            printf(", \"n\": %u", n);
        } else if (!strcmp(cmd, "count")) {
            U32 count = 0;
            rc = ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(stmt, 2, &count);
            result(rc);
            printf(", \"count\": %lu", count);
        } else if (!strcmp(cmd, "tabletype") || !strcmp(cmd, "chartype") ||
                   !strcmp(cmd, "locktype") || !strcmp(cmd, "readonly") ||
                   !strcmp(cmd, "rights")) {
            const char *fn = !strcmp(cmd, "tabletype") ? "AdsStmtSetTableType"
                             : !strcmp(cmd, "chartype") ? "AdsStmtSetTableCharType"
                             : !strcmp(cmd, "locktype") ? "AdsStmtSetTableLockType"
                             : !strcmp(cmd, "readonly") ? "AdsStmtSetTableReadOnly"
                                                        : "AdsStmtSetTableRights";
            result(((U32(WINAPI *)(H, U16))F(fn))(stmt, (U16)atoi(rest)));
        } else if (!strcmp(cmd, "collation")) {
            result(((U32(WINAPI *)(H, U8 *))F("AdsStmtSetTableCollation"))(stmt, (U8 *)rest));
        } else if (!strcmp(cmd, "setcollation")) {
            result(((U32(WINAPI *)(H, U8 *))F("AdsSetCollation"))(connection, (U8 *)rest));
        } else if (!strcmp(cmd, "getcollation")) {
            U8 text[300];
            U16 len = sizeof text;
            rc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsGetCollation"))(connection, text, &len);
            result(rc);
            printf(", \"collation\": ");
            json_bytes(text, rc ? 0 : len);
        } else if (!strcmp(cmd, "closestmt")) {
            result(((U32(WINAPI *)(H))F("AdsCloseSQLStatement"))(stmt));
        } else if (!strcmp(cmd, "closecursor")) {
            result(((U32(WINAPI *)(H))F("AdsCloseTable"))(cur));
        } else if (!strcmp(cmd, "info")) {
            U16 stype = 0, ctype = 0;
            U32 src = ((U32(WINAPI *)(H, U16 *))F("AdsGetHandleType"))(stmt, &stype);
            U32 crc = ((U32(WINAPI *)(H, U16 *))F("AdsGetHandleType"))(cur, &ctype);
            printf("\"stmt_type\": %lu, \"cursor_type\": %lu", src ? src + 100000 : stype,
                   crc ? crc + 100000 : ctype);
            H owner = 0;
            rc = ((U32(WINAPI *)(H, H *))F("AdsGetSQLStatementHandle"))(cur, &owner);
            printf(", \"owner_rc\": %lu, \"owner_is_stmt\": %s", rc, owner == stmt ? "true" : "false");
            U8 text[4096];
            U16 len = sizeof text;
            rc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsGetSQLStatement"))(stmt, text, &len);
            printf(", \"sql_rc\": %lu, \"sql\": ", rc);
            json_bytes(text, rc ? 0 : len);
            len = sizeof text;
            rc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsGetFilter"))(cur, text, &len);
            printf(", \"filter_rc\": %lu, \"filter\": ", rc);
            json_bytes(text, rc ? 0 : len);
            len = sizeof text;
            rc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsGetAOF"))(cur, text, &len);
            printf(", \"aof_rc\": %lu, \"aof\": ", rc);
            json_bytes(text, rc ? 0 : len);
        } else if (!strcmp(cmd, "peek")) {
            H target = cur, table = 0;
            if (!strcmp(rest, "stmt")) target = stmt;
            else if (!strcmp(rest, "0")) target = 0;
            else if (!strcmp(rest, "table")) {
                char path[2048];
                snprintf(path, sizeof path, "%s\\indexed.adt", argv[2]);
                typedef U32(WINAPI * Open)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *);
                ((Open)F("AdsOpenTable"))(connection, (U8 *)path, NULL, 3, 1, 1, 1, 4, &table);
                target = table;
            }
            U8 live = 0xAA;
            rc = ((U32(WINAPI *)(H, U8 *))F("AdsSqlPeekStatement"))(target, &live);
            result(rc);
            printf(", \"live\": %u", live);
            if (table) ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
        } else if (!strcmp(cmd, "txcount")) {
            U32 n = 0xAAAA;
            rc = ((U32(WINAPI *)(H, U32 *))F("AdsGetTransactionCount"))(connection, &n);
            result(rc);
            printf(", \"count\": %lu", n);
        } else if (!strcmp(cmd, "cap")) {
            dump_cap = atoi(rest);
            printf("\"rc\": 0");
        } else if (!strcmp(cmd, "sleep")) {
            Sleep((DWORD)atoi(rest));
            result(0);
        } else if (!strcmp(cmd, "cachecursors")) {
            rc = ((U32(WINAPI *)(U16))F("AdsCacheOpenCursors"))((U16)atoi(rest));
            result(rc);
        } else if (!strcmp(cmd, "connect")) {
            char path[2048];
            snprintf(path, sizeof path, "%s\\%s", argv[2], word(&rest));
            char *user = word(&rest);
            char *password = word(&rest);
            H fresh = 0;
            if (server == 1) {
                rc = ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
                    (U8 *)path, 1, *user ? (U8 *)user : NULL, *password ? (U8 *)password : NULL,
                    0, &fresh);
            } else {
                char cs[2400];
                snprintf(cs, sizeof cs,
                         "Data Source=%s;ServerType=REMOTE;CommType=TCP_IP;User ID=%s;Password=%s",
                         path, user, password);
                rc = ((U32(WINAPI *)(U8 *, H *, H *))F("AdsConnect101"))((U8 *)cs, NULL, &fresh);
            }
            result(rc);
            if (!rc) connection = fresh;
        } else if (!strcmp(cmd, "switch")) {
            // Swaps the connection, statement and cursor in use with the ones
            // set aside (none the first time), for a second connection.
            static H other[3];
            H mine[3] = {connection, stmt, cur};
            connection = other[0];
            stmt = other[1];
            cur = other[2];
            memcpy(other, mine, sizeof mine);
            result(0);
        } else if (!strcmp(cmd, "ddcreate")) {
            char path[2048];
            snprintf(path, sizeof path, "%s\\%s", argv[2], word(&rest));
            H fresh = 0;
            rc = ((U32(WINAPI *)(U8 *, U16, U8 *, H *))F("AdsDDCreate"))((U8 *)path, 0, NULL,
                                                                         &fresh);
            result(rc);
            if (!rc) connection = fresh;
        } else if (!strcmp(cmd, "ddview")) {
            // ddview <name> <SELECT ...>: AdsDDAddView on the connection.
            char *name = word(&rest);
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *))F("AdsDDAddView"))(
                connection, (U8 *)name, NULL, (U8 *)rest);
            result(rc);
        } else if (!strcmp(cmd, "remove")) {
            // remove <path>: deletes a file (the <data> folder works in it), to
            // probe what a rename does when a companion file is missing.
            printf("\"rc\": %d", DeleteFileA(rest) ? 0 : 1);
        } else if (!strcmp(cmd, "ddrename")) {
            // ddrename <name> <new name> <type> <options>: AdsDDRenameObject on the
            // connection; a name of - is none (NULL), = is empty, and ~ stands for
            // a space.
            char *name = word(&rest);
            char *to = word(&rest);
            for (char *c = name; *c; c++) if (*c == '~') *c = ' ';
            for (char *c = to; *c; c++) if (*c == '~') *c = ' ';
            if (!strcmp(name, "=")) *name = 0;
            if (!strcmp(to, "=")) *to = 0;
            U16 type = (U16)strtoul(word(&rest), NULL, 10);
            U32 options = (U32)strtoul(word(&rest), NULL, 10);
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U32))F("AdsDDRenameObject"))(
                connection, !strcmp(name, "-") ? NULL : (U8 *)name,
                !strcmp(to, "-") ? NULL : (U8 *)to, type, options);
            result(rc);
        } else if (!strcmp(cmd, "dduser")) {
            // dduser <name> <password>: AdsDDCreateUser, in no group.
            char *name = word(&rest);
            char *password = word(&rest);
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *))F("AdsDDCreateUser"))(
                connection, NULL, (U8 *)name, *password ? (U8 *)password : NULL, NULL);
            result(rc);
        } else if (!strcmp(cmd, "dddbprop")) {
            // dddbprop <id> <length> <text>: AdsDDSetDatabaseProperty.
            U16 id = (U16)atoi(word(&rest));
            int length = atoi(word(&rest));
            char *text = word(&rest);
            if (!strcmp(text, "-")) *text = 0;
            if (length < 0) length = (int)strlen(text) + 1;
            rc = ((U32(WINAPI *)(H, U16, void *, U16))F("AdsDDSetDatabaseProperty"))(
                connection, id, text, (U16)length);
            result(rc);
        } else if (!strcmp(cmd, "ddlink")) {
            // ddlink <alias> <path> <user> <password> <options>: AdsDDCreateLink;
            // a user or password of - is none (NULL).
            char *alias = word(&rest);
            char *path = word(&rest);
            char *user = word(&rest);
            char *password = word(&rest);
            if (!strcmp(user, "-")) *user = 0;
            if (!strcmp(password, "-")) *password = 0;
            U32 options = (U32)strtoul(word(&rest), NULL, 10);
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U32))F("AdsDDCreateLink"))(
                connection, (U8 *)alias, (U8 *)path, *user ? (U8 *)user : NULL,
                *password ? (U8 *)password : NULL, options);
            result(rc);
        } else if (!strcmp(cmd, "ddpub") || !strcmp(cmd, "ddart") || !strcmp(cmd, "ddsub") ||
                   !strcmp(cmd, "ddpubcomment") || !strcmp(cmd, "ddsubcomment") ||
                   !strcmp(cmd, "dddelpub") || !strcmp(cmd, "dddelsub")) {
            // The replication objects of the dictionary; see the list at the top.
            char *w[9];
            int n = !strcmp(cmd, "ddsub") ? 9 : !strcmp(cmd, "ddpub") ? 3 :
                    !strcmp(cmd, "ddart") ? 4 : !strncmp(cmd, "dddel", 5) ? 1 : 2;
            for (int i = 0; i < n; i++) {
                w[i] = word(&rest);
                for (char *c = w[i]; *c; c++) if (*c == '~') *c = ' ';
            }
#define OPT(i) (!strcmp(w[i], "-") ? NULL : (U8 *)w[i])
            if (!strcmp(cmd, "ddpub")) {
                rc = ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsDDCreatePublication"))(
                    connection, (U8 *)w[0], OPT(1), (U32)strtoul(w[2], NULL, 10));
            } else if (!strcmp(cmd, "ddart")) {
                rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U32))F("AdsDDCreateArticle"))(
                    connection, (U8 *)w[0], (U8 *)w[1], OPT(2), NULL,
                    (U32)strtoul(w[3], NULL, 10));
            } else if (!strcmp(cmd, "ddsub")) {
                rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U8 *, U16, U8 *,
                                     U32))F("AdsDDCreateSubscription"))(
                    connection, (U8 *)w[0], (U8 *)w[1], (U8 *)w[2], (U8 *)w[3], (U8 *)w[4],
                    (U8 *)w[5], (U16)atoi(w[6]), OPT(7), (U32)strtoul(w[8], NULL, 10));
            } else if (!strcmp(cmd, "ddpubcomment")) {
                rc = ((U32(WINAPI *)(H, U8 *, U16, void *, U16))F("AdsDDSetPublicationProperty"))(
                    connection, (U8 *)w[0], 1, w[1], (U16)strlen(w[1]));
            } else if (!strcmp(cmd, "ddsubcomment")) {
                rc = ((U32(WINAPI *)(H, U8 *, U16, void *, U16))F("AdsDDSetSubscriptionProperty"))(
                    connection, (U8 *)w[0], 1, w[1], (U16)strlen(w[1]));
            } else if (!strcmp(cmd, "dddelpub")) {
                rc = ((U32(WINAPI *)(H, U8 *))F("AdsDDDeletePublication"))(connection, (U8 *)w[0]);
            } else {
                rc = ((U32(WINAPI *)(H, U8 *))F("AdsDDDeleteSubscription"))(connection, (U8 *)w[0]);
            }
#undef OPT
            result(rc);
        } else if (!strcmp(cmd, "ddapi")) {
            // ddapi <function> <args...>: any AdsDD call on the connection (its
            // first argument), whose other arguments are strings or integers, so
            // that each is one 32-bit stack slot. An argument of - is NULL, =
            // is an empty string, #n is the integer n and #L the length of the
            // string before it, plus one; in a string ~ stands for a space.
            char *fn = word(&rest);
            U32 a[10];
            int n = 1;
            a[0] = (U32)(size_t)connection;
            while (*rest && n < 10) {
                char *w = word(&rest);
                if (!*w) break;
                if (!strcmp(w, "-")) a[n] = 0;
                else if (!strcmp(w, "=")) a[n] = (U32)(size_t)"";
                else if (!strcmp(w, "#L")) a[n] = (U32)strlen((char *)(size_t)a[n - 1]) + 1;
                else if (w[0] == '#') a[n] = (U32)strtoul(w + 1, NULL, 10);
                else {
                    for (char *c = w; *c; c++) if (*c == '~') *c = ' ';
                    a[n] = (U32)(size_t)w;
                }
                n++;
            }
            typedef U32(WINAPI * G)();
            G g = (G)F(fn);
            switch (n) {
            case 2: rc = ((U32(WINAPI *)(U32, U32))g)(a[0], a[1]); break;
            case 3: rc = ((U32(WINAPI *)(U32, U32, U32))g)(a[0], a[1], a[2]); break;
            case 4: rc = ((U32(WINAPI *)(U32, U32, U32, U32))g)(a[0], a[1], a[2], a[3]); break;
            case 5: rc = ((U32(WINAPI *)(U32, U32, U32, U32, U32))g)(a[0], a[1], a[2], a[3], a[4]); break;
            case 6: rc = ((U32(WINAPI *)(U32, U32, U32, U32, U32, U32))g)(a[0], a[1], a[2], a[3], a[4], a[5]); break;
            case 7: rc = ((U32(WINAPI *)(U32, U32, U32, U32, U32, U32, U32))g)(a[0], a[1], a[2], a[3], a[4], a[5], a[6]); break;
            case 8: rc = ((U32(WINAPI *)(U32, U32, U32, U32, U32, U32, U32, U32))g)(a[0], a[1], a[2], a[3], a[4], a[5], a[6], a[7]); break;
            default: rc = 99999;
            }
            result(rc);
        } else if (!strcmp(cmd, "droplink")) {
            // droplink <alias> <global>: AdsDDDropLink.
            char *alias = word(&rest);
            U16 global = (U16)atoi(word(&rest));
            rc = ((U32(WINAPI *)(H, U8 *, U16))F("AdsDDDropLink"))(connection, (U8 *)alias,
                                                                  global);
            result(rc);
        } else if (!strcmp(cmd, "numlinks")) {
            U16 links = 0xAAAA;
            rc = ((U32(WINAPI *)(H, U16 *))F("AdsGetNumActiveLinks"))(connection, &links);
            result(rc);
            printf(", \"links\": %u", links);
        } else if (!strcmp(cmd, "linkinfo")) {
            // linkinfo <number> <buffer size>: AdsGetActiveLinkInfo.
            U16 number = (U16)atoi(word(&rest));
            U16 len = (U16)atoi(word(&rest));
            U8 buf[1100];
            memset(buf, 0xEE, sizeof buf);
            rc = ((U32(WINAPI *)(H, U16, U8 *, U16 *))F("AdsGetActiveLinkInfo"))(connection,
                                                                               number, buf, &len);
            result(rc);
            // What it wrote: the bytes up to the first sentinel byte left.
            size_t n = 0;
            while (n < sizeof buf && buf[n] != 0xEE) n++;
            // The length written back counts the data folder (in the
            // link's path) as the 6 bytes of <data>.
            unsigned shown = len;
            if (rc == 0 || rc == 5005) shown = len - strlen(data_folder) + 6;
            printf(", \"len\": %u, \"value\": ", shown);
            json_bytes(buf, n);
        } else if (!strcmp(cmd, "find62") || !strcmp(cmd, "find")) {
            // find62 [mask] / find [mask]: the whole listing of
            // AdsFindFirstTable62 / AdsFindFirstTable, with each table's
            // name (and linked dictionary's) and length written back.
            int with_dd = cmd[4] == '6';
            U8 *mask = *rest ? (U8 *)rest : NULL;
            H find = 0;
            printf("\"names\": [");
            for (int n = 0; n < 100; n++) {
                U8 name[300], dd[300];
                U16 nlen = 250, ddlen = 250;
                memset(dd, 0x5A, sizeof dd);
                if (n == 0 && with_dd)
                    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16 *, U8 *, U16 *, H *))F(
                        "AdsFindFirstTable62"))(connection, mask, dd, &ddlen, name, &nlen, &find);
                else if (n == 0)
                    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16 *, H *))F("AdsFindFirstTable"))(
                        connection, mask, name, &nlen, &find);
                else if (with_dd)
                    rc = ((U32(WINAPI *)(H, H, U8 *, U16 *, U8 *, U16 *))F("AdsFindNextTable62"))(
                        connection, find, dd, &ddlen, name, &nlen);
                else
                    rc = ((U32(WINAPI *)(H, H, U8 *, U16 *))F("AdsFindNextTable"))(
                        connection, find, name, &nlen);
                if (rc) break;
                printf("%s[", n ? ", " : "");
                json_bytes(name, strnlen((char *)name, nlen));
                printf(", %u", nlen);
                if (with_dd) {
                    printf(", ");
                    json_bytes(dd, strnlen((char *)dd, ddlen));
                    printf(", %u", ddlen);
                }
                printf("]");
            }
            printf("], ");
            result(rc);
            if (find) ((U32(WINAPI *)(H, H))F("AdsFindClose"))(connection, find);
        } else if (!strcmp(cmd, "objects")) {
            // objects <type> [parent]: the whole AdsDDFindFirstObject listing.
            U16 type = (U16)atoi(word(&rest));
            U8 *parent = *rest ? (U8 *)rest : NULL;
            H find = 0;
            printf("\"names\": [");
            for (int n = 0; n < 100; n++) {
                U8 name[300];
                U16 nlen = 250;
                if (n == 0)
                    rc = ((U32(WINAPI *)(H, U16, U8 *, U8 *, U16 *, H *))F(
                        "AdsDDFindFirstObject"))(connection, type, parent, name, &nlen, &find);
                else
                    rc = ((U32(WINAPI *)(H, H, U8 *, U16 *))F("AdsDDFindNextObject"))(
                        connection, find, name, &nlen);
                if (rc) break;
                printf("%s[", n ? ", " : "");
                json_bytes(name, strnlen((char *)name, nlen));
                printf(", %u]", nlen);
            }
            printf("], ");
            result(rc);
            if (find) ((U32(WINAPI *)(H, H))F("AdsDDFindClose"))(connection, find);
        } else if (!strcmp(cmd, "open")) {
            // open <name>: AdsOpenTable on the connection (default type,
            // rights checked), the table recorded as a cursor, then closed.
            H table = 0;
            typedef U32(WINAPI * Open)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *);
            rc = ((Open)F("AdsOpenTable"))(connection, (U8 *)rest, NULL, 0, 1, 0, 1, 0, &table);
            result(rc);
            if (!rc) {
                cursor(table);
                ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
            }
        } else if (!strcmp(cmd, "keep")) {
            // keep <name>: AdsOpenTable on the connection (default type,
            // rights checked), left open as the cursor the edit, append,
            // delrec and row commands work on.
            H table = 0;
            typedef U32(WINAPI * Open)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *);
            rc = ((Open)F("AdsOpenTable"))(connection, (U8 *)rest, NULL, 0, 1, 0, 1, 0, &table);
            result(rc);
            if (!rc) cur = table;
        } else if (!strcmp(cmd, "otype")) {
            // otype <type> <name>: AdsOpenTable of <name> exactly as given
            // (a #name too) as table type <type>, recorded as a cursor, then
            // closed.
            H table = 0;
            char *type = word(&rest);
            typedef U32(WINAPI * Open)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *);
            rc = ((Open)F("AdsOpenTable"))(connection, (U8 *)rest, NULL, (U16)atoi(type), 1, 0, 1,
                                           0, &table);
            result(rc);
            if (!rc) {
                cursor(table);
                ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
            }
        } else if (!strcmp(cmd, "ctable")) {
            // ctable <type> <name> <fields>: AdsCreateTable of <name> as
            // table type <type>, the table closed again.
            H table = 0;
            char *type = word(&rest);
            char *name = word(&rest);
            typedef U32(WINAPI * Create)(H, U8 *, U8 *, U16, U16, U16, U16, U32, U8 *, H *);
            rc = ((Create)F("AdsCreateTable"))(connection, (U8 *)name, NULL, (U16)atoi(type), 1, 0,
                                               1, 0, (U8 *)rest, &table);
            result(rc);
            if (!rc) {
                // Not recorded as a cursor: what a created handle reports is not probed.
                ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
            }
        } else if (!strcmp(cmd, "transfree")) {
            // transfree <type> <name>: AdsOpenTable of <name> as table type
            // <type> (0 on a dictionary connection), and its
            // AdsIsTableTransactionFree.
            H table = 0;
            char *type = word(&rest);
            char path[1024];
            // A free connection opens a table by its path.
            if (atoi(type)) snprintf(path, sizeof path, "%s\\%s", data_folder, rest);
            else snprintf(path, sizeof path, "%s", rest);
            typedef U32(WINAPI * Open)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *);
            rc = ((Open)F("AdsOpenTable"))(connection, (U8 *)path, NULL, (U16)atoi(type), 1, 0, 1,
                                           0, &table);
            result(rc);
            if (!rc) {
                U16 free = 0xAAAA;
                rc = ((U32(WINAPI *)(H, U16 *))F("AdsIsTableTransactionFree"))(table, &free);
                printf(", \"free_rc\": %lu, \"trans_free\": %u", rc, free);
                ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
            }
        } else if (!strcmp(cmd, "indexes")) {
            // indexes: the last cursor's AdsGetNumIndexes and, for each
            // order, its handle's name, expression, condition and file name
            // (AdsGetIndexHandleByOrder and the AdsGetIndex* getters), then
            // the handle AdsGetIndexHandle gives for the current order (a
            // NULL name).
            U16 n = 0xAAAA;
            rc = ((U32(WINAPI *)(H, U16 *))F("AdsGetNumIndexes"))(cur, &n);
            result(rc);
            printf(", \"num\": %u, \"orders\": [", n);
            for (U16 i = 1; i <= n; i++) {
                H ih = 0;
                U32 hrc = ((U32(WINAPI *)(H, U16, H *))F("AdsGetIndexHandleByOrder"))(cur, i, &ih);
                printf("%s{\"order\": %u, \"rc\": %lu", i > 1 ? ", " : "", i, hrc);
                if (hrc) {
                    printf("}");
                    continue;
                }
                U8 text[1024];
                U16 len = sizeof text;
                U32 nrc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsGetIndexName"))(ih, text, &len);
                printf(", \"name_rc\": %lu, \"name\": ", nrc);
                json_bytes(text, nrc ? 0 : len);
                U8 name[300];
                memcpy(name, text, nrc ? 0 : len);
                name[nrc ? 0 : len] = 0;
                len = sizeof text;
                nrc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsGetIndexExpr"))(ih, text, &len);
                printf(", \"expr_rc\": %lu, \"expr\": ", nrc);
                json_bytes(text, nrc ? 0 : len);
                len = sizeof text;
                nrc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsGetIndexCondition"))(ih, text, &len);
                printf(", \"cond_rc\": %lu, \"cond\": ", nrc);
                json_bytes(text, nrc ? 0 : len);
                len = sizeof text;
                nrc = ((U32(WINAPI *)(H, U16, U8 *, U16 *))F("AdsGetIndexFilename"))(ih, 2, text,
                                                                                    &len);
                printf(", \"file_rc\": %lu, \"file\": ", nrc);
                json_bytes(text, nrc ? 0 : len);
                U16 back = 0xAAAA;
                nrc = ((U32(WINAPI *)(H, U16 *))F("AdsGetIndexOrderByHandle"))(ih, &back);
                printf(", \"back_rc\": %lu, \"back\": %u", nrc, back);
                H byname = 0;
                nrc = ((U32(WINAPI *)(H, U8 *, H *))F("AdsGetIndexHandle"))(cur, name, &byname);
                printf(", \"byname_rc\": %lu, \"byname_same\": %s", nrc,
                       byname == ih ? "true" : "false");
                U16 type = 0xAAAA;
                nrc = ((U32(WINAPI *)(H, U16 *))F("AdsGetHandleType"))(ih, &type);
                printf(", \"handle_type\": %u}", type);
            }
            printf("]");
            H current = 0;
            rc = ((U32(WINAPI *)(H, U8 *, H *))F("AdsGetIndexHandle"))(cur, NULL, &current);
            printf(", \"current_rc\": %lu", rc);
            if (!rc) {
                U8 text[300];
                U16 len = sizeof text;
                rc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsGetIndexName"))(current, text, &len);
                printf(", \"current_name\": ");
                json_bytes(text, rc ? 0 : len);
            }
        } else if (!strcmp(cmd, "row") || !strcmp(cmd, "gotop") || !strcmp(cmd, "gobottom") ||
                   !strcmp(cmd, "skip") || !strcmp(cmd, "gorec") || !strcmp(cmd, "idxtop") ||
                   !strcmp(cmd, "idxbottom") || !strcmp(cmd, "idxskip") ||
                   !strcmp(cmd, "idxseek") || !strcmp(cmd, "idxseekx") ||
                   !strcmp(cmd, "idxseeklast") || !strcmp(cmd, "idxscope") ||
                   !strcmp(cmd, "idxclearscope")) {
            // row | gotop | gobottom | skip <n> | gorec <n>: navigate the
            // last cursor, then show where it is. idxtop <order> |
            // idxbottom <order> | idxskip <order> <n> | idxseek <order>
            // <key> do so through the order's handle (a number, else a
            // name), the way a client sets a cursor to an order. idxseekx
            // <order> <hard|soft|gt> <string|double> <key> and idxseeklast
            // <order> <string|double> <key> seek with the type given (a
            // double's key is its text read as a number, the key length 8);
            // idxscope <order> <top|bottom> <string|double> <key> and
            // idxclearscope <order> <top|bottom> set and clear a scope.
            H target = cur;
            int through = cmd[0] == 'i';
            if (through) {
                char *which = word(&rest);
                H ih = 0;
                if (*which >= '0' && *which <= '9')
                    rc = ((U32(WINAPI *)(H, U16, H *))F("AdsGetIndexHandleByOrder"))(
                        cur, (U16)atoi(which), &ih);
                else
                    rc = ((U32(WINAPI *)(H, U8 *, H *))F("AdsGetIndexHandle"))(cur, (U8 *)which,
                                                                              &ih);
                target = ih;
                cmd += 3;
                if (rc) {
                    result(rc);
                    goto done;
                }
            }
            U16 found = 0xAAAA;
            if (!strcmp(cmd, "top") || !strcmp(cmd, "gotop"))
                rc = ((U32(WINAPI *)(H))F("AdsGotoTop"))(target);
            else if (!strcmp(cmd, "bottom") || !strcmp(cmd, "gobottom"))
                rc = ((U32(WINAPI *)(H))F("AdsGotoBottom"))(target);
            else if (!strcmp(cmd, "skip"))
                rc = ((U32(WINAPI *)(H, LONG))F("AdsSkip"))(target, atol(word(&rest)));
            else if (!strcmp(cmd, "gorec"))
                rc = ((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(cur, (U32)atol(word(&rest)));
            else if (!strcmp(cmd, "seek"))
                rc = ((U32(WINAPI *)(H, U8 *, U16, U16, U16, U16 *))F("AdsSeek"))(
                    target, (U8 *)rest, (U16)strlen(rest), 2, 2, &found);
            else if (!strcmp(cmd, "seekx") || !strcmp(cmd, "seeklast") ||
                     !strcmp(cmd, "scope")) {
                U16 seektype = 2;
                U16 option = 1;
                if (!strcmp(cmd, "seekx")) {
                    char *how = word(&rest);
                    seektype = !strcmp(how, "soft") ? 1 : !strcmp(how, "gt") ? 4 : 2;
                } else if (!strcmp(cmd, "scope")) {
                    option = !strcmp(word(&rest), "top") ? 1 : 2;
                }
                char *kind_word = word(&rest);
                double number = 0;
                U8 *key = (U8 *)rest;
                U16 keylen = (U16)strlen(rest);
                U16 kind = 2;
                if (!strcmp(kind_word, "double")) {
                    number = atof(rest);
                    key = (U8 *)&number;
                    keylen = 8;
                    kind = 4;
                }
                if (!strcmp(cmd, "seeklast"))
                    rc = ((U32(WINAPI *)(H, U8 *, U16, U16, U16 *))F("AdsSeekLast"))(
                        target, key, keylen, kind, &found);
                else if (!strcmp(cmd, "scope"))
                    rc = ((U32(WINAPI *)(H, U16, U8 *, U16, U16))F("AdsSetScope"))(
                        target, option, key, keylen, kind);
                else
                    rc = ((U32(WINAPI *)(H, U8 *, U16, U16, U16, U16 *))F("AdsSeek"))(
                        target, key, keylen, kind, seektype, &found);
            } else if (!strcmp(cmd, "clearscope"))
                rc = ((U32(WINAPI *)(H, U16))F("AdsClearScope"))(
                    target, !strcmp(rest, "top") ? 1 : 2);
            else rc = 0;
            result(rc);
            if (!strncmp(cmd, "seek", 4)) printf(", \"found\": %u", found);
            U16 eof = 0xAAAA, bof = 0xAAAA;
            ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(cur, &eof);
            ((U32(WINAPI *)(H, U16 *))F("AdsAtBOF"))(cur, &bof);
            U32 recno = 0xAAAA;
            ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordNum"))(cur, 2, &recno);
            printf(", \"eof\": %u, \"bof\": %u, \"recno\": %lu", eof, bof, recno);
            // The cursor's current order afterwards.
            {
                H current = 0;
                U8 text[300];
                U16 len = sizeof text;
                U32 crc = ((U32(WINAPI *)(H, U8 *, H *))F("AdsGetIndexHandle"))(cur, NULL, &current);
                if (!crc)
                    crc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsGetIndexName"))(current, text, &len);
                printf(", \"order_rc\": %lu, \"order\": ", crc);
                json_bytes(text, crc ? 0 : len);
            }
            printf(", \"fields\": [");
            U16 nfields = 0;
            ((U32(WINAPI *)(H, U16 *))F("AdsGetNumFields"))(cur, &nfields);
            for (U16 f = 1; f <= nfields; f++) {
                U8 buf[2000];
                U32 flen = sizeof buf;
                U32 grc = ((U32(WINAPI *)(H, U8 *, U8 *, U32 *, U16))F("AdsGetField"))(
                    cur, FIELD(f), buf, &flen, 0);
                printf("%s", f > 1 ? ", " : "");
                if (grc) printf("\"E%lu\"", grc);
                else json_bytes(buf, flen);
            }
            printf("]");
        } else if (!strcmp(cmd, "edit")) {
            // edit <field number> <text>: AdsSetString on the last cursor's
            // current record, then AdsWriteRecord.
            U16 field = (U16)atoi(word(&rest));
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetString"))(cur, FIELD(field),
                                                                      (U8 *)rest, strlen(rest));
            printf("\"set\": {");
            result(rc);
            printf("}, ");
            result(((U32(WINAPI *)(H))F("AdsWriteRecord"))(cur));
        } else if (!strcmp(cmd, "setstring") || !strcmp(cmd, "setfield")) {
            // setstring | setfield <field number> <text>: AdsSetString or
            // AdsSetField on the last cursor's current record, not written.
            U16 field = (U16)atoi(word(&rest));
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U32))F(
                !strcmp(cmd, "setfield") ? "AdsSetField" : "AdsSetString"))(
                cur, FIELD(field), (U8 *)rest, strlen(rest));
            result(rc);
        } else if (!strcmp(cmd, "write")) {
            result(((U32(WINAPI *)(H))F("AdsWriteRecord"))(cur));
        } else if (!strcmp(cmd, "append")) {
            result(((U32(WINAPI *)(H))F("AdsAppendRecord"))(cur));
        } else if (!strcmp(cmd, "delrec")) {
            result(((U32(WINAPI *)(H))F("AdsDeleteRecord"))(cur));
        } else if (!strcmp(cmd, "indexflags")) {
            // indexflags <order>: the AdsIsIndex* answers, the key count
            // (AdsGetKeyCount, respecting and ignoring filters), the key
            // number of the current record and the collation of an order.
            H ih = 0;
            rc = ((U32(WINAPI *)(H, U16, H *))F("AdsGetIndexHandleByOrder"))(
                cur, (U16)atoi(rest), &ih);
            result(rc);
            if (!rc) {
                static const char *names[] = {"AdsIsIndexDescending", "AdsIsIndexUnique",
                                              "AdsIsIndexCandidate", "AdsIsIndexCompound",
                                              "AdsIsIndexCustom", "AdsIsIndexPrimaryKey",
                                              "AdsIsIndexFTS", "AdsIsIndexNullable"};
                for (int i = 0; i < 8; i++) {
                    U16 flag = 0xAAAA;
                    U32 frc = ((U32(WINAPI *)(H, U16 *))F(names[i]))(ih, &flag);
                    printf(", \"%s\": [%lu, %u]", names[i] + 10, frc, flag);
                }
                for (int option = 1; option <= 2; option++) {
                    U32 count = 0xAAAA;
                    U32 krc = ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetKeyCount"))(
                        ih, (U16)option, &count);
                    printf(", \"key_count_%d\": [%lu, %lu]", option, krc, count);
                }
                U32 flags = 0xAAAA;
                U32 frc = ((U32(WINAPI *)(H, U32 *))F("AdsGetIndexFlags"))(ih, &flags);
                printf(", \"flags\": [%lu, %lu]", frc, flags);
                U16 klen = 0xAAAA, ktype = 0xAAAA;
                frc = ((U32(WINAPI *)(H, U16 *))F("AdsGetKeyLength"))(ih, &klen);
                printf(", \"key_length\": [%lu, %u]", frc, klen);
                frc = ((U32(WINAPI *)(H, U16 *))F("AdsGetKeyType"))(ih, &ktype);
                printf(", \"key_type\": [%lu, %u]", frc, ktype);
                U32 num = 0xAAAA;
                U32 krc = ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetKeyNum"))(ih, 1, &num);
                printf(", \"key_num\": [%lu, %lu]", krc, num);
                U8 text[300];
                U16 len = sizeof text;
                krc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsGetIndexCollation"))(ih, text, &len);
                printf(", \"collation\": [%lu, ", krc);
                json_bytes(text, krc ? 0 : len);
                printf("]");
            }
        } else if (!strcmp(cmd, "files")) {
            // files: AdsGetTableFilename and AdsGetIndexFilename of the
            // last cursor's last order, with options 1 and 2 (3 is a path of the host).
            H ih = 0;
            U16 n = 0;
            ((U32(WINAPI *)(H, U16 *))F("AdsGetNumIndexes"))(cur, &n);
            if (n) ((U32(WINAPI *)(H, U16, H *))F("AdsGetIndexHandleByOrder"))(cur, n, &ih);
            for (U16 option = 1; option <= 2; option++) {
                U8 text[600];
                U16 len = sizeof text;
                U32 trc = ((U32(WINAPI *)(H, U16, U8 *, U16 *))F("AdsGetTableFilename"))(
                    cur, option, text, &len);
                printf("%s\"table_%u\": [%lu, ", option > 1 ? ", " : "", option, trc);
                json_bytes(text, trc ? 0 : len);
                printf("]");
                len = sizeof text;
                trc = ih ? ((U32(WINAPI *)(H, U16, U8 *, U16 *))F("AdsGetIndexFilename"))(
                              ih, option, text, &len)
                         : 99999;
                printf(", \"index_%u\": [%lu, ", option, trc);
                json_bytes(text, trc ? 0 : len);
                printf("]");
            }
        } else if (!strcmp(cmd, "openindex")) {
            // openindex <name>: AdsOpenIndex on the last cursor, the
            // handles it gives and the current order after it.
            H handles[16];
            U16 n = 16;
            rc = ((U32(WINAPI *)(H, U8 *, H *, U16 *))F("AdsOpenIndex"))(cur, (U8 *)rest, handles,
                                                                         &n);
            result(rc);
            printf(", \"n\": %u", rc ? 0 : n);
            H current = 0;
            U8 text[300];
            U16 len = sizeof text;
            U32 crc = ((U32(WINAPI *)(H, U8 *, H *))F("AdsGetIndexHandle"))(cur, NULL, &current);
            if (!crc)
                crc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsGetIndexName"))(current, text, &len);
            printf(", \"order_rc\": %lu, \"order\": ", crc);
            json_bytes(text, crc ? 0 : len);
        } else if (!strcmp(cmd, "allindexes")) {
            // allindexes: AdsGetAllIndexes on the last cursor, each handle's
            // name and the order AdsGetIndexOrderByHandle gives.
            H handles[32];
            U16 n = 32;
            rc = ((U32(WINAPI *)(H, H *, U16 *))F("AdsGetAllIndexes"))(cur, handles, &n);
            result(rc);
            printf(", \"n\": %u, \"names\": [", rc ? 0 : n);
            for (U16 i = 0; !rc && i < n; i++) {
                U8 text[300];
                U16 len = sizeof text;
                U32 nrc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsGetIndexName"))(handles[i], text,
                                                                               &len);
                printf("%s", i ? ", " : "");
                json_bytes(text, nrc ? 0 : len);
            }
            printf("]");
        } else if (!strcmp(cmd, "byexpr")) {
            // byexpr <descending> <expression>: AdsGetIndexHandleByExpr.
            U32 descending = (U32)atol(word(&rest));
            H ih = 0;
            rc = ((U32(WINAPI *)(H, U8 *, U32, H *))F("AdsGetIndexHandleByExpr"))(
                cur, (U8 *)rest, descending, &ih);
            result(rc);
            if (!rc) {
                U8 text[300];
                U16 len = sizeof text;
                U32 nrc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsGetIndexName"))(ih, text, &len);
                printf(", \"name\": ");
                json_bytes(text, nrc ? 0 : len);
            }
        } else if (!strcmp(cmd, "disconnect")) {
            result(((U32(WINAPI *)(H))F("AdsDisconnect"))(connection));
        } else if (!strcmp(cmd, "stmtparams")) {
            size_t n = strlen(rest);
            U8 *copy = malloc(n + 16);
            memcpy(copy, rest, n + 1);
            memset(copy + n + 1, 0xAA, 15);
            rc = ((U32(WINAPI *)(U8 *))F("AdsGetSQLStmtParams"))(copy);
            result(rc);
            printf(", \"text\": ");
            json_bytes(copy, n + 16);
            free(copy);
        } else {
            printf("\"unknown\": true");
        }
    done:
        printf("}");
        fflush(stdout);
    }
    printf("\n]\n");
    return 0;
}
