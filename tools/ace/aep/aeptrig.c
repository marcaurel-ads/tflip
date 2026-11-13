/* A trigger library (the trigger prototype of the Advantage 12 help's
 * "Trigger Containers": connection id, handle, trigger name, table name,
 * event, trigger type, record number), calling back through the ACE it
 * links: for the probe of the original server (aeptrig.txt, run.sh: a 64-bit
 * DLL linking ace64.dll) and for tflip's tests (crates/tflip-ace/tests/
 * aeptrig.rs, crates/tflipd/tests/aeptrig.rs: a shared object linking
 * libtflip_ace.so).
 *
 *     x86_64-w64-mingw32-gcc -shared -o aeptrig.aep aeptrig.c ace64.dll
 *     cc -shared -fPIC -o aeptrig.aep aeptrig.c [-L<dir> -ltflip_ace]
 *
 * Built with -DPLAIN it has no GetInterfaceVersion, Startup and Shutdown, as
 * a library of the help's trigger template has none.
 */
#include <stddef.h>
#include <stdio.h>
#include <string.h>

#ifdef _WIN32
#define EXPORT __declspec(dllexport)
#define ENTRY __stdcall
#else
#define EXPORT
#define ENTRY
#endif

typedef unsigned int UNSIGNED32;
typedef size_t ADSHANDLE;

UNSIGNED32 ENTRY AdsCreateSQLStatement(ADSHANDLE connection, ADSHANDLE *statement);
UNSIGNED32 ENTRY AdsExecuteSQLDirect(ADSHANDLE statement, const char *sql, ADSHANDLE *cursor);
UNSIGNED32 ENTRY AdsGetLong(ADSHANDLE table, const char *field, int *value);
UNSIGNED32 ENTRY AdsCloseTable(ADSHANDLE table);
UNSIGNED32 ENTRY AdsCloseSQLStatement(ADSHANDLE statement);

EXPORT int starts, shutdowns;

#define TRIGGER(f)                                                                   \
    EXPORT UNSIGNED32 ENTRY f(UNSIGNED32 id, ADSHANDLE connection, const char *name, \
                              const char *table, UNSIGNED32 event, UNSIGNED32 type,  \
                              UNSIGNED32 recno)

#ifndef PLAIN
EXPORT UNSIGNED32 ENTRY GetInterfaceVersion(void) { return 2; }

EXPORT UNSIGNED32 ENTRY Startup(UNSIGNED32 id, ADSHANDLE connection) {
    (void)id; (void)connection;
    starts++;
    return 0;
}

EXPORT UNSIGNED32 ENTRY Shutdown(UNSIGNED32 id, ADSHANDLE connection) {
    (void)id; (void)connection;
    shutdowns++;
    return 0;
}
#endif

static UNSIGNED32 run(ADSHANDLE connection, const char *sql, ADSHANDLE *cursor) {
    ADSHANDLE statement;
    UNSIGNED32 error = AdsCreateSQLStatement(connection, &statement);
    if (error) return error;
    error = AdsExecuteSQLDirect(statement, sql, cursor);
    if (!cursor) AdsCloseSQLStatement(statement);
    return error;
}

/* A row in the table log (what, a, b, c, d), a record of what happened. */
static void log_row(ADSHANDLE connection, const char *what, const char *a, int b, int c, int d) {
    char sql[512];
    snprintf(sql, sizeof sql, "INSERT INTO log (what, a, b, c, d) VALUES ('%s', '%s', %d, %d, %d)", what, a, b, c, d);
    run(connection, sql, NULL);
}

/* The arguments as the library gets them, and whether its connection id is
 * the one Startup got. */
TRIGGER(Args) {
    char text[256];
    (void)id;
    snprintf(text, sizeof text, "%s/%s", name ? name : "(null)", table ? table : "(null)");
    log_row(connection, "args", text, event, type, recno);
    return 0;
}

/* The first column n of __new and __old, -1 where there is none (the error
 * code of the statement, negated, if it fails). */
TRIGGER(Rows) {
    ADSHANDLE cursor;
    int n = -1, o = -1;
    UNSIGNED32 error;
    (void)id; (void)name; (void)table; (void)recno; (void)type;
    error = run(connection, "SELECT n FROM __new", &cursor);
    if (!error) {
        if (AdsGetLong(cursor, "n", &n)) n = -2;
        AdsCloseTable(cursor);
    } else n = -(int)error;
    error = run(connection, "SELECT n FROM __old", &cursor);
    if (!error) {
        if (AdsGetLong(cursor, "n", &o)) o = -2;
        AdsCloseTable(cursor);
    } else o = -(int)error;
    log_row(connection, "rows", "", event, n, o);
    return 0;
}

