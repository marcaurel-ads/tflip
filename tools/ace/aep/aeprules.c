/* An external procedure library for what a procedure may do with the
 * connection handle it is given, and with connections of its own
 * (aeprules.txt, aeprules.sh; tests: crates/tflip-ace/tests/aeprules.rs,
 * crates/tflipd/tests/aeprules.rs).
 *
 *     i686-w64-mingw32-gcc -shared -Wl,--kill-at -o aeprules.aep aeprules.c ace32.dll
 *     cc -shared -fPIC -o aeprules.aep aeprules.c [-L<dir> -ltflip_ace]
 *
 * Every procedure writes the codes of the calls it makes to __output (an
 * error code 0 being "it worked") and fails itself only when a statement
 * that is its business does.
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
typedef unsigned short UNSIGNED16;
typedef size_t ADSHANDLE;

UNSIGNED32 ENTRY AdsCreateSQLStatement(ADSHANDLE connection, ADSHANDLE *statement);
UNSIGNED32 ENTRY AdsExecuteSQLDirect(ADSHANDLE statement, const char *sql, ADSHANDLE *cursor);
UNSIGNED32 ENTRY AdsGetLong(ADSHANDLE table, const char *field, int *value);
UNSIGNED32 ENTRY AdsGetConnectionPath(ADSHANDLE connection, char *path, UNSIGNED16 *length);
UNSIGNED32 ENTRY AdsCloseTable(ADSHANDLE table);
UNSIGNED32 ENTRY AdsCloseSQLStatement(ADSHANDLE statement);
UNSIGNED32 ENTRY AdsBeginTransaction(ADSHANDLE connection);
UNSIGNED32 ENTRY AdsCommitTransaction(ADSHANDLE connection);
UNSIGNED32 ENTRY AdsRollbackTransaction(ADSHANDLE connection);
UNSIGNED32 ENTRY AdsInTransaction(ADSHANDLE connection, UNSIGNED16 *in);
UNSIGNED32 ENTRY AdsGetHandleType(ADSHANDLE handle, UNSIGNED16 *type);
UNSIGNED32 ENTRY AdsGetConnectionType(ADSHANDLE connection, UNSIGNED16 *type);
UNSIGNED32 ENTRY AdsConnect60(const char *path, UNSIGNED16 type, const char *user, const char *password, UNSIGNED32 options, ADSHANDLE *connection);
UNSIGNED32 ENTRY AdsDisconnect(ADSHANDLE connection);
UNSIGNED32 ENTRY AdsOpenTable(ADSHANDLE connection, const char *name, const char *alias, UNSIGNED16 type, UNSIGNED16 chars, UNSIGNED16 lock, UNSIGNED16 rights, UNSIGNED32 options, ADSHANDLE *table);
UNSIGNED32 ENTRY AdsGetRecordCount(ADSHANDLE table, UNSIGNED16 filter, UNSIGNED32 *count);
UNSIGNED32 ENTRY AdsGotoTop(ADSHANDLE table);

EXPORT UNSIGNED32 ENTRY GetInterfaceVersion(void) { return 2; }
EXPORT UNSIGNED32 ENTRY Startup(UNSIGNED32 id, ADSHANDLE connection) { (void)id; (void)connection; return 0; }
EXPORT UNSIGNED32 ENTRY Shutdown(UNSIGNED32 id, ADSHANDLE connection) { (void)id; (void)connection; return 0; }

static UNSIGNED32 run(ADSHANDLE connection, const char *sql, ADSHANDLE *cursor) {
    ADSHANDLE statement;
    UNSIGNED32 error = AdsCreateSQLStatement(connection, &statement);
    if (error) return error;
    error = AdsExecuteSQLDirect(statement, sql, cursor);
    if (!cursor) AdsCloseSQLStatement(statement);
    return error;
}

/* `__output`'s one row of `n` integers. */
static UNSIGNED32 output(ADSHANDLE connection, int n, const UNSIGNED32 *v) {
    char sql[256] = "INSERT INTO __output VALUES (";
    int i;
    for (i = 0; i < n; i++) {
        char one[16];
        snprintf(one, sizeof one, i ? ", %u" : "%u", v[i]);
        strcat(sql, one);
    }
    strcat(sql, ")");
    return run(connection, sql, NULL);
}

static UNSIGNED32 in_tx(ADSHANDLE connection) {
    UNSIGNED16 in = 7;
    UNSIGNED32 error = AdsInTransaction(connection, &in);
    return error ? 1000000 + error : in;
}

/* The rows of t, counted through a statement on the connection. */
static UNSIGNED32 count_t(ADSHANDLE connection) {
    ADSHANDLE cursor;
    int n = -1;
    UNSIGNED32 error = run(connection, "SELECT COUNT(*) AS c FROM t WHERE n > 0", &cursor);
    if (error) return 2000000 + error;
    AdsGetLong(cursor, "c", &n);
    AdsCloseTable(cursor);
    return n;
}

