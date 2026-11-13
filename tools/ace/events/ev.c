/* Records what an Advantage server does with event notifications, across
 * several connections and threads, for fixtures/sql/events*.json.
 *
 *     ev.exe <ace32.dll> <server data path> <script>  > events.json
 *
 * Built with i686-w64-mingw32-gcc and run under Wine against the evaluation
 * server (<server data path> is \\127.0.0.1:6262\data\<folder>). One command
 * a line; each result is a JSON object, with the time it took in ms:
 *
 *     # comment
 *     free <id> [subfolder]      connect to the folder (or its subfolder) as free tables
 *     connect <id> <dd> [user [pw]]
 *                                connect to the dictionary <dd> (a path under
 *                                the folder), as ADSSYS with no password
 *                                unless given
 *     sql <id> <sql>             AdsExecuteSQLDirect on connection <id>; the
 *                                cursor's columns and rows are written
 *     async <id> <sql>           the same on a thread of its own, not waited for
 *     join <id>                  waits for the async statement of <id>
 *     sleep <ms>
 *     pace <ms>                  sleep <ms> that only keeps the server from
 *                                resetting the circuit (6624) after a burst of
 *                                failed logins: a replay need not wait
 *     cancel <id>                sp_CancelQuery on connection <id>, of the last
 *                                statement that waits for an event (found with
 *                                sp_GetSQLStatements)
 *     hold <id> <sql>            like sql, but the statement and its cursor stay
 *                                open (the table with them) until release
 *     release <id>               closes what hold left open on connection <id>
 *     userprop <id> <user> <property id> [buffer length]
 *                                AdsDDGetUserProperty on connection <id>; the
 *                                bytes of the value are written
 *     dbprop <id> <property id> [buffer length]
 *                                AdsDDGetDatabaseProperty
 *     connopt <text>             text, such as TableType=CDX; (ending in ;), is added to
 *                                the connection strings of the connections that follow
 *     open <id> <table> <x|s> [type]
 *                                AdsOpenTable (exclusive or shared, table type as for
 *                                ttype, default 0); it stays open
 *     lock <id> <record>         AdsLockRecord on the table <id> opened
 *     closetable <id>
 *     goto <id> <record>         AdsGotoRecord on the table <id> opened; recall <id> AdsRecallRecord; delete <id> AdsDeleteRecord; skip <id> <n> AdsSkip (0: GotoTop); showdel <id> AdsShowDeleted(TRUE);
 *                                setstr <id> <field> <text> AdsSetString; setbin <id> <field> <total> <offset> <text>
 *                                AdsSetBinary (a partial write) on the current record
 *     reindex <id>               AdsReindex on the table <id> opened
 *     shell <command>            cmd.exe /c <command> (to copy a folder between
 *                                two statements, as the server would see it)
 *     ttype <n>                 table type of the CREATE TABLE statements that follow
 *                                (1 NTX, 2 CDX, 3 ADT, 4 VFP; 0 the connection's default)
 *     disconnect <id>
 *
 * \r, \n and \t in <sql> stand for those characters. */
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

static const char *folder;
static H conns[32];
static HANDLE threads[32];
static char *pending_sql[32];
static char *pending_out[32];
static H held_stmt[32], held_cursor[32];
/* The connection whose next statement stays open (hold), or -1. */
static int hold_id = -1;
static U16 table_type = 0;
static H opened[32];
/* Connection string options (each ending in ;) of the connections made next. */
static char conn_extra[400] = "";

static void jb(char *o, size_t *n, const U8 *s, size_t len) {
    o[(*n)++] = '"';
    for (size_t i = 0; i < len; i++) {
        U8 c = s[i];
        if (c == '"' || c == '\\') *n += sprintf(o + *n, "\\%c", c);
        else if (c < 32 || c >= 127) *n += sprintf(o + *n, "\\u%04x", c);
        else o[(*n)++] = c;
    }
    o[(*n)++] = '"';
}