/* Tries to change __new, and logs the error of that. */
TRIGGER(ChangeNew) {
    (void)id; (void)name; (void)table; (void)event; (void)type; (void)recno;
    log_row(connection, "update", "", run(connection, "UPDATE __new SET n = n + 100", NULL), 0, 0);
    return 0;
}

/* Refuses the change with an error of its own. */
TRIGGER(Refuse) {
    (void)id; (void)name; (void)table; (void)event; (void)type; (void)recno;
    return run(connection, "INSERT INTO __error VALUES (42, 'no thanks')", NULL);
}

TRIGGER(RefuseText) {
    (void)id; (void)name; (void)table; (void)event; (void)type; (void)recno;
    return run(connection, "INSERT INTO __error (MESSAGE) VALUES ('just text')", NULL);
}

/* Returns an error code without saying more. */
TRIGGER(ReturnCode) {
    (void)id; (void)connection; (void)name; (void)table; (void)event; (void)type; (void)recno;
    return 7008;
}

/* A statement that fails, whose error it returns. */
TRIGGER(BadSql) {
    (void)id; (void)name; (void)table; (void)event; (void)type; (void)recno;
    return run(connection, "SELECT * FROM nosuchtable", NULL);
}

/* Writes the log only, so that what the change's transaction does with a
 * trigger's writes shows. */
TRIGGER(Writes) {
    (void)id; (void)name; (void)table; (void)event; (void)type; (void)recno;
    log_row(connection, "writes", "", 0, 0, 0);
    return 0;
}

/* Writes, then refuses. */
TRIGGER(WritesRefuses) {
    log_row(connection, "writes", "", 0, 0, 0);
    return Refuse(id, connection, name, table, event, type, recno);
}

/* How often Startup and Shutdown ran. */
TRIGGER(Counts) {
    (void)id; (void)name; (void)table; (void)event; (void)type; (void)recno;
    log_row(connection, "counts", "", starts, shutdowns, 0);
    return 0;
}

/* Whether __new's memo is NULL (1) or not (0). */
TRIGGER(MemoNull) {
    ADSHANDLE cursor;
    int m = -1;
    (void)id; (void)name; (void)table; (void)recno; (void)type;
    if (!run(connection, "SELECT CASE WHEN memo IS NULL THEN 1 ELSE 0 END AS m FROM __new", &cursor)) {
        AdsGetLong(cursor, "m", &m);
        AdsCloseTable(cursor);
    }
    log_row(connection, "memo", "", event, m, 0);
    return 0;
}

/* A number 0 and a NULL one are no error, with the message. */
TRIGGER(RefuseZero) {
    (void)id; (void)name; (void)table; (void)event; (void)type; (void)recno;
    return run(connection, "INSERT INTO __error VALUES (0, 'zero')", NULL);
}

TRIGGER(RefuseNull) {
    (void)id; (void)name; (void)table; (void)event; (void)type; (void)recno;
    return run(connection, "INSERT INTO __error VALUES (NULL, 'null number')", NULL);
}

/* Whether the connection id is the one of the last call (-1 for the first),
 * and that it and the handle are not 0. */
TRIGGER(Ids) {
    static UNSIGNED32 previous;
    (void)name; (void)table; (void)event; (void)type; (void)recno;
    log_row(connection, "ids", "", previous ? id == previous : -1, id != 0, connection != 0);
    previous = id;
    return 0;
}

/* What the library sees of the trigger's table as the change leaves it: the
 * count of its rows with a value. COUNT(*) is not used: in the library of an
 * AFTER DELETE trigger it still counts the deleted record. */
TRIGGER(Tally) {
    ADSHANDLE cursor;
    int c = -1;
    char sql[128];
    (void)id; (void)name; (void)recno;
    snprintf(sql, sizeof sql, "SELECT COUNT(n) AS c FROM %s WHERE n > 0", table);
    if (!run(connection, sql, &cursor)) {
        AdsGetLong(cursor, "c", &c);
        AdsCloseTable(cursor);
    }
    log_row(connection, "count", "", event, type, c);
    return 0;
}