/* The transaction calls on the handle given: the code of the call, then
 * AdsInTransaction's code (1000000 + the error) or flag after it. */
#define TX(name, call) \
    EXPORT UNSIGNED32 ENTRY name(UNSIGNED32 id, ADSHANDLE connection, UNSIGNED32 *rows) { \
        UNSIGNED32 v[2]; \
        (void)id; (void)rows; \
        v[0] = call(connection); \
        v[1] = in_tx(connection); \
        return output(connection, 2, v); \
    }
TX(ApiBegin, AdsBeginTransaction)
TX(ApiCommit, AdsCommitTransaction)
TX(ApiRollback, AdsRollbackTransaction)

/* The SQL forms, run by the library on the handle given. */
#define SQLTX(name, text) \
    EXPORT UNSIGNED32 ENTRY name(UNSIGNED32 id, ADSHANDLE connection, UNSIGNED32 *rows) { \
        UNSIGNED32 v[2]; \
        (void)id; (void)rows; \
        v[0] = run(connection, text, NULL); \
        v[1] = in_tx(connection); \
        return output(connection, 2, v); \
    }
SQLTX(SqlBegin, "BEGIN TRANSACTION")
SQLTX(SqlCommit, "COMMIT")
SQLTX(SqlRollback, "ROLLBACK")

/* A row into t, and the rows of t after it. */
EXPORT UNSIGNED32 ENTRY WriteT(UNSIGNED32 id, ADSHANDLE connection, UNSIGNED32 *rows) {
    UNSIGNED32 v[2];
    (void)id; (void)rows;
    v[0] = run(connection, "INSERT INTO t VALUES (1)", NULL);
    v[1] = count_t(connection);
    return output(connection, 2, v);
}

EXPORT UNSIGNED32 ENTRY ReadT(UNSIGNED32 id, ADSHANDLE connection, UNSIGNED32 *rows) {
    UNSIGNED32 v[1];
    (void)id; (void)rows;
    v[0] = count_t(connection);
    return output(connection, 1, v);
}

/* t opened with the API on the handle given, rights checked (1), and its
 * rows. */
EXPORT UNSIGNED32 ENTRY OpenT(UNSIGNED32 id, ADSHANDLE connection, UNSIGNED32 *rows) {
    ADSHANDLE table = 0;
    UNSIGNED32 v[2] = {0, 0};
    (void)id; (void)rows;
    v[0] = AdsOpenTable(connection, "t", NULL, 0, 0, 0, 1, 0, &table);
    if (!v[0]) {
        AdsGetRecordCount(table, 1, &v[1]);
        AdsCloseTable(table);
    }
    return output(connection, 2, v);
}

/* What the handle is: AdsGetHandleType and AdsGetConnectionType. */
EXPORT UNSIGNED32 ENTRY Kind(UNSIGNED32 id, ADSHANDLE connection, UNSIGNED32 *rows) {
    UNSIGNED16 handle = 99, type = 99;
    UNSIGNED32 v[4];
    (void)id; (void)rows;
    v[0] = AdsGetHandleType(connection, &handle);
    v[1] = handle;
    v[2] = AdsGetConnectionType(connection, &type);
    v[3] = type;
    return output(connection, 4, v);
}

/* A statement, a table and a cursor opened on the connection the procedure
 * ran on, kept for what runs later, on that connection or another. */
static ADSHANDLE kept_statement, kept_table, kept_cursor;

EXPORT UNSIGNED32 ENTRY Keep(UNSIGNED32 id, ADSHANDLE connection, UNSIGNED32 *rows) {
    UNSIGNED32 v[3];
    (void)id; (void)rows;
    v[0] = AdsCreateSQLStatement(connection, &kept_statement);
    v[1] = AdsOpenTable(connection, "t", NULL, 0, 0, 0, 0, 0, &kept_table);
    v[2] = run(connection, "SELECT n FROM t", &kept_cursor);
    return output(connection, 3, v);
}

EXPORT UNSIGNED32 ENTRY UseStatement(UNSIGNED32 id, ADSHANDLE connection, UNSIGNED32 *rows) {
    ADSHANDLE cursor = 0;
    UNSIGNED32 v[1];
    (void)id; (void)rows;
    v[0] = AdsExecuteSQLDirect(kept_statement, "SELECT n FROM t", &cursor);
    if (!v[0]) AdsCloseTable(cursor);
    return output(connection, 1, v);
}

EXPORT UNSIGNED32 ENTRY UseTable(UNSIGNED32 id, ADSHANDLE connection, UNSIGNED32 *rows) {
    UNSIGNED32 v[2] = {0, 0};
    (void)id; (void)rows;
    v[0] = AdsGotoTop(kept_table);
    return output(connection, 1, v);
}

