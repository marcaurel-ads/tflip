/*
 * A typical ACE client session: process-wide settings, a connection to a
 * remote server, a few queries, and the teardown.
 * crates/tflip-protocol/examples/session.rs does the same on the engine API,
 * locally or on a tflipd.
 *
 * Build against the SDK's ace.h and the tflip-ace library, such as:
 *
 *     cargo build -p tflip-ace
 *     cc -I<acesdk> session.c -Ltarget/debug -ltflip_ace -o session
 *     ./session '\\server:6262\data\app.add' user password
 *
 * On Linux, ace.h includes unixutils.h from the Linux SDK.
 *
 * The connection to the server uses TLS, which the ACE API knows nothing
 * of: tflip-ace trusts the server by the fingerprint of its certificate,
 * which tflipd prints when it starts. It comes from
 * TFLIP_SERVER_FINGERPRINT, or the [servers] section of tflip.ini next to
 * the library (see crates/tflip-ace/src/config.rs):
 *
 *     TFLIP_SERVER_FINGERPRINT=sha256:... ./session '\\server:6262\data' ...
 */

#include <stdio.h>
#include <stdlib.h>

#include "ace.h"

/* Prints the last error and exits if `call` fails. */
#define CHECK(call)                                                        \
    do {                                                                   \
        if ((call) != AE_SUCCESS)                                          \
            fail(#call);                                                   \
    } while (0)

static void fail(const char *call)
{
    UNSIGNED32 code = 0;
    UNSIGNED8 message[ADS_MAX_ERROR_LEN + 1];
    UNSIGNED16 len = sizeof message;

    AdsGetLastError(&code, message, &len);
    fprintf(stderr, "%s: %lu %s\n", call, (unsigned long)code, message);
    AdsApplicationExit();
    exit(1);
}

/* Prints the rows of `cursor` from the top, every field as text. */
static void print_rows(ADSHANDLE cursor)
{
    UNSIGNED16 fields = 0;
    UNSIGNED16 eof = 0;
    UNSIGNED16 i;

    CHECK(AdsGetNumFields(cursor, &fields));
    for (i = 1; i <= fields; i++) {
        UNSIGNED8 name[ADS_MAX_FIELD_NAME + 1];
        UNSIGNED16 len = sizeof name;

        CHECK(AdsGetFieldName(cursor, i, name, &len));
        printf("%s%s", i > 1 ? " | " : "", name);
    }
    printf("\n");

    CHECK(AdsGotoTop(cursor));
    CHECK(AdsAtEOF(cursor, &eof));
    while (!eof) {
        for (i = 1; i <= fields; i++) {
            UNSIGNED8 value[256];
            UNSIGNED32 len = sizeof value;

            /* A field number in place of the name pointer is allowed. */
            CHECK(AdsGetField(cursor, ADSFIELD(i), value, &len, ADS_TRIM));
            printf("%s%s", i > 1 ? " | " : "", value);
        }
        printf("\n");
        CHECK(AdsSkip(cursor, 1));
        CHECK(AdsAtEOF(cursor, &eof));
    }
}

int main(int argc, char **argv)
{
    char *path = argc > 1 ? argv[1] : "\\\\server:6262\\data\\app.add";
    char *user = argc > 2 ? argv[2] : "adssys";
    char *password = argc > 3 ? argv[3] : "";
    ADSHANDLE connection = 0;
    ADSHANDLE statement = 0;
    ADSHANDLE cursor = 0;

    /* 1. Settings: they belong to the process, not to a connection, and
     *    apply to connections opened later too. */
    CHECK(AdsShowDeleted(ADS_FALSE));
    CHECK(AdsSetDateFormat((UNSIGNED8 *)"YYYY-MM-DD"));
    CHECK(AdsSetEpoch(1950));
    CHECK(AdsSetDecimals(2));
    CHECK(AdsSetExact(ADS_FALSE));

    /* 2. Connect to a data dictionary (or a folder of free tables) on a
     *    remote server only. */
    CHECK(AdsConnect60((UNSIGNED8 *)path, ADS_REMOTE_SERVER,
                       (UNSIGNED8 *)user, (UNSIGNED8 *)password,
                       ADS_DEFAULT, &connection));

    /* 3a. Run SQL directly. A cursor is a table handle: the table API
     *     reads it, and AdsCloseTable closes it. */
    CHECK(AdsCreateSQLStatement(connection, &statement));
    CHECK(AdsStmtSetTableType(statement, ADS_ADT)); /* for free tables */
    CHECK(AdsExecuteSQLDirect(statement, (UNSIGNED8 *)"SELECT * FROM customers",
                              &cursor));
    print_rows(cursor);
    CHECK(AdsCloseTable(cursor));

    /* 3b. Or prepare it, and set its parameters on the statement. The
     *     statement is reused. */
    CHECK(AdsPrepareSQL(statement,
                        (UNSIGNED8 *)"SELECT :n * 2 AS twice FROM system.iota"));
    CHECK(AdsSetLong(statement, (UNSIGNED8 *)"n", 21));
    CHECK(AdsExecuteSQL(statement, &cursor));
    print_rows(cursor);
    CHECK(AdsCloseTable(cursor));
    CHECK(AdsCloseSQLStatement(statement));

    /* 4. Disconnect, which closes whatever is still open on the connection,
     *    and let the client engine go. */
    CHECK(AdsDisconnect(connection));
    AdsApplicationExit();
    return 0;
}
