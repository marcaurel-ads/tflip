/* An external procedure library that reaches __input, __output and __error
 * through the table API (AdsOpenTable and the rest), for the probe of the
 * original's local server (aeptables.txt, aeptables.sh: a 32-bit DLL linking
 * ace32.dll) and for tflip's tests (crates/tflip-ace/tests/aeptables.rs,
 * crates/tflipd/tests/aeptables.rs: a shared object linking libtflip_ace.so).
 *
 *     i686-w64-mingw32-gcc -shared -o aeptables.aep aeptables.c ace32.dll
 *     cc -shared -fPIC -o aeptables.aep aeptables.c [-L<dir> -ltflip_ace]
 *
 * Built with -DPLAIN (run.sh does, as plain_aeptables.aep) a failed command's
 * trace has only its error code, not the server's text, which names the
 * server's own temporary files for some.
 *
 * One procedure, Run(cmd, ...), takes a script of commands separated by ';'
 * (the CMD of __input's row) and traces each one's result as "command=code"
 * into the trace its output parameter holds (a row of __output written
 * through SQL at the end, after the rows the script appended, and a row of the
 * table log likewise). The commands,
 * with <slot> a number 0 to 7 naming a table handle of the library:
 *
 *   open <slot> <name> [type [charType lockType checkRights options]]
 *   close <slot> | info <slot> | top <slot> | skip <slot> <n> | goto <slot> <n>
 *   get <slot> <field> | getfield <slot> <field> | getlong <slot> <field>
 *   empty <slot> <field>
 *   app <slot> | write <slot> | delete <slot>
 *   set <slot> <field> <text> | setfield <slot> <field> <text>
 *   setlong <slot> <field> <n> | setdouble <slot> <field> <x> | setnull <slot> <field>
 *   sql <statement> | ret <code> | handle <slot>
 */
#include <stdarg.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
#define EXPORT __declspec(dllexport)
#define ENTRY __stdcall
#else
#define EXPORT
#define ENTRY
#endif

typedef unsigned int UNSIGNED32;
typedef unsigned short UNSIGNED16;
typedef unsigned char UNSIGNED8;
typedef size_t ADSHANDLE;

UNSIGNED32 ENTRY AdsCreateSQLStatement(ADSHANDLE connection, ADSHANDLE *statement);
UNSIGNED32 ENTRY AdsExecuteSQLDirect(ADSHANDLE statement, const char *sql, ADSHANDLE *cursor);
UNSIGNED32 ENTRY AdsCloseSQLStatement(ADSHANDLE statement);
UNSIGNED32 ENTRY AdsOpenTable(ADSHANDLE connection, const char *name, const char *alias,
                              UNSIGNED16 type, UNSIGNED16 charType, UNSIGNED16 lockType,
                              UNSIGNED16 checkRights, UNSIGNED32 options, ADSHANDLE *table);