EXPORT UNSIGNED32 ENTRY UseCursor(UNSIGNED32 id, ADSHANDLE connection, UNSIGNED32 *rows) {
    int n = 0;
    UNSIGNED32 v[2];
    (void)id; (void)rows;
    v[0] = AdsGotoTop(kept_cursor);
    v[1] = AdsGetLong(kept_cursor, "n", &n);
    return output(connection, 2, v);
}

EXPORT UNSIGNED32 ENTRY DropKept(UNSIGNED32 id, ADSHANDLE connection, UNSIGNED32 *rows) {
    UNSIGNED32 v[3];
    (void)id; (void)rows;
    v[0] = AdsCloseSQLStatement(kept_statement);
    v[1] = AdsCloseTable(kept_table);
    v[2] = AdsCloseTable(kept_cursor);
    return output(connection, 3, v);
}

/* A connection of its own to the data dictionary the handle is connected
 * to, a local one (type 1: the original's local server, asked for any
 * type, may connect to a server of its own instead): its code, then in a transaction of its own (begin, an INSERT into t, the
 * rows of t it counts, the rows the passed handle counts, rollback, the
 * rows it counts after it), and the connection types. */
EXPORT UNSIGNED32 ENTRY Own(UNSIGNED32 id, ADSHANDLE connection, UNSIGNED32 *rows) {
    ADSHANDLE own = 0;
    char path[512] = "";
    UNSIGNED16 type = 99, length = sizeof path;
    UNSIGNED32 v[9];
    (void)id; (void)rows;
    if (AdsGetConnectionPath(connection, path, &length)) return 1;
    memset(v, 0, sizeof v);
    v[0] = AdsConnect60(path, 1, "ADSSYS", "", 0, &own);
    if (!v[0]) {
        v[1] = AdsBeginTransaction(own);
        v[2] = run(own, "INSERT INTO t VALUES (2)", NULL);
        v[3] = count_t(own);
        v[4] = count_t(connection);
        v[5] = AdsRollbackTransaction(own);
        v[6] = count_t(own);
        v[7] = AdsGetConnectionType(own, &type);
        v[8] = type;
    }
    output(connection, 9, v);
    if (own) AdsDisconnect(own);
    return 0;
}

/* Transaction calls in a sequence, with a write between them, to see what a
 * call did to the caller's work: each returns the code of every step, and
 * the rows of t that the passed handle counts after it (the marker rows are
 * 1000 and up). The first four: begin, a write, in a transaction, then
 * commit or rollback, by the API or SQL, then in a transaction. */
static UNSIGNED32 marker_rows(ADSHANDLE connection) {
    ADSHANDLE cursor;
    int n = -1;
    UNSIGNED32 error = run(connection, "SELECT COUNT(*) AS c FROM t WHERE n >= 1000", &cursor);
    if (error) return 2000000 + error;
    AdsGetLong(cursor, "c", &n);
    AdsCloseTable(cursor);
    return n;
}

#define SEQ(name, begin, insert, end) \
    EXPORT UNSIGNED32 ENTRY name(UNSIGNED32 id, ADSHANDLE connection, UNSIGNED32 *rows) { \
        UNSIGNED32 v[6]; \
        (void)id; (void)rows; \
        v[0] = begin; \
        v[1] = run(connection, insert, NULL); \
        v[2] = in_tx(connection); \
        v[3] = end; \
        v[4] = in_tx(connection); \
        v[5] = marker_rows(connection); \
        return output(connection, 6, v); \
    }
SEQ(SeqApiCommit, AdsBeginTransaction(connection), "INSERT INTO t VALUES (1000)", AdsCommitTransaction(connection))
SEQ(SeqApiRollback, AdsBeginTransaction(connection), "INSERT INTO t VALUES (1001)", AdsRollbackTransaction(connection))
SEQ(SeqSqlCommit, run(connection, "BEGIN TRANSACTION", NULL), "INSERT INTO t VALUES (1002)", run(connection, "COMMIT", NULL))
SEQ(SeqSqlRollback, run(connection, "BEGIN TRANSACTION", NULL), "INSERT INTO t VALUES (1003)", run(connection, "ROLLBACK", NULL))
/* Without a begin of its own: a write, then the commit or rollback. */
SEQ(WriteApiCommit, 0, "INSERT INTO t VALUES (1004)", AdsCommitTransaction(connection))
SEQ(WriteApiRollback, 0, "INSERT INTO t VALUES (1005)", AdsRollbackTransaction(connection))
SEQ(WriteSqlCommit, 0, "INSERT INTO t VALUES (1006)", run(connection, "COMMIT", NULL))
SEQ(WriteSqlRollback, 0, "INSERT INTO t VALUES (1007)", run(connection, "ROLLBACK", NULL))

