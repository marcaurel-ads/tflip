/* An external procedure library, the version 2 interface of the Advantage 12
 * help, calling back through the ACE it links: for the probe of the original
 * server (aep.txt, run.sh: a 64-bit DLL linking ace64.dll) and for tflip's
 * tests (crates/tflip-ace/tests/aep.rs, crates/tflipd/tests/aep.rs: a shared
 * object linking libtflip_ace.so).
 *
 *     x86_64-w64-mingw32-gcc -shared -o aeptest.aep aep.c ace64.dll
 *     cc -shared -fPIC -o aeptest.aep aep.c [-L<dir> -ltflip_ace]
 */
#include <stddef.h>
#include <stdio.h>

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
/* The connection ids and handles Startup was given, the last first. */
static UNSIGNED32 last_id;
static ADSHANDLE last_handle;

EXPORT UNSIGNED32 ENTRY GetInterfaceVersion(void) { return 2; }

EXPORT UNSIGNED32 ENTRY Startup(UNSIGNED32 id, ADSHANDLE connection) {
    starts++;
    last_id = id;
    last_handle = connection;
    return 0;
}

EXPORT UNSIGNED32 ENTRY Shutdown(UNSIGNED32 id, ADSHANDLE connection) {
    (void)id; (void)connection;
    shutdowns++;
    return 0;
}

/* Runs `sql` on the connection, and the cursor of a SELECT in `cursor`. */
static UNSIGNED32 run(ADSHANDLE connection, const char *sql, ADSHANDLE *cursor) {
    ADSHANDLE statement;
    UNSIGNED32 error = AdsCreateSQLStatement(connection, &statement);
    if (error) return error;
    error = AdsExecuteSQLDirect(statement, sql, cursor);
    if (!cursor) AdsCloseSQLStatement(statement);
    return error;
}

/* `__output`'s one row of integers. */
static UNSIGNED32 output(ADSHANDLE connection, const char *values) {
    char sql[128];
    snprintf(sql, sizeof sql, "INSERT INTO __output VALUES (%s)", values);
    return run(connection, sql, NULL);
}

/* result = n * 2, n read from __input. */
EXPORT UNSIGNED32 ENTRY DoubleIt(UNSIGNED32 id, ADSHANDLE connection, UNSIGNED32 *rows) {
    ADSHANDLE cursor;
    int n;
    char values[32];
    UNSIGNED32 error = run(connection, "SELECT n FROM __input", &cursor);
    (void)id; (void)rows;
    if (error) return error;
    error = AdsGetLong(cursor, "n", &n);
    AdsCloseTable(cursor);
    if (error) return error;
    snprintf(values, sizeof values, "%d", n * 2);
    return output(connection, values);
}

/* How often Startup and Shutdown ran. */
EXPORT UNSIGNED32 ENTRY Counts(UNSIGNED32 id, ADSHANDLE connection, UNSIGNED32 *rows) {
    char values[32];
    (void)id; (void)rows;
    snprintf(values, sizeof values, "%d, %d", starts, shutdowns);
    return output(connection, values);
}

/* Whether the connection id and handle are Startup's, and whether the
 * handle is a connection's (2 if it is, 1 if not) and a server's. */
EXPORT UNSIGNED32 ENTRY Ids(UNSIGNED32 id, ADSHANDLE connection, UNSIGNED32 *rows) {
    char values[64];
    (void)rows;
    snprintf(values, sizeof values, "%d, %d", id == last_id, connection == last_handle);
    return output(connection, values);
}

/* A row in the dictionary's table t, through the connection it was given,
 * and 5 rows affected. */
EXPORT UNSIGNED32 ENTRY AddRow(UNSIGNED32 id, ADSHANDLE connection, UNSIGNED32 *rows) {
    (void)id;
    *rows = 5;
    return run(connection, "INSERT INTO t VALUES (7)", NULL);
}

/* Fails with an error of its own. */
EXPORT UNSIGNED32 ENTRY Refuse(UNSIGNED32 id, ADSHANDLE connection, UNSIGNED32 *rows) {
    (void)id; (void)rows;
    return run(connection, "INSERT INTO __error VALUES (42, 'no thanks')", NULL);
}

/* Fails with a message and no number. */
EXPORT UNSIGNED32 ENTRY RefuseText(UNSIGNED32 id, ADSHANDLE connection, UNSIGNED32 *rows) {
    (void)id; (void)rows;
    return run(connection, "INSERT INTO __error (MESSAGE) VALUES ('just text')", NULL);
}

/* Fails with a message and a NULL number, or a number 0. */
EXPORT UNSIGNED32 ENTRY RefuseNull(UNSIGNED32 id, ADSHANDLE connection, UNSIGNED32 *rows) {
    (void)id; (void)rows;
    return run(connection, "INSERT INTO __error VALUES (NULL, 'null number')", NULL);
}

EXPORT UNSIGNED32 ENTRY RefuseZero(UNSIGNED32 id, ADSHANDLE connection, UNSIGNED32 *rows) {
    (void)id; (void)rows;
    return run(connection, "INSERT INTO __error VALUES (0, 'zero')", NULL);
}

/* Returns an error code without saying more, after writing a result. */
EXPORT UNSIGNED32 ENTRY ReturnCode(UNSIGNED32 id, ADSHANDLE connection, UNSIGNED32 *rows) {
    (void)id; (void)rows;
    output(connection, "3");
    return 7008;
}

/* Runs a statement that fails, and returns its error. */
EXPORT UNSIGNED32 ENTRY BadSql(UNSIGNED32 id, ADSHANDLE connection, UNSIGNED32 *rows) {
    (void)id; (void)rows;
    return run(connection, "SELECT * FROM nosuchtable", NULL);
}

/* The error code a statement of the library that fails gets. */
EXPORT UNSIGNED32 ENTRY BadSqlCode(UNSIGNED32 id, ADSHANDLE connection, UNSIGNED32 *rows) {
    char values[32];
    (void)id; (void)rows;
    snprintf(values, sizeof values, "%u", run(connection, "SELECT * FROM nosuchtable", NULL));
    return output(connection, values);
}