UNSIGNED32 ENTRY AdsCloseTable(ADSHANDLE table);
UNSIGNED32 ENTRY AdsGetHandleType(ADSHANDLE handle, UNSIGNED16 *type);
UNSIGNED32 ENTRY AdsGetTableType(ADSHANDLE table, UNSIGNED16 *type);
UNSIGNED32 ENTRY AdsGetTableAlias(ADSHANDLE table, UNSIGNED8 *alias, UNSIGNED16 *length);
UNSIGNED32 ENTRY AdsGetNumFields(ADSHANDLE table, UNSIGNED16 *count);
UNSIGNED32 ENTRY AdsGetFieldName(ADSHANDLE table, UNSIGNED16 field, UNSIGNED8 *name, UNSIGNED16 *length);
UNSIGNED32 ENTRY AdsGetFieldType(ADSHANDLE table, const char *field, UNSIGNED16 *type);
UNSIGNED32 ENTRY AdsGetFieldLength(ADSHANDLE table, const char *field, UNSIGNED32 *length);
UNSIGNED32 ENTRY AdsGetRecordCount(ADSHANDLE table, UNSIGNED16 filter, UNSIGNED32 *count);
UNSIGNED32 ENTRY AdsGetRecordNum(ADSHANDLE table, UNSIGNED16 filter, UNSIGNED32 *number);
UNSIGNED32 ENTRY AdsAtEOF(ADSHANDLE table, UNSIGNED16 *eof);
UNSIGNED32 ENTRY AdsAtBOF(ADSHANDLE table, UNSIGNED16 *bof);
UNSIGNED32 ENTRY AdsGotoTop(ADSHANDLE table);
UNSIGNED32 ENTRY AdsGotoRecord(ADSHANDLE table, UNSIGNED32 record);
UNSIGNED32 ENTRY AdsSkip(ADSHANDLE table, int n);
UNSIGNED32 ENTRY AdsGetField(ADSHANDLE table, const char *field, UNSIGNED8 *buffer, UNSIGNED32 *length, UNSIGNED16 trim);
UNSIGNED32 ENTRY AdsGetString(ADSHANDLE table, const char *field, UNSIGNED8 *buffer, UNSIGNED32 *length, UNSIGNED16 trim);
UNSIGNED32 ENTRY AdsGetLong(ADSHANDLE table, const char *field, int *value);
UNSIGNED32 ENTRY AdsIsEmpty(ADSHANDLE table, const char *field, UNSIGNED16 *empty);
UNSIGNED32 ENTRY AdsAppendRecord(ADSHANDLE table);
UNSIGNED32 ENTRY AdsWriteRecord(ADSHANDLE table);
UNSIGNED32 ENTRY AdsDeleteRecord(ADSHANDLE table);
UNSIGNED32 ENTRY AdsSetString(ADSHANDLE table, const char *field, const UNSIGNED8 *value, UNSIGNED32 length);
UNSIGNED32 ENTRY AdsSetField(ADSHANDLE table, const char *field, const UNSIGNED8 *value, UNSIGNED32 length);
UNSIGNED32 ENTRY AdsSetLong(ADSHANDLE table, const char *field, int value);
UNSIGNED32 ENTRY AdsSetDouble(ADSHANDLE table, const char *field, double value);
UNSIGNED32 ENTRY AdsSetEmpty(ADSHANDLE table, const char *field);
UNSIGNED32 ENTRY AdsGetLastError(UNSIGNED32 *code, UNSIGNED8 *message, UNSIGNED16 *length);

EXPORT UNSIGNED32 ENTRY GetInterfaceVersion(void) { return 2; }
EXPORT UNSIGNED32 ENTRY Startup(UNSIGNED32 id, ADSHANDLE connection) { (void)id; (void)connection; return 0; }
EXPORT UNSIGNED32 ENTRY Shutdown(UNSIGNED32 id, ADSHANDLE connection) { (void)id; (void)connection; return 0; }

static char trace[4000];
static int failed;

/* Notes that a call of the command failed, so that its last error is traced. */
static UNSIGNED32 note(UNSIGNED32 error) {
    if (error) failed = 1;
    return error;
}

static void say(const char *format, ...) __attribute__((format(printf, 1, 2)));
static void say(const char *format, ...) {
    size_t used = strlen(trace);
    va_list args;
    va_start(args, format);
    vsnprintf(trace + used, sizeof trace - used, format, args);
    va_end(args);
}

static UNSIGNED32 run(ADSHANDLE connection, const char *sql, ADSHANDLE *cursor) {
    ADSHANDLE statement;
    UNSIGNED32 error = AdsCreateSQLStatement(connection, &statement);
    if (error) return error;
    error = AdsExecuteSQLDirect(statement, sql, cursor);
    if (!cursor) AdsCloseSQLStatement(statement);
    return error;
}