/* A connection of its own of the connection type `type`, to the path of the
 * passed handle (type 0: the path with ADS_DEFAULT, AdsConnect60's
 * "any"): the codes of AdsGetConnectionPath and of AdsConnect60, the
 * code of AdsGetConnectionType, the type and a statement on it
 * (a count of t). */
#define OWNT(name, type) \
    EXPORT UNSIGNED32 ENTRY name(UNSIGNED32 id, ADSHANDLE connection, UNSIGNED32 *rows) { \
        ADSHANDLE own = 0; \
        char path[512] = ""; \
        UNSIGNED16 ctype = 99, length = sizeof path; \
        UNSIGNED32 v[5]; \
        (void)id; (void)rows; \
        memset(v, 0, sizeof v); \
        v[0] = AdsGetConnectionPath(connection, path, &length); \
        v[1] = AdsConnect60(path, type, "ADSSYS", "", 0, &own); \
        if (!v[1]) { \
            v[2] = AdsGetConnectionType(own, &ctype); \
            v[3] = ctype; \
            v[4] = count_t(own); \
            AdsDisconnect(own); \
        } \
        return output(connection, 5, v); \
    }
OWNT(OwnDefault, 0)
OWNT(OwnLocal, 1)
OWNT(OwnRemote, 2)
OWNT(OwnBoth, 3)
OWNT(OwnLocalAis, 5)
OWNT(OwnRemoteAis, 6)
OWNT(OwnAis, 4)
OWNT(OwnAny, 7)

/* A transaction left open by the library: a begin and a write, nothing
 * after, by the API and by SQL. */
SEQ(OpenApi, AdsBeginTransaction(connection), "INSERT INTO t VALUES (1008)", 0)
SEQ(OpenSql, run(connection, "BEGIN TRANSACTION", NULL), "INSERT INTO t VALUES (1009)", 0)

/* The path of the passed handle's connection, as text. */
EXPORT UNSIGNED32 ENTRY PathOf(UNSIGNED32 id, ADSHANDLE connection, UNSIGNED32 *rows) {
    char path[512] = "", sql[600];
    UNSIGNED16 length = sizeof path;
    (void)id; (void)rows;
    AdsGetConnectionPath(connection, path, &length);
    snprintf(sql, sizeof sql, "INSERT INTO __output VALUES ('%s')", path);
    return run(connection, sql, NULL);
}

/* A connection of its own, type 0 (what the server makes of it), in a
 * transaction of its own: its begin, a write, the rows it counts, the rows
 * the passed handle counts, its rollback, the rows it counts after it. */
EXPORT UNSIGNED32 ENTRY OwnTx(UNSIGNED32 id, ADSHANDLE connection, UNSIGNED32 *rows) {
    ADSHANDLE own = 0;
    char path[512] = "";
    UNSIGNED16 length = sizeof path;
    UNSIGNED32 v[7];
    (void)id; (void)rows;
    memset(v, 0, sizeof v);
    AdsGetConnectionPath(connection, path, &length);
    v[0] = AdsConnect60(path, 0, "ADSSYS", "", 0, &own);
    if (!v[0]) {
        v[1] = AdsBeginTransaction(own);
        v[2] = run(own, "INSERT INTO t VALUES (2000)", NULL);
        v[3] = marker_rows(own);
        v[4] = marker_rows(connection);
        v[5] = AdsRollbackTransaction(own);
        v[6] = marker_rows(own);
        AdsDisconnect(own);
    }
    return output(connection, 7, v);
}

/* The text of the error a kept handle of another connection gets. */
UNSIGNED32 ENTRY AdsGetLastError(UNSIGNED32 *code, char *message, UNSIGNED16 *length);

static UNSIGNED32 last_error_text(ADSHANDLE connection) {
    UNSIGNED32 code = 0;
    UNSIGNED16 length = 300;
    char message[300] = "", sql[400];
    AdsGetLastError(&code, message, &length);
    snprintf(sql, sizeof sql, "INSERT INTO __output VALUES ('%u: %s')", code, message);
    return run(connection, sql, NULL);
}

EXPORT UNSIGNED32 ENTRY UseStatementText(UNSIGNED32 id, ADSHANDLE connection, UNSIGNED32 *rows) {
    ADSHANDLE cursor = 0;
    (void)id; (void)rows;
    AdsExecuteSQLDirect(kept_statement, "SELECT n FROM t", &cursor);
    return last_error_text(connection);
}

EXPORT UNSIGNED32 ENTRY UseTableText(UNSIGNED32 id, ADSHANDLE connection, UNSIGNED32 *rows) {
    (void)id; (void)rows;
    AdsGotoTop(kept_table);
    return last_error_text(connection);
}