/* Runs sql on connection c; the JSON of the outcome goes to out. */
static void run(H c, const char *sql, char *out) {
    size_t n = 0;
    U8 *buf = malloc(70000);
    DWORD t0 = GetTickCount();
    H stmt = 0, cur = 0;
    U32 rc = ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(c, &stmt);
    if (!rc && table_type) rc = ((U32(WINAPI *)(H, U16))F("AdsStmtSetTableType"))(stmt, table_type);
    if (!rc) rc = ((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(stmt, (U8 *)sql, &cur);
    DWORD ms = GetTickCount() - t0;
    n += sprintf(out + n, "\"rc\": %lu", rc);
    if (rc) {
        U32 code = 0;
        U8 text[1200];
        U16 len = sizeof text;
        ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
        n += sprintf(out + n, ", \"error\": ");
        jb(out, &n, text, len);
    }
    if (!rc && cur) {
        U16 fields = 0;
        ((U32(WINAPI *)(H, U16 *))F("AdsGetNumFields"))(cur, &fields);
        n += sprintf(out + n, ", \"fields\": [");
        for (U16 f = 1; f <= fields; f++) {
            U8 name[260];
            U16 nlen = sizeof name, ftype = 0;
            U32 flen = 0;
            ((U32(WINAPI *)(H, U16, U8 *, U16 *))F("AdsGetFieldName"))(cur, f, name, &nlen);
            ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsGetFieldType"))(cur, FIELD(f), &ftype);
            ((U32(WINAPI *)(H, U8 *, U32 *))F("AdsGetFieldLength"))(cur, FIELD(f), &flen);
            n += sprintf(out + n, "%s[", f > 1 ? ", " : "");
            jb(out, &n, name, nlen);
            n += sprintf(out + n, ", %u, %lu]", ftype, flen);
        }
        n += sprintf(out + n, "], \"rows\": [");
        ((U32(WINAPI *)(H))F("AdsGotoTop"))(cur);
        for (int r = 0; r < 100; r++) {
            U16 eof = 0;
            ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(cur, &eof);
            if (eof) break;
            n += sprintf(out + n, "%s[", r ? ", " : "");
            for (U16 f = 1; f <= fields; f++) {
                if (f > 1) n += sprintf(out + n, ", ");
                U16 null = 0;
                ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsIsNull"))(cur, FIELD(f), &null);
                if (null) {
                    n += sprintf(out + n, "null");
                    continue;
                }
                U32 flen = 70000;
                U32 r2 = ((U32(WINAPI *)(H, U8 *, U8 *, U32 *, U16))F("AdsGetField"))(
                    cur, FIELD(f), buf, &flen, 0);
                if (r2) n += sprintf(out + n, "\"E%lu\"", r2);
                else jb(out, &n, buf, flen);
            }
            n += sprintf(out + n, "]");
            ((U32(WINAPI *)(H, U32))F("AdsSkip"))(cur, 1);
        }
        n += sprintf(out + n, "]");
    }
    n += sprintf(out + n, ", \"ms\": %lu", ms);
    out[n] = 0;
    if (hold_id >= 0) {
        held_stmt[hold_id] = stmt;
        held_cursor[hold_id] = cur;
        hold_id = -1;
    } else {
        if (cur) ((U32(WINAPI *)(H))F("AdsCloseTable"))(cur);
        if (stmt) ((U32(WINAPI *)(H))F("AdsCloseSQLStatement"))(stmt);
    }
    free(buf);
}

/* The number of the last statement that waits for an event, as
 * sp_GetSQLStatements lists it to the connection c. */
static long waiting_query(H c) {
    long number = 0;
    H stmt = 0, cur = 0;
    const char *sql =
        "SELECT MAX(\"Query Number\") FROM (EXECUTE PROCEDURE sp_GetSQLStatements()) a "
        "WHERE Query LIKE '%WaitFor%Event%'";
    if (((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(c, &stmt)) return 0;
    if (!((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(stmt, (U8 *)sql, &cur) && cur) {
        U8 buf[100];
        U32 len = sizeof buf - 1;
        ((U32(WINAPI *)(H))F("AdsGotoTop"))(cur);
        if (!((U32(WINAPI *)(H, U8 *, U8 *, U32 *, U16))F("AdsGetField"))(cur, FIELD(1), buf, &len, 0)) {
            buf[len] = 0;
            number = atol((char *)buf);
        }
        ((U32(WINAPI *)(H))F("AdsCloseTable"))(cur);
    }
    ((U32(WINAPI *)(H))F("AdsCloseSQLStatement"))(stmt);
    return number;
}

static DWORD WINAPI worker(void *arg) {
    int id = (int)(ULONG_PTR)arg;
    run(conns[id], pending_sql[id], pending_out[id]);
    return 0;
}

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

static char *word(char **s) {
    while (**s == ' ') (*s)++;
    char *w = *s;
    while (**s && **s != ' ') (*s)++;
    if (**s) *(*s)++ = 0;
    return w;
}

int main(int argc, char **argv) {
    if (argc != 4) return fprintf(stderr, "usage: ev <ace32.dll> <path> <script>\n"), 2;
    folder = argv[2];
    ace = LoadLibraryA(argv[1]);
    if (!ace) return fprintf(stderr, "cannot load\n"), 1;
    FILE *script = fopen(argv[3], "r");
    if (!script) return fprintf(stderr, "cannot open script\n"), 1;
    // EV_SERVER_TYPE=1 runs on the local server, the folder then a local path.
    U16 server = getenv("EV_SERVER_TYPE") ? (U16)atoi(getenv("EV_SERVER_TYPE")) : 2;
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(server);
    static char line[70000], text[70000], head[140000], out[200000];
    int first = 1;
    printf("[\n");
    while (fgets(line, sizeof line, script)) {
        line[strcspn(line, "\r\n")] = 0;
        if (!*line || *line == '#') continue;
        strcpy(text, line);
        char *rest = line;
        char *cmd = word(&rest);
        unescape(rest);
        size_t n = sprintf(head, "%s{\"cmd\": ", first ? "" : ",\n");
        first = 0;
        jb(head, &n, (U8 *)text, strlen(text));
        n += sprintf(head + n, ", ");
        head[n] = 0;
        out[0] = 0;
        if (!strcmp(cmd, "free") || !strcmp(cmd, "connect")) {
            int id = atoi(word(&rest));
            char cs[4000], path[2000];
            H h = 0;
            char *sub = word(&rest);
            if (!strcmp(cmd, "free") && !*sub) snprintf(path, sizeof path, "%s", folder);
            else snprintf(path, sizeof path, "%s\\%s", folder, sub);
            char *user = word(&rest), *pw = word(&rest);
            if (!strcmp(cmd, "connect") && !*user) user = "ADSSYS";
            snprintf(cs, sizeof cs,
                     "Data Source=%s;ServerType=%s;%s%sUser ID=%s;Password=%s",
                     path, server == 1 ? "LOCAL" : "REMOTE", server == 1 ? "" : "CommType=TCP_IP;",
                     conn_extra,
                     !strcmp(cmd, "free") ? "" : user, pw);
            U32 rc = ((U32(WINAPI *)(U8 *, H *, H *))F("AdsConnect101"))((U8 *)cs, NULL, &h);
            if (!rc) conns[id] = h;
            size_t k = sprintf(out, "\"rc\": %lu", rc);
            if (rc) {
                U32 code = 0;
                U8 text[1200];
                U16 len = sizeof text;
                ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
                k += sprintf(out + k, ", \"error\": ");
                jb(out, &k, text, len);
                out[k] = 0;
            }
        } else if (!strcmp(cmd, "userprop") || !strcmp(cmd, "dbprop")) {
            int id = atoi(word(&rest));
            char *user = !strcmp(cmd, "userprop") ? word(&rest) : NULL;
            U16 prop = (U16)atoi(word(&rest));
            char *size = word(&rest);
            U8 buf[400];
            U16 len = *size ? (U16)atoi(size) : sizeof buf;
            U32 rc = user ? ((U32(WINAPI *)(H, U8 *, U16, void *, U16 *))F("AdsDDGetUserProperty"))(
                                conns[id], (U8 *)user, prop, buf, &len)
                          : ((U32(WINAPI *)(H, U16, void *, U16 *))F("AdsDDGetDatabaseProperty"))(
                                conns[id], prop, buf, &len);
            size_t k = sprintf(out, "\"rc\": %lu", rc);
            if (rc) {
                U32 code = 0;
                U8 text[1200];
                U16 tlen = sizeof text;
                ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &tlen);
                k += sprintf(out + k, ", \"error\": ");
                jb(out, &k, text, tlen);
            } else {
                k += sprintf(out + k, ", \"len\": %u, \"value\": [", len);
                for (U16 i = 0; i < len; i++) k += sprintf(out + k, "%s%u", i ? ", " : "", buf[i]);
                k += sprintf(out + k, "]");
            }
            out[k] = 0;
        } else if (!strcmp(cmd, "sql")) {
            int id = atoi(word(&rest));
            run(conns[id], rest, out);
        } else if (!strcmp(cmd, "hold")) {
            int id = atoi(word(&rest));
            hold_id = id;
            run(conns[id], rest, out);
        } else if (!strcmp(cmd, "release")) {
            int id = atoi(word(&rest));
            U32 rc = 0;
            if (held_cursor[id]) rc = ((U32(WINAPI *)(H))F("AdsCloseTable"))(held_cursor[id]);
            if (!rc && held_stmt[id]) rc = ((U32(WINAPI *)(H))F("AdsCloseSQLStatement"))(held_stmt[id]);
            held_cursor[id] = held_stmt[id] = 0;
            sprintf(out, "\"rc\": %lu", rc);
        } else if (!strcmp(cmd, "cancel")) {
            int id = atoi(word(&rest));
            char sql[100];
            snprintf(sql, sizeof sql, "EXECUTE PROCEDURE sp_CancelQuery(%ld)", waiting_query(conns[id]));
            run(conns[id], sql, out);
        } else if (!strcmp(cmd, "async")) {
            int id = atoi(word(&rest));
            pending_sql[id] = strdup(rest);
            pending_out[id] = calloc(1, 200000);
            threads[id] = CreateThread(NULL, 0, worker, (void *)(ULONG_PTR)id, 0, NULL);
            sprintf(out, "\"started\": true");
        } else if (!strcmp(cmd, "join")) {
            int id = atoi(word(&rest));
            WaitForSingleObject(threads[id], INFINITE);
            strcpy(out, pending_out[id]);
        } else if (!strcmp(cmd, "sleep") || !strcmp(cmd, "pace")) {
            Sleep(atoi(word(&rest)));
            sprintf(out, "\"%s\": true", !strcmp(cmd, "pace") ? "paced" : "slept");
        } else if (!strcmp(cmd, "open")) {
            int id = atoi(word(&rest));
            char *name = word(&rest), *mode = word(&rest), *type = word(&rest);
            U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
                conns[id], (U8 *)name, NULL, (U16)atoi(type), 1, 1, 1, *mode == 'x' ? 1 : 4, &opened[id]);
            size_t k = sprintf(out, "\"rc\": %lu", rc);
            if (rc) {
                U32 code = 0;
                U8 text[1200];
                U16 len = sizeof text;
                ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
                k += sprintf(out + k, ", \"error\": ");
                jb(out, &k, text, len);
                out[k] = 0;
            }
        } else if (!strcmp(cmd, "lock")) {
            int id = atoi(word(&rest));
            U32 rc = ((U32(WINAPI *)(H, U32))F("AdsLockRecord"))(opened[id], (U32)atoi(word(&rest)));
            sprintf(out, "\"rc\": %lu", rc);
        } else if (!strcmp(cmd, "goto")) {
            int id = atoi(word(&rest));
            U32 rc = ((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(opened[id], (U32)atoi(word(&rest)));
            sprintf(out, "\"rc\": %lu", rc);
        } else if (!strcmp(cmd, "showdel")) {
            U16 on = (U16)atoi(word(&rest));
            sprintf(out, "\"rc\": %lu", ((U32(WINAPI *)(U16))F("AdsShowDeleted"))(on));
        } else if (!strcmp(cmd, "skip")) {
            /* skip <id> <n>: AdsGotoTop when n is 0, else AdsSkip; reports the record number and its deleted flag */
            int id = atoi(word(&rest)), n = atoi(word(&rest));
            U32 rc = n ? ((U32(WINAPI *)(H, int))F("AdsSkip"))(opened[id], n) : ((U32(WINAPI *)(H))F("AdsGotoTop"))(opened[id]);
            U32 rec = 0;
            U16 del = 0;
            ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordNum"))(opened[id], 1, &rec);
            ((U32(WINAPI *)(H, U16 *))F("AdsIsRecordDeleted"))(opened[id], &del);
            U8 name[40] = "";
            U32 nlen = sizeof name - 1;
            ((U32(WINAPI *)(H, U8 *, U8 *, U32 *, U16))F("AdsGetString"))(opened[id], (U8 *)"name", name, &nlen, 0);
            name[nlen < sizeof name ? nlen : sizeof name - 1] = 0;
            sprintf(out, "\"rc\": %lu, \"record\": %lu, \"deleted\": %u, \"name\": \"%s\"", rc, rec, del, name);
        } else if (!strcmp(cmd, "reindex")) {
            int id = atoi(word(&rest));
            sprintf(out, "\"rc\": %lu", ((U32(WINAPI *)(H))F("AdsReindex"))(opened[id]));
        } else if (!strcmp(cmd, "delete")) {
            int id = atoi(word(&rest));
            sprintf(out, "\"rc\": %lu", ((U32(WINAPI *)(H))F("AdsDeleteRecord"))(opened[id]));
        } else if (!strcmp(cmd, "recall")) {
            int id = atoi(word(&rest));
            sprintf(out, "\"rc\": %lu", ((U32(WINAPI *)(H))F("AdsRecallRecord"))(opened[id]));
        } else if (!strcmp(cmd, "setstr")) {
            /* setstr <id> <field> <text>: AdsSetString on the current record */
            int id = atoi(word(&rest));
            char *f = word(&rest);
            U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetString"))(opened[id], (U8 *)f, (U8 *)rest, (U32)strlen(rest));
            sprintf(out, "\"rc\": %lu", rc);
        } else if (!strcmp(cmd, "setbin")) {
            /* setbin <id> <field> <total length> <offset> <text>: AdsSetBinary (a partial write) */
            int id = atoi(word(&rest));
            char *f = word(&rest);
            U32 total = (U32)atoi(word(&rest)), off = (U32)atoi(word(&rest));
            U32 rc = ((U32(WINAPI *)(H, U8 *, U16, U32, U32, U8 *, U32))F("AdsSetBinary"))(opened[id], (U8 *)f, 6, total, off,
                                                                                         (U8 *)rest, (U32)strlen(rest));
            sprintf(out, "\"rc\": %lu", rc);
        } else if (!strcmp(cmd, "closetable")) {
            int id = atoi(word(&rest));
            sprintf(out, "\"rc\": %lu", ((U32(WINAPI *)(H))F("AdsCloseTable"))(opened[id]));
        } else if (!strcmp(cmd, "connopt")) {
            snprintf(conn_extra, sizeof conn_extra, "%s", rest);
            sprintf(out, "\"connopt\": true");
        } else if (!strcmp(cmd, "shell")) {
            char cmdline[70100];
            snprintf(cmdline, sizeof cmdline, "%s >nul 2>&1", rest);
            sprintf(out, "\"exit\": %d", system(cmdline));
        } else if (!strcmp(cmd, "ttype")) {
            table_type = (U16)atoi(word(&rest));
            sprintf(out, "\"ttype\": %u", table_type);
        } else if (!strcmp(cmd, "disconnect")) {
            int id = atoi(word(&rest));
            U32 rc = ((U32(WINAPI *)(H))F("AdsDisconnect"))(conns[id]);
            sprintf(out, "\"rc\": %lu", rc);
        } else sprintf(out, "\"error\": \"unknown command\"");
        printf("%s%s}", head, out);
        fflush(stdout);
    }
    printf("\n]\n");
    return 0;
}