EXPORT UNSIGNED32 ENTRY Run(UNSIGNED32 id, ADSHANDLE connection, UNSIGNED32 *rows) {
    static ADSHANDLE slots[8];
    char script[2100], *command, *save = NULL;
    UNSIGNED32 ret = 0, error, length;
    UNSIGNED16 type;
    char buffer[2100], sql[4200];
    ADSHANDLE input, statement;
    (void)id;
    (void)rows;
    trace[0] = 0;
    memset(slots, 0, sizeof slots);
    length = sizeof script - 1;
    if (AdsCreateSQLStatement(connection, &statement)) return 1;
    if (AdsExecuteSQLDirect(statement, "SELECT cmd FROM __input", &input)) return 2;
    if (AdsGetString(input, "cmd", (UNSIGNED8 *)script, &length, 0)) return 3;
    script[length] = 0;
    AdsCloseTable(input);
    AdsCloseSQLStatement(statement);
    for (command = strtok_r(script, ";", &save); command; command = strtok_r(NULL, ";", &save)) {
        char name[32] = "", a[64] = "", b[64] = "", c[200] = "";
        int slot;
        ADSHANDLE h;
        UNSIGNED16 flag;
        failed = 0;
        sscanf(command, " %31s %63s %63s %199[^\n]", name, a, b, c);
        slot = atoi(a) & 7;
        h = slots[slot];
        if (!strcmp(name, "open")) {
            UNSIGNED16 t = 3, ch = 1, lk = 1, cr = 1;
            UNSIGNED32 opt = 0;
            unsigned v[5];
            int got = sscanf(c, "%u %u %u %u %u", &v[0], &v[1], &v[2], &v[3], &v[4]);
            if (got >= 1) t = (UNSIGNED16)v[0];
            if (got >= 2) ch = (UNSIGNED16)v[1];
            if (got >= 3) lk = (UNSIGNED16)v[2];
            if (got >= 4) cr = (UNSIGNED16)v[3];
            if (got >= 5) opt = v[4];
            error = note(AdsOpenTable(connection, b, NULL, t, ch, lk, cr, opt, &slots[slot]));
            say("open %s=%u", b, error);
            if (error) slots[slot] = 0;
        } else if (!strcmp(name, "close")) {
            say("close=%u", note(AdsCloseTable(h)));
            slots[slot] = 0;
        } else if (!strcmp(name, "handle")) {
            type = 0;
            error = note(AdsGetHandleType(h, &type));
            say("handle=%u:%u", error, type);
        } else if (!strcmp(name, "info")) {
            UNSIGNED16 count = 0, i, eof = 9, bof = 9, ttype = 0, len = sizeof buffer;
            UNSIGNED32 records = 0, number = 0, flen;
            type = 0;
            error = note(AdsGetHandleType(h, &type));
            say("info=[ht %u:%u", error, type);
            error = note(AdsGetTableType(h, &ttype));
            say(" tt %u:%u", error, ttype);
            buffer[0] = 0;
            error = note(AdsGetTableAlias(h, (UNSIGNED8 *)buffer, &len));
            say(" alias %u:%s", error, buffer);
            error = note(AdsGetNumFields(h, &count));
            say(" n %u:%u", error, count);
            for (i = 1; i <= count; i++) {
                len = sizeof buffer;
                if (note(AdsGetFieldName(h, i, (UNSIGNED8 *)buffer, &len))) {
                    say(" f?");
                    continue;
                }
                type = 0;
                flen = 0;
                note(AdsGetFieldType(h, buffer, &type));
                note(AdsGetFieldLength(h, buffer, &flen));
                say(" %s/%u/%u", buffer, type, flen);
            }
            error = note(AdsGetRecordCount(h, 1, &records));
            say(" count %u:%u", error, records);
            error = note(AdsGetRecordNum(h, 1, &number));
            say(" recno %u:%u", error, number);
            error = note(AdsAtEOF(h, &eof));
            say(" eof %u:%u", error, eof);
            error = note(AdsAtBOF(h, &bof));
            say(" bof %u:%u]", error, bof);
        } else if (!strcmp(name, "top")) {
            say("top=%u", note(AdsGotoTop(h)));
        } else if (!strcmp(name, "skip")) {
            say("skip=%u", note(AdsSkip(h, atoi(b))));
        } else if (!strcmp(name, "goto")) {
            say("goto=%u", note(AdsGotoRecord(h, (UNSIGNED32)atoi(b))));
        } else if (!strcmp(name, "get") || !strcmp(name, "getfield")) {
            length = sizeof buffer - 1;
            error = !strcmp(name, "get") ? note(AdsGetString(h, b, (UNSIGNED8 *)buffer, &length, 0))
                                         : note(AdsGetField(h, b, (UNSIGNED8 *)buffer, &length, 1));
            if (error) length = 0;
            buffer[length] = 0;
            say("%s %s=%u:'%s'", name, b, error, buffer);
        } else if (!strcmp(name, "getlong")) {
            int value = -99;
            error = note(AdsGetLong(h, b, &value));
            say("getlong %s=%u:%d", b, error, value);
        } else if (!strcmp(name, "empty")) {
            flag = 9;
            error = note(AdsIsEmpty(h, b, &flag));
            say("empty %s=%u:%u", b, error, flag);
        } else if (!strcmp(name, "app")) {
            say("app=%u", note(AdsAppendRecord(h)));
        } else if (!strcmp(name, "write")) {
            say("write=%u", note(AdsWriteRecord(h)));
        } else if (!strcmp(name, "delete")) {
            say("delete=%u", note(AdsDeleteRecord(h)));
        } else if (!strcmp(name, "set")) {
            say("set %s=%u", b, note(AdsSetString(h, b, (UNSIGNED8 *)c, (UNSIGNED32)strlen(c))));
        } else if (!strcmp(name, "setfield")) {
            say("setfield %s=%u", b, note(AdsSetField(h, b, (UNSIGNED8 *)c, (UNSIGNED32)strlen(c))));
        } else if (!strcmp(name, "setlong")) {
            say("setlong %s=%u", b, note(AdsSetLong(h, b, atoi(c))));
        } else if (!strcmp(name, "setdouble")) {
            say("setdouble %s=%u", b, note(AdsSetDouble(h, b, atof(c))));
        } else if (!strcmp(name, "setnull")) {
            say("setnull %s=%u", b, note(AdsSetEmpty(h, b)));
        } else if (!strcmp(name, "sql")) {
            snprintf(sql, sizeof sql, "%s %s %s", a, b, c);
            say("sql=%u", run(connection, sql, NULL));
        } else if (!strcmp(name, "ret")) {
            ret = (UNSIGNED32)atoi(a);
            say("ret");
        } else {
            say("?%s", name);
        }
        {
            UNSIGNED32 code = 0;
            UNSIGNED16 len = sizeof buffer;
            buffer[0] = 0;
            AdsGetLastError(&code, (UNSIGNED8 *)buffer, &len);
#ifdef PLAIN
            if (failed && code) say(" {%u}", code);
#else
            if (failed && code) say(" {%u %s}", code, buffer);
#endif
        }
        say(" | ");
    }
    /* Whatever is left open stays open: the procedure's end closes it. */
    snprintf(sql, sizeof sql, "INSERT INTO __output (trace) VALUES ('");
    {
        char *p = sql + strlen(sql);
        const char *q;
        for (q = trace; *q; q++) {
            if (*q == '\'') *p++ = '\'';
            *p++ = *q;
        }
        strcpy(p, "')");
    }
    run(connection, sql, NULL);
    /* The trace of a call that fails is read from the dictionary's log. */
    snprintf(sql, sizeof sql, "INSERT INTO log VALUES ('");
    {
        char *p = sql + strlen(sql);
        const char *q;
        for (q = trace; *q; q++) {
            if (*q == '\'') *p++ = '\'';
            *p++ = *q;
        }
        strcpy(p, "')");
    }
    run(connection, sql, NULL);
    return ret;
}
