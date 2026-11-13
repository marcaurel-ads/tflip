/* Probes what an Advantage Client Engine does with transactions: the
 * answers of the transaction calls, nesting and savepoints, what a change
 * made in a transaction does to the files and to what another connection
 * sees, locks, operations the help calls illegal in a transaction, and the
 * SQL transaction statements. Notes on the results are in
 * docs/TRANSACTIONS.md.
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o tx.exe tx.c
 *     wine tx.exe <ace32.dll> <empty data folder as a Windows path>
 *     wine tx.exe <ace32.dll> "Data Source=\\127.0.0.1:6262\data\tx;
 *             ServerType=REMOTE;CommType=TCP_IP" 2 <that folder as a Windows
 *             path> [<the server's log folder>]
 *
 * A Data Source that is a dictionary (an .add) opens its tables by name.
 * The first form uses the local server; the second a remote server (type
 * 2), whose folders it lists through the paths given (the evaluation server
 * under Wine, setup in LOCAL.md; logs in its C:\).
 *
 * It prints one line per call; a call with a bad handle may crash the
 * original, so stdout is unbuffered. */
#include <windows.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;
static HMODULE ace;
/* The connection's path, the folder listed for the table files, the folder
 * listed for transaction logs (none if empty), and the server type. */
static char dir[MAX_PATH], files_dir[MAX_PATH], logs_dir[MAX_PATH];
static int server_type = 1;
#define F(name) ((void *)GetProcAddress(ace, name))

static U32 err(const char *what, U32 rc) {
    U32 code = 0;
    U8 text[600];
    U16 len = sizeof text;
    text[0] = 0;
    if (rc) ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
    else len = 0;
    printf("%-44s rc=%lu %.*s\n", what, rc, (int)len, text);
    return rc;
}

typedef U32(WINAPI *Fh)(H);
#define CALL1(name, h) err(#name "(" #h ")", ((Fh)F(#name))(h))

static void state(const char *label, H c) {
    U16 in = 99, count = 99;
    U32 r1 = ((U32(WINAPI *)(H, U16 *))F("AdsInTransaction"))(c, &in);
    U32 r2 = ((U32(WINAPI *)(H, U32 *))F("AdsGetTransactionCount"))(c, (U32 *)&count);
    printf("  [%s] InTransaction rc=%lu flag=%u  TransactionCount rc=%lu count=%u\n", label, r1, in,
           r2, count);
}

static H conn_new(void) {
    H c = 0;
    /* A connection string (Data Source=...;CommType=TCP_IP) goes through
     * AdsConnect101. */
    if (!_strnicmp(dir, "Data Source", 11))
        err("AdsConnect101",
            ((U32(WINAPI *)(U8 *, H *, H *))F("AdsConnect101"))((U8 *)dir, NULL, &c));
    else
        err("AdsConnect26", ((U32(WINAPI *)(U8 *, U16, H *))F("AdsConnect26"))(
                                (U8 *)dir, (U16)server_type, &c));
    return c;
}

static void sql(H c, const char *text) {
    H s = 0, cur = 0;
    ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(c, &s);
    char what[80];
    snprintf(what, sizeof what, "sql %.38s", text);
    err(what, ((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(s, (U8 *)text, &cur));
    if (cur) ((Fh)F("AdsCloseTable"))(cur);
    ((Fh)F("AdsCloseSQLStatement"))(s);
}

/* The connection's folder: `dir`, or a connection string's Data Source. */
static const char *folder(void) {
    static char path[MAX_PATH];
    if (_strnicmp(dir, "Data Source=", 12)) return dir;
    snprintf(path, sizeof path, "%s", dir + 12);
    path[strcspn(path, ";")] = 0;
    return path;
}

/* Whether the connection is to a data dictionary (a path to an .add). */
static int dictionary(void) {
    return strstr(dir, ".add") || strstr(dir, ".ADD");
}

/* The table type to open tables as: a dictionary's (ADS_DEFAULT), or ADT. */
static U16 table_type(void) {
    return dictionary() ? 0 : 3;
}

static H open_t(H c, const char *short_name) {
    H t = 0;
    char name[MAX_PATH];
    /* A dictionary's tables by their names. Else, as the original takes a
     * relative name from the process's folder, the name in the connection's
     * folder (a connection string's Data Source). */
    if (dictionary()) snprintf(name, sizeof name, "%s", short_name);
    else snprintf(name, sizeof name, "%s\\%s", folder(), short_name);
    char what[80];
    snprintf(what, sizeof what, "AdsOpenTable %s", name);
    err(what, ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
                  c, (U8 *)name, NULL, table_type(), 1, 1, 2, 4 /* shared */, &t));
    return t;
}

static void field(const char *label, H t, const char *name) {
    U8 buf[64];
    U32 len = sizeof buf;
    U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U32 *, U16))F("AdsGetField"))(t, (U8 *)name, buf, &len, 0);
    U32 n = 0, rec = 0;
    ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(t, 0, &n);
    ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordNum"))(t, 0, &rec);
    printf("  [%s] rec %lu %s=%.*s (rc=%lu) count=%lu\n", label, rec, name, (int)len, buf, rc, n);
}

static void set(H t, const char *name, const char *v) {
    char what[80];
    snprintf(what, sizeof what, "AdsSetString %s=%s", name, v);
    err(what, ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetString"))(t, (U8 *)name, (U8 *)v, strlen(v)));
}

static void go(H t, U32 n) {
    char what[40];
    snprintf(what, sizeof what, "AdsGotoRecord %lu", n);
    err(what, ((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(t, n));
}

/* The files of `folder`, with sizes and a checksum. */
static void list(const char *label, const char *folder, const char *pattern) {
    char mask[MAX_PATH];
    snprintf(mask, sizeof mask, "%s\\%s", folder, pattern);
    WIN32_FIND_DATAA fd;
    HANDLE h = FindFirstFileA(mask, &fd);
    printf("  [files %s]", label);
    if (h == INVALID_HANDLE_VALUE) return (void)puts(" none");
    do {
        if (fd.cFileName[0] == '.') continue;
        char path[MAX_PATH];
        snprintf(path, sizeof path, "%s\\%s", folder, fd.cFileName);
        HANDLE f = CreateFileA(path, GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE, NULL,
                               OPEN_EXISTING, 0, NULL);
        U32 sum = 0;
        if (f != INVALID_HANDLE_VALUE) {
            U8 b[4096];
            DWORD got;
            while (ReadFile(f, b, sizeof b, &got, NULL) && got)
                for (DWORD i = 0; i < got; i++) sum = sum * 31 + b[i];
            CloseHandle(f);
        }
        printf(" %s:%lu:%08lx", fd.cFileName, fd.nFileSizeLow, sum);
    } while (FindNextFileA(h, &fd));
    FindClose(h);
    puts("");
}

/* The table files, and the transaction logs if their folder is known. */
static void files(const char *label) {
    list(label, files_dir, "*");
    if (*logs_dir) {
        char logs[MAX_PATH];
        snprintf(logs, sizeof logs, "%s logs", label);
        list(logs, logs_dir, "_T*");
    }
}

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IONBF, 0);
    ace = LoadLibraryA(argv[1]);
    if (!ace) return 1;
    strcpy(dir, argv[2]);
    strcpy(files_dir, argc > 3 ? argv[4] : argv[2]);
    if (argc > 3) server_type = atoi(argv[3]);
    if (argc > 5) strcpy(logs_dir, argv[5]);
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))((U16)server_type);
    H c = conn_new(), c2 = conn_new();

    puts("== setup");
    sql(c, "CREATE TABLE t (id integer, name char(10), m memo)");
    sql(c, "CREATE INDEX name ON t (name)");
    sql(c, "INSERT INTO t VALUES (1, 'one', 'memo one')");
    sql(c, "INSERT INTO t VALUES (2, 'two', NULL)");
    files("start");

    puts("== API answers without a transaction");
    state("c", c);
    state("0", 0);
    CALL1(AdsCommitTransaction, c);
    CALL1(AdsRollbackTransaction, c);
    CALL1(AdsCommitTransaction, 0);
    CALL1(AdsRollbackTransaction, 0);
    err("AdsCreateSavepoint c sp1",
        ((U32(WINAPI *)(H, U8 *, U32))F("AdsCreateSavepoint"))(c, (U8 *)"sp1", 0));
    err("AdsRollbackTransaction80 c sp1",
        ((U32(WINAPI *)(H, U8 *, U32))F("AdsRollbackTransaction80"))(c, (U8 *)"sp1", 0));

    puts("== nesting");
    CALL1(AdsBeginTransaction, c);
    state("c", c);
    state("c2", c2);
    CALL1(AdsBeginTransaction, c);
    state("c", c);
    CALL1(AdsCommitTransaction, c);
    state("c", c);
    CALL1(AdsCommitTransaction, c);
    state("c", c);
    CALL1(AdsCommitTransaction, c);
    puts("-- begin(0)");
    CALL1(AdsBeginTransaction, 0);
    state("c", c);
    state("c2", c2);
    CALL1(AdsRollbackTransaction, 0);
    state("c", c);
    state("c2", c2);

    puts("== savepoints");
    CALL1(AdsBeginTransaction, c);
    err("AdsCreateSavepoint c sp1",
        ((U32(WINAPI *)(H, U8 *, U32))F("AdsCreateSavepoint"))(c, (U8 *)"sp1", 0));
    err("AdsCreateSavepoint c NULL",
        ((U32(WINAPI *)(H, U8 *, U32))F("AdsCreateSavepoint"))(c, NULL, 0));
    err("AdsRollbackTransaction80 c nosuch",
        ((U32(WINAPI *)(H, U8 *, U32))F("AdsRollbackTransaction80"))(c, (U8 *)"nosuch", 0));
    err("AdsRollbackTransaction80 c sp1",
        ((U32(WINAPI *)(H, U8 *, U32))F("AdsRollbackTransaction80"))(c, (U8 *)"sp1", 0));
    state("c", c);
    err("AdsRollbackTransaction80 c NULL",
        ((U32(WINAPI *)(H, U8 *, U32))F("AdsRollbackTransaction80"))(c, NULL, 0));
    state("c", c);

    puts("== a change in a transaction, rolled back");
    H t = open_t(c, "t"), t2 = open_t(c2, "t");
    CALL1(AdsBeginTransaction, c);
    files("after begin");
    go(t, 1);
    set(t, "name", "uno");
    CALL1(AdsWriteRecord, t);
    files("after write");
    field("t", t, "name");
    go(t2, 1);
    field("t2 (c2)", t2, "name");
    err("AdsIsRecordLocked t2 1", 0);
    {
        U16 locked = 9;
        ((U32(WINAPI *)(H, U32, U16 *))F("AdsIsRecordLocked"))(t2, 1, &locked);
        printf("  t2 sees record 1 locked=%u\n", locked);
        ((U32(WINAPI *)(H, U32, U16 *))F("AdsIsRecordLocked"))(t, 1, &locked);
        printf("  t sees record 1 locked=%u\n", locked);
    }
    err("AdsLockRecord t2 1", ((U32(WINAPI *)(H, U32))F("AdsLockRecord"))(t2, 1));
    err("AdsUnlockRecord t 1", ((U32(WINAPI *)(H, U32))F("AdsUnlockRecord"))(t, 1));
    puts("-- append in the transaction");
    CALL1(AdsAppendRecord, t);
    set(t, "id", "3");
    set(t, "name", "three");
    CALL1(AdsWriteRecord, t);
    field("t", t, "name");
    files("after append");
    puts("-- operations illegal in a transaction?");
    CALL1(AdsPackTable, t);
    CALL1(AdsZapTable, t2);
    CALL1(AdsReindex, t);
    CALL1(AdsCloseTable, t);
    t = open_t(c, "t");
    CALL1(AdsRollbackTransaction, c);
    files("after rollback");
    go(t, 1);
    field("t", t, "name");
    go(t2, 1);
    field("t2", t2, "name");

    puts("== committed");
    CALL1(AdsBeginTransaction, c);
    go(t, 2);
    set(t, "name", "dos");
    CALL1(AdsWriteRecord, t);
    CALL1(AdsCommitTransaction, c);
    go(t2, 2);
    field("t2", t2, "name");
    files("after commit");

    err("AdsUnlockRecord t2 1", ((U32(WINAPI *)(H, U32))F("AdsUnlockRecord"))(t2, 1));
    puts("== exclusive table in a transaction");
    sql(c, "CREATE TABLE u (id integer, name char(10))");
    sql(c, "INSERT INTO u VALUES (1, 'a')");
    {
        H u = 0;
        char name[MAX_PATH];
        if (dictionary()) strcpy(name, "u");
        else snprintf(name, sizeof name, "%s\\u", folder());
        err("AdsOpenTable u exclusive", ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
                  c, (U8 *)name, NULL, table_type(), 1, 1, 2, 1, &u));
        CALL1(AdsBeginTransaction, c);
        err("AdsSetTableTransactionFree u 1", ((U32(WINAPI *)(H, U16))F("AdsSetTableTransactionFree"))(u, 1));
        CALL1(AdsReindex, u);
        CALL1(AdsPackTable, u);
        CALL1(AdsZapTable, u);
        CALL1(AdsCloseTable, u);
        CALL1(AdsRollbackTransaction, c);
    }
    puts("== transaction-free table, disconnect in transaction");
    err("AdsSetTableTransactionFree t",
        ((U32(WINAPI *)(H))F("AdsSetTableTransactionFree"))(t));
    CALL1(AdsBeginTransaction, c);
    CALL1(AdsCloseTable, t);
    CALL1(AdsDisconnect, c);
    state("c after disconnect", c);

    puts("== SQL statements");
    c = conn_new();
    sql(c, "BEGIN TRANSACTION");
    state("c", c);
    sql(c, "SAVEPOINT a");
    sql(c, "UPDATE t SET name = 'eins' WHERE id = 1");
    sql(c, "ROLLBACK TO SAVEPOINT a");
    {
        H e = open_t(c, "t");
        ((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(e, 1);
        field("after SQL rollback to a", e, "name");
        ((Fh)F("AdsCloseTable"))(e);
    }
    sql(c, "ROLLBACK TO a");
    sql(c, "COMMIT WORK");
    state("c", c);
    sql(c, "COMMIT");
    sql(c, "ROLLBACK");
    sql(c, "BEGIN TRAN");
    sql(c, "ROLLBACK WORK");
    sql(c, "EXECUTE PROCEDURE sp_IgnoreTableTransactions('t')");
    puts("-- SQL and API mixed, fresh connection");
    CALL1(AdsDisconnect, c);
    c = conn_new();
    sql(c, "COMMIT");
    sql(c, "ROLLBACK TO SAVEPOINT nosuch");
    sql(c, "SAVEPOINT x");
    sql(c, "BEGIN TRANSACTION");
    sql(c, "BEGIN TRANSACTION");
    state("c after 2 SQL BEGIN", c);
    CALL1(AdsCommitTransaction, c);
    CALL1(AdsBeginTransaction, c);
    sql(c, "COMMIT");
    state("c after API begin, SQL commit", c);
    sql(c, "SET TRANSACTION AUTOCOMMIT_ON");
    sql(c, "ROLLBACK TO SAVEPOINT nosuch");
    sql(c, "COMMIT");
    sql(c, "COMMIT");
    state("c", c);
    {
        U16 in = 9;
        err("AdsInTransaction 0", ((U32(WINAPI *)(H, U16 *))F("AdsInTransaction"))(0, &in));
        printf("  flag=%u\n", in);
        err("AdsInTransaction 777", ((U32(WINAPI *)(H, U16 *))F("AdsInTransaction"))(777, &in));
    }

    puts("-- values at the end");
    {
        H e = open_t(c, "t");
        for (U32 r = 1; r <= 4; r++) {
            if (((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(e, r)) break;
            field("end", e, "name");
        }
        ((Fh)F("AdsCloseTable"))(e);
    }
    puts("== recovery");
    err("AdsFailedTransactionRecovery NULL",
        ((U32(WINAPI *)(U8 *))F("AdsFailedTransactionRecovery"))(NULL));
    err("AdsFailedTransactionRecovery dir",
        ((U32(WINAPI *)(U8 *))F("AdsFailedTransactionRecovery"))((U8 *)dir));
    files("end");
    return 0;
}
