/* Records what an Advantage Client Engine does when a client writes through
 * the ACE API, for fixtures/write/ and fixtures/fields/setters.json.
 *
 *     write.exe <ace32.dll> <data folder> <script>  > result.json
 *
 * Built with i686-w64-mingw32-gcc and run under Wine (see run.sh). The data
 * folder is connected to as a free-table folder (local server), and table
 * and index names are taken as files in it; the script
 * has one command per line, and each one's result is a JSON object with the
 * command, its return code ("rc") and, when a call fails, AdsGetLastError's
 * text ("error"):
 *
 *     # comment
 *     sql <statement>          AdsExecuteSQLDirect without a cursor (to set up
 *                              tables: CREATE TABLE, CREATE INDEX, INSERT)
 *     tabletype <type>         the table type of later opens and SQL
 *                              statements (AdsStmtSetTableType): adt (the
 *                              default), ntx, cdx, vfp, database
 *                              (ADS_DATABASE_TABLE) or a number
 *     chartype <type>          their character type (AdsStmtSetTableCharType):
 *                              ansi (the default) or oem
 *     open <table> [opts]      AdsOpenTable (proprietary locking, the table and
 *                              character type set above); opts: exclusive
 *                              (the default), shared, readonly or a number.
 *                              The new table is the current one; tables are
 *                              numbered from 1
 *     open2 <table> [opts]     AdsOpenTable on a second connection, as
 *                              another user sees the table
 *     use <n>                  makes table n the current one; use conn,
 *                              the connection's handle (for the calls on
 *                              the current table that take another)
 *     close                    AdsCloseTable
 *     ddcreate <file>          AdsDDCreate of a data dictionary in the data
 *                              folder; the commands after it (sql, cursor)
 *                              are on its connection
 *     ddconnect <file> [<user> [<password>]]   (- for no password)
 *                              AdsConnect60 to a dictionary of the data
 *                              folder, as the commands after it use
 *     cursor <select>          AdsExecuteSQLDirect on a statement of its own
 *                              (with the table and character type above and
 *                              the `constrain` and `readall` settings); the
 *                              cursor it opens is the current table, as open
 *                              makes one, and what the commands on a table
 *                              do they do to it
 *     constrain <1|2>  readall <1|2>  cursorreadonly <0|1>
 *                              AdsStmtConstrainUpdates, AdsStmtReadAllColumns
 *                              and AdsStmtSetTableReadOnly on the statements
 *                              of later cursors
 *     cursor2 <select>         the same on the second connection (after open2)
 *     column <field>           AdsGetField of the field in every record from
 *                              the top to EOF ("values", text)
 *     closecursor              AdsCloseSQLStatement of the current cursor's
 *                              statement
 *     stmtset <constrain|readall|readonly> <n>
 *                              AdsStmtConstrainUpdates, AdsStmtReadAllColumns
 *                              or AdsStmtSetTableReadOnly on the current
 *                              cursor's statement, which is still open
 *     openindex <file>         AdsOpenIndex
 *     append write delete recall cancel writeall refresh
 *                              AdsAppendRecord, AdsWriteRecord,
 *                              AdsDeleteRecord, AdsRecallRecord,
 *                              AdsCancelUpdate, AdsWriteAllRecords,
 *                              AdsRefreshRecord
 *     top bottom               AdsGotoTop, AdsGotoBottom
 *     goto <n>  skip <n>       AdsGotoRecord, AdsSkip
 *     lock <n>  unlock <n>     AdsLockRecord, AdsUnlockRecord
 *     locktable unlocktable    AdsLockTable, AdsUnlockTable
 *     islocked <n>             AdsIsRecordLocked ("locked")
 *     tablelocked              AdsIsTableLocked ("locked")
 *     locks                    AdsGetAllLocks ("locks")
 *     set <field> <kind> [v]   AdsSet<kind> on the current table; kinds:
 *                              string stringw field fieldw (text), stringu
 *                              fieldu (UTF-16LE hex), long
 *                              longlong short julian ms money logical
 *                              (integers), double, date time timestamp
 *                              (text), binary image tsraw (hex), null,
 *                              empty
 *     get <field>              AdsIsNull ("null") and AdsGetField ("value",
 *                              hex)
 *     getopt <field> <option> [len]
 *                              AdsGetField with an option and a buffer of
 *                              len bytes (200) ("len", "value", hex)
 *     setbin <field> <type> <total> <offset> <hex>
 *                              AdsSetBinary with the type, total length and
 *                              offset given: a value in pieces
 *     handletype [<order>]     AdsGetHandleType of the current table (of
 *                              the connection after use conn) ("type"), or
 *                              of the order <order> of the table
 *     lasterror                AdsGetLastError's code and text as they
 *                              stand ("code", "text")
 *     record                   AdsGetRecord ("record", hex)
 *     raw <field>              the field's bytes in AdsGetRecord's record ("raw",
 *                              hex): from AdsGetFieldOffset up to the next
 *                              field's offset or the record's end
 *     state                    AdsGetRecordNum, AdsGetRecordCount,
 *                              AdsIsRecordDeleted and AdsGetLastAutoinc
 *     enableenc <password>     AdsEnableEncryption on the current table
 *     encrypttable decrypttable encryptrecord decryptrecord disableenc
 *                              AdsEncryptTable, AdsDecryptTable,
 *                              AdsEncryptRecord, AdsDecryptRecord (the
 *                              current record), AdsDisableEncryption
 *     encstate                 AdsIsEncryptionEnabled, AdsIsRecordEncrypted
 *                              and AdsIsTableEncrypted ("enabled", "record",
 *                              "table", each [rc, value])
 *     info                     AdsGetRecordLength, AdsGetNumFields,
 *                              AdsGetMemoBlockSize, AdsIsTableEncrypted and
 *                              AdsGetTableType, each [rc, value]
 *     showdeleted <0|1>        AdsShowDeleted
 *     exact <0|1>              AdsSetExact
 *     collation <name>         AdsSetCollation on the connection: the dynamic
 *                              collation of the tables and indexes made and
 *                              opened from then on
 *     aof <options> <expr>     AdsSetAOF with the options (a number)
 *     clearaof refreshaof      AdsClearAOF, AdsRefreshAOF
 *     filter <expr>            AdsSetFilter
 *     clearfilter              AdsClearFilter
 *     aoflevel                 AdsGetAOFOptLevel ("level", "nonopt")
 *     evalaof <expr>           AdsEvalAOF ("level")
 *     inaof <n>                AdsIsRecordInAOF ("flag")
 *     custom <option> <n>...   AdsCustomizeAOF
 *     order <name>|-           moves through the current table's index
 *                              order (AdsGetIndexHandle), or in record order
 *     otop obottom oskip <n>   AdsGotoTop, AdsGotoBottom, AdsSkip on the
 *                              current table's order (set with `order`), or
 *                              on the table without one
 *     seek <type> <text>       AdsSeek of a text key in the order, with the
 *                              seek type (1 soft, 2 hard, 4 greater)
 *                              ("found")
 *     pos                      AdsGetRecordNum (filters ignored), AdsAtBOF,
 *                              AdsAtEOF and AdsIsFound of the current table
 *                              ("pos": [recno, bof, eof, found], and their
 *                              return codes "rcs"; the return code is 0)
 *     scope <1|2> <text>       AdsSetScope of a text key, top or bottom, on
 *                              the order
 *     getscope <1|2>           AdsGetScope ("scope", hex)
 *     clearscope <1|2>         AdsClearScope
 *     relation <child> <expr>  AdsSetRelation from the current table to the
 *                              order of table <child> (set with `order`; its
 *                              table handle if none, or with t<n> anyway; 0
 *                              for the handle 0); an expression - is NULL
 *     scopedrelation <child> <expr>
 *                              AdsSetScopedRelation, the same way
 *     clearrelation            AdsClearRelation
 *     rows                     the record numbers from the top to EOF with
 *                              filters respected, in the order ("rows"), and
 *                              AdsGetRecordCount and in an order
 *                              AdsGetKeyCount, both respecting filters
 *                              ("count", "keys")
 *     count                    the same counts alone, without moving
 *     createtable <name> <memo size> <fields>
 *                              AdsCreateTable71 with the table type and
 *                              character type set above; the new table is
 *                              the current one (as open does)
 *     createindex <file|-> <tag> <options> <page size> <expr>[ FOR <cond>]
 *                              AdsCreateIndex61 on the current table (file
 *                              - for its structural index)
 *     deleteindex <tag>        AdsGetIndexHandle and AdsDeleteIndex
 *     reindex pack zap         AdsReindex, AdsPackTable, AdsZapTable
 *     reindex61 <page size>    AdsReindex61
 *     createfts <file|-> <tag> <field> <page size> <min> <max> <options>
 *               <use default delimiters> <delimiters> <use default noise>
 *               <noise> <use default drop> <drop> <use default conditionals>
 *               <conditionals>
 *                              AdsCreateFTSIndex on the current table; each
 *                              list escaped as for set (\x20 for a blank),
 *                              - for NULL, "" for an empty one
 *     reindexfts <page size>   AdsReindexFTS
 *     recallall deletetable flush activateaof deactivateaof
 *                              AdsRecallAllRecords, AdsDeleteTable,
 *                              AdsFlushFileBuffers, AdsActivateAOF,
 *                              AdsDeactivateAOF
 *     pack120 <size> <options> AdsPackTable120
 *     conn <function> [n]      a function of the connection alone, with a
 *                              flag if given (AdsSetFlushFlag, AdsEnableRI,
 *                              ...)
 *     links                    AdsGetNumActiveLinks ("links")
 *     transfree <n>            AdsSetTableTransactionFree, then
 *                              AdsIsTableTransactionFree ("free")
 *     keycolumn [len]          AdsGetKeyColumn ("len", "value")
 *     lookup <tag> <type> <key>
 *                              AdsLookupKey ("found"): type 1 a raw key in
 *                              hex, 2 text, 4 a double
 *     keyfromrecord <tag> [hex]
 *                              AdsBuildKeyFromRecord on the current record
 *                              or the one given ("key")
 *     setrecord <hex> [len]    AdsSetRecord
 *     fieldraw <field> <hex>   AdsSetFieldRaw
 *     vfpnull <field> <options>
 *                              AdsAccessVfpSystemField on the current record
 *                              ("flag")
 *     rawkey <tag> <options> [<field>=<value>;...]
 *                              AdsInitRawKey, AdsSetField on the order (an
 *                              empty value AdsSetEmpty), AdsBuildRawKey100
 *                              ("key")
 *     stepkey <tag> <direction> <hex key>
 *                              AdsStepIndexKey ("key")
 *     locate <0|1> <expr>      AdsLocate, backwards or forwards ("found")
 *     continue                 AdsContinue ("found")
 *     filetobinary <field> <type> <file>
 *                              AdsFileToBinary of a file in the data folder
 *     cpstring <field> <code page> <text>
 *                              AdsSetStringFromCodePage
 *     addcustom <tag>  delcustom <tag>
 *                              AdsAddCustomKey, AdsDeleteCustomKey
 *     copytable <filter> <file>
 *                              AdsCopyTable of the current table (or of its
 *                              order, set with `order`) to a file in the
 *                              data folder; filter 1 respects filters, 2
 *                              ignores them, 3 respects scopes
 *     copystructure <file>     AdsCopyTableStructure
 *     convert <filter> <type> <file>
 *                              AdsConvertTable to a table of the type (adt,
 *                              ntx, cdx, vfp)
 *     copycontents <table n> <filter>
 *                              AdsCopyTableContents of the current table (or
 *                              its order) to table n
 *     restructure <table> <block size> <add>|<delete>|<change>
 *                              AdsRestructureTable120 with the table and
 *                              character types set above (each list may be
 *                              empty); :<name> for a dictionary's table
 *
 *     begin commit rollback    AdsBeginTransaction, AdsCommitTransaction,
 *                              AdsRollbackTransaction on the connection
 *     savepoint <name>         AdsCreateSavepoint
 *     rollbackto <name>        AdsRollbackTransaction80 to the savepoint
 *     txcount                  AdsGetTransactionCount ("count")
 *     snapshot <name>          copies the data folder's files (WRITE_FILES
 *                              if the process reaches them by another path)
 *                              into WRITE_SNAPS\<name>, to compare the files
 *                              in the middle of the script
 *
 * With WRITE_CONNECT set to a connection string (Data Source=...;
 * ServerType=REMOTE;CommType=TCP_IP) it connects through AdsConnect101, to
 * a remote server; the data folder is then that server's path for it.
 *
 * A <field> is a name, or #n for field number n. Text values run to the end
 * of the line; in them \r, \n, \t, \\ and \xHH stand for those bytes,
 * \@NNNNNc for NNNNN (five digits) times the character c, and an empty
 * value is given as "". The data folder is written as <data> wherever it
 * appears.
 *
 * With WRITE_STACK_FILL set to a byte value (0x5A, ...), the stack below
 * main's frame is filled with it before every call, and so are the bytes
 * after a binary value's, so that bytes the engine writes without
 * initialising them, or reads past the end of what it was given, differ
 * between runs with different fills (run.sh compares two runs to find
 * them). */
#include <windows.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef long S32;
typedef short S16;
typedef long long S64;
typedef ULONG_PTR H;

static HMODULE ace;
/* The WRITE_STACK_FILL byte; -1 for none. */
static int stack_fill = -1;
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

static void hex(const char *key, const U8 *bytes, size_t n) {
    printf(", \"%s\": \"", key);
    for (size_t i = 0; i < n; i++) printf("%02x", bytes[i]);
    printf("\"");
}

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

/* Replaces the escapes in s by their bytes, in place; returns the length.
 * \@NNNNNc expands, so s must have room for the expansion (see expand). */
static size_t unescape(char *s) {
    if (!strcmp(s, "\"\"")) return *s = 0;
    char *start = s, *out = s;
    for (; *s; s++) {
        if (*s != '\\' || !s[1]) {
            *out++ = *s;
            continue;
        }
        s++;
        if (*s == '@' && strlen(s) >= 7) {
            char digits[6] = {s[1], s[2], s[3], s[4], s[5], 0};
            long n = strtol(digits, NULL, 10);
            /* The expansion overtakes the text after it: move that first. */
            char *rest = s + 7;
            size_t tail = strlen(rest) + 1;
            if (out + n > rest) {
                memmove(out + n, rest, tail);
                rest = out + n;
            }
            memset(out, s[6], n);
            out += n;
            s = rest - 1;
            continue;
        }
        if (*s == 'x' && s[1] && s[2]) {
            char digits[3] = {s[1], s[2], 0};
            *out++ = (char)strtoul(digits, NULL, 16);
            s += 2;
        } else *out++ = *s == 'r' ? '\r' : *s == 'n' ? '\n' : *s == 't' ? '\t' : *s;
    }
    *out = 0;
    return (size_t)(out - start);
}

/* The next space-separated word of *s, which moves past it. */
static char *word(char **s) {
    while (**s == ' ') (*s)++;
    char *w = *s;
    while (**s && **s != ' ') (*s)++;
    if (**s) *(*s)++ = 0;
    return w;
}

static U8 *field_arg(char *name) {
    if (*name == '#') return FIELD(atoi(name + 1));
    return (U8 *)name;
}

static size_t from_hex(const char *s, U8 *out) {
    size_t n = 0;
    for (; s[0] && s[1]; s += 2) {
        char digits[3] = {s[0], s[1], 0};
        out[n++] = (U8)strtoul(digits, NULL, 16);
    }
    return n;
}

static U32 set(H t, U8 *field, const char *kind, char *value) {
    static U8 bytes[70000];
    static WCHAR wide[70000];
    if (!strcmp(kind, "null")) return ((U32(WINAPI *)(H, U8 *))F("AdsSetNull"))(t, field);
    if (!strcmp(kind, "empty")) return ((U32(WINAPI *)(H, U8 *))F("AdsSetEmpty"))(t, field);
    if (!strcmp(kind, "binary") || !strcmp(kind, "image")) {
        size_t n = from_hex(value, bytes);
        /* The bytes after the value: the run's fill (see main), so that what
         * the engine reads past the end differs between runs. */
        memset(bytes + n, stack_fill >= 0 ? stack_fill : 0, 16);
        U16 type = kind[0] == 'b' ? 6 : 7;
        return ((U32(WINAPI *)(H, U8 *, U16, U32, U32, U8 *, U32))F("AdsSetBinary"))(
            t, field, type, (U32)n, 0, bytes, (U32)n);
    }
    if (!strcmp(kind, "tsraw")) {
        size_t n = from_hex(value, bytes);
        return ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetTimeStampRaw"))(t, field, bytes,
                                                                            (U32)n);
    }
    if (!strcmp(kind, "double"))
        return ((U32(WINAPI *)(H, U8 *, double))F("AdsSetDouble"))(t, field, strtod(value, NULL));
    if (!strcmp(kind, "long") || !strcmp(kind, "julian") || !strcmp(kind, "ms")) {
        const char *fn = kind[0] == 'l' ? "AdsSetLong" : kind[0] == 'j' ? "AdsSetJulian"
                                                                        : "AdsSetMilliseconds";
        return ((U32(WINAPI *)(H, U8 *, S32))F(fn))(t, field, strtol(value, NULL, 10));
    }
    if (!strcmp(kind, "longlong") || !strcmp(kind, "money")) {
        const char *fn = kind[0] == 'l' ? "AdsSetLongLong" : "AdsSetMoney";
        return ((U32(WINAPI *)(H, U8 *, S64))F(fn))(t, field, _strtoi64(value, NULL, 10));
    }
    if (!strcmp(kind, "short"))
        return ((U32(WINAPI *)(H, U8 *, S16))F("AdsSetShort"))(t, field,
                                                               (S16)strtol(value, NULL, 10));
    if (!strcmp(kind, "logical"))
        return ((U32(WINAPI *)(H, U8 *, U16))F("AdsSetLogical"))(t, field,
                                                                 (U16)strtoul(value, NULL, 10));
    if (!strcmp(kind, "stringu") || !strcmp(kind, "fieldu")) {
        /* Text beyond Windows-1252, as the hex of its UTF-16LE bytes. */
        size_t bytes_n = from_hex(value, bytes);
        memcpy(wide, bytes, bytes_n);
        const char *fn = kind[0] == 's' ? "AdsSetStringW" : "AdsSetFieldW";
        return ((U32(WINAPI *)(H, U8 *, WCHAR *, U32))F(fn))(t, field, wide,
                                                             (U32)(bytes_n / 2));
    }
    size_t n = unescape(value);
    if (!strcmp(kind, "date") || !strcmp(kind, "time")) {
        const char *fn = kind[0] == 'd' ? "AdsSetDate" : "AdsSetTime";
        return ((U32(WINAPI *)(H, U8 *, U8 *, U16))F(fn))(t, field, (U8 *)value, (U16)n);
    }
    if (!strcmp(kind, "timestamp"))
        return ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetTimeStamp"))(t, field, (U8 *)value,
                                                                         (U32)n);
    if (!strcmp(kind, "string") || !strcmp(kind, "field")) {
        const char *fn = kind[0] == 's' ? "AdsSetString" : "AdsSetField";
        return ((U32(WINAPI *)(H, U8 *, U8 *, U32))F(fn))(t, field, (U8 *)value, (U32)n);
    }
    if (!strcmp(kind, "stringw") || !strcmp(kind, "fieldw")) {
        /* The text is Windows-1252 bytes, given to the W setter as UTF-16. */
        int units = MultiByteToWideChar(1252, 0, value, (int)n, wide, 70000);
        const char *fn = kind[0] == 's' ? "AdsSetStringW" : "AdsSetFieldW";
        return ((U32(WINAPI *)(H, U8 *, WCHAR *, U32))F(fn))(t, field, wide, (U32)units);
    }
    fprintf(stderr, "unknown setter kind %s\n", kind);
    exit(2);
}

static U16 table_type = 3, char_type = 1;
static int constrain = -1, read_columns = -1, cursor_read_only = -1;
static H cursor_stmts[1024];
static int on_connection;

static U16 type_of(const char *s) {
    if (!strcmp(s, "ntx")) return 1;
    if (!strcmp(s, "cdx")) return 2;
    if (!strcmp(s, "vfp")) return 4;
    if (!strcmp(s, "adt")) return 3;
    if (!strcmp(s, "database")) return 0;
    if (*s >= '0' && *s <= '9') return (U16)atoi(s);
    fprintf(stderr, "unknown table type %s\n", s);
    exit(2);
}

/* Fills the stack below the caller's frame with the WRITE_STACK_FILL byte. */
static void __attribute__((noinline)) fill_stack(void) {
    volatile U8 area[256 * 1024];
    memset((void *)area, stack_fill, sizeof area);
}

static U32 options_of(const char *s) {
    if (!*s || !strcmp(s, "exclusive")) return 1;
    if (!strcmp(s, "shared")) return 4;
    if (!strcmp(s, "readonly")) return 2 | 4;
    return strtoul(s, NULL, 0);
}

typedef U32(WINAPI *Op)(H);

/* Connects to the data folder `folder` with the local server, or through
 * the connection string in WRITE_CONNECT (a remote server). */
static U32 connect_to(const char *folder, H *connection) {
    const char *string = getenv("WRITE_CONNECT");
    if (string && *string)
        return ((U32(WINAPI *)(U8 *, H *, H *))F("AdsConnect101"))((U8 *)string, NULL, connection);
    return ((U32(WINAPI *)(U8 *, U16, H *))F("AdsConnect26"))((U8 *)folder, 1, connection);
}

/* Copies the files of WRITE_FILES (the data folder as this process reaches
 * its files; the data folder if unset) into WRITE_SNAPS\<name>. */
static U32 snapshot(const char *name) {
    const char *from = getenv("WRITE_FILES");
    const char *into = getenv("WRITE_SNAPS");
    if (!from || !*from) from = data_folder;
    if (!into || !*into) return 1;
    char target[MAX_PATH], mask[MAX_PATH];
    snprintf(target, sizeof target, "%s\\%s", into, name);
    CreateDirectoryA(into, NULL);
    CreateDirectoryA(target, NULL);
    snprintf(mask, sizeof mask, "%s\\*", from);
    WIN32_FIND_DATAA found;
    HANDLE h = FindFirstFileA(mask, &found);
    if (h == INVALID_HANDLE_VALUE) return 2;
    do {
        if (found.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) continue;
        char a[MAX_PATH], b[MAX_PATH];
        snprintf(a, sizeof a, "%s\\%s", from, found.cFileName);
        snprintf(b, sizeof b, "%s\\%s", target, found.cFileName);
        if (!CopyFileA(a, b, FALSE)) return 3;
    } while (FindNextFileA(h, &found));
    FindClose(h);
    return 0;
}

int main(int argc, char **argv) {
    if (argc != 4) {
        fprintf(stderr, "usage: write <ace32.dll> <data folder> <script>\n");
        return 2;
    }
    data_folder = argv[2];
    ace = LoadLibraryA(argv[1]);
    if (!ace) return fprintf(stderr, "cannot load %s\n", argv[1]), 1;
    FILE *script = fopen(argv[3], "r");
    if (!script) return fprintf(stderr, "cannot open %s\n", argv[3]), 1;
    const char *fill = getenv("WRITE_STACK_FILL");
    if (fill && *fill) stack_fill = (int)strtoul(fill, NULL, 0) & 0xFF;
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    H connection = 0, connection2 = 0, stmt = 0;
    U32 rc = connect_to(argv[2], &connection);
    if (rc) return fprintf(stderr, "connect: %lu\n", rc), 1;

    H tables[1024] = {0}, orders[1024] = {0};
    int count = 0, current = 0;
    static char line[1 << 20];
    int first = 1;
    printf("[\n");
    while (fgets(line, sizeof line, script)) {
        line[strcspn(line, "\r\n")] = 0;
        if (!*line || *line == '#') continue;
        printf("%s{\"cmd\": ", first ? "" : ",\n");
        first = 0;
        json_bytes((U8 *)line, strlen(line));
        printf(", ");
        char *rest = line;
        char *cmd = word(&rest);
        if (stack_fill >= 0) fill_stack();
        H t = on_connection ? connection : current ? tables[current - 1] : 0;
        static const struct {
            const char *cmd, *fn;
        } simple[] = {{"append", "AdsAppendRecord"}, {"write", "AdsWriteRecord"},
                      {"delete", "AdsDeleteRecord"}, {"recall", "AdsRecallRecord"},
                      {"cancel", "AdsCancelUpdate"}, {"refresh", "AdsRefreshRecord"},
                      {"top", "AdsGotoTop"},         {"bottom", "AdsGotoBottom"},
                      {"locktable", "AdsLockTable"}, {"unlocktable", "AdsUnlockTable"},
                      {"close", "AdsCloseTable"},    {"clearaof", "AdsClearAOF"},
                      {"refreshaof", "AdsRefreshAOF"}, {"clearfilter", "AdsClearFilter"},
                      {"reindex", "AdsReindex"},     {"pack", "AdsPackTable"},
                      {"zap", "AdsZapTable"},       {"recallall", "AdsRecallAllRecords"},
                      {"deletetable", "AdsDeleteTable"}, {"flush", "AdsFlushFileBuffers"},
                      {"activateaof", "AdsActivateAOF"}, {"deactivateaof", "AdsDeactivateAOF"},
                      {"clearrelation", "AdsClearRelation"},
                      {"encrypttable", "AdsEncryptTable"}, {"decrypttable", "AdsDecryptTable"},
                      {"encryptrecord", "AdsEncryptRecord"}, {"decryptrecord", "AdsDecryptRecord"},
                      {"disableenc", "AdsDisableEncryption"}};
        int done = 0;
        for (size_t k = 0; k < sizeof simple / sizeof *simple; k++) {
            if (strcmp(cmd, simple[k].cmd)) continue;
            result(((Op)F(simple[k].fn))(t));
            done = 1;
        }
        if (done) {
        } else if (!strcmp(cmd, "sql")) {
            if (!stmt) ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(connection, &stmt);
            unescape(rest);
            result(((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(stmt, (U8 *)rest,
                                                                           NULL));
        } else if (!strcmp(cmd, "tabletype") || !strcmp(cmd, "chartype")) {
            if (!stmt) ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(connection, &stmt);
            if (cmd[0] == 't') {
                table_type = type_of(rest);
                result(((U32(WINAPI *)(H, U16))F("AdsStmtSetTableType"))(stmt, table_type));
            } else {
                char_type = !strcmp(rest, "oem") ? 2 : 1;
                result(((U32(WINAPI *)(H, U16))F("AdsStmtSetTableCharType"))(stmt, char_type));
            }
        } else if (!strcmp(cmd, "ddcreate") || !strcmp(cmd, "ddconnect")) {
            char path[600];
            snprintf(path, sizeof path, "%s\\%s", data_folder, word(&rest));
            char *user = word(&rest), *password = word(&rest);
            if (!strcmp(password, "-")) password = "";
            H fresh = 0;
            rc = cmd[3] == 'r' ? ((U32(WINAPI *)(U8 *, U16, U8 *, H *))F("AdsDDCreate"))(
                                     (U8 *)path, 0, NULL, &fresh)
                               : ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F(
                                     "AdsConnect60"))((U8 *)path, 1, *user ? (U8 *)user : NULL,
                                                      *user ? (U8 *)password : NULL, 0, &fresh);
            result(rc);
            if (!rc) {
                connection = fresh;
                stmt = 0;
            }
        } else if (!strcmp(cmd, "exact")) {
            result(((U32(WINAPI *)(U16))F("AdsSetExact"))((U16)atoi(rest)));
        } else if (!strcmp(cmd, "collation")) {
            result(((U32(WINAPI *)(H, U8 *))F("AdsSetCollation"))(connection, (U8 *)rest));
        } else if (!strcmp(cmd, "showdeleted")) {
            result(((U32(WINAPI *)(U16))F("AdsShowDeleted"))((U16)atoi(rest)));
        } else if (!strcmp(cmd, "open")) {
            char path[600];
            char *table_name = word(&rest);
            // :<name> is a dictionary's table, given by its name.
            if (table_name[0] == ':')
                snprintf(path, sizeof path, "%s", table_name + 1);
            else
                snprintf(path, sizeof path, "%s\\%s", data_folder, table_name);
            H table = 0;
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
                connection, (U8 *)path, NULL, table_type, char_type, 1, 1, options_of(word(&rest)),
                &table);
            result(rc);
            if (!rc) {
                orders[count] = 0;
                tables[count++] = table;
                current = count;
                printf(", \"table\": %d", count);
            }
        } else if (!strcmp(cmd, "constrain") || !strcmp(cmd, "readall") ||
                   !strcmp(cmd, "cursorreadonly")) {
            constrain = cmd[0] == 'c' && cmd[1] == 'o' ? atoi(rest) : constrain;
            read_columns = cmd[0] == 'r' ? atoi(rest) : read_columns;
            cursor_read_only = cmd[0] == 'c' && cmd[1] == 'u' ? atoi(rest) : cursor_read_only;
            result(0);
        } else if (!strcmp(cmd, "cursor") || !strcmp(cmd, "cursor2")) {
            H cstmt = 0, cursor = 0;
            unescape(rest);
            rc = ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(
                cmd[6] == '2' ? connection2 : connection, &cstmt);
            if (!rc) rc = ((U32(WINAPI *)(H, U16))F("AdsStmtSetTableType"))(cstmt, table_type);
            if (!rc) rc = ((U32(WINAPI *)(H, U16))F("AdsStmtSetTableCharType"))(cstmt, char_type);
            if (!rc && constrain >= 0)
                rc = ((U32(WINAPI *)(H, U16))F("AdsStmtConstrainUpdates"))(cstmt, (U16)constrain);
            if (!rc && cursor_read_only >= 0)
                rc = ((U32(WINAPI *)(H, U16))F("AdsStmtSetTableReadOnly"))(cstmt,
                                                                           (U16)cursor_read_only);
            if (!rc && read_columns >= 0)
                rc = ((U32(WINAPI *)(H, U16))F("AdsStmtReadAllColumns"))(cstmt, (U16)read_columns);
            if (!rc)
                rc = ((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(cstmt, (U8 *)rest,
                                                                             &cursor);
            result(rc);
            if (!rc && cursor) {
                orders[count] = 0;
                cursor_stmts[count] = cstmt;
                tables[count++] = cursor;
                current = count;
                printf(", \"table\": %d", count);
            }
        } else if (!strcmp(cmd, "stmtset")) {
            char *what = word(&rest);
            const char *fn = !strcmp(what, "constrain")  ? "AdsStmtConstrainUpdates"
                             : !strcmp(what, "readall") ? "AdsStmtReadAllColumns"
                                                        : "AdsStmtSetTableReadOnly";
            result(((U32(WINAPI *)(H, U16))F(fn))(cursor_stmts[current - 1], (U16)atoi(rest)));
        } else if (!strcmp(cmd, "closecursor")) {
            result(((U32(WINAPI *)(H))F("AdsCloseSQLStatement"))(cursor_stmts[current - 1]));
        } else if (!strcmp(cmd, "open2")) {
            /* A table on a second connection, as another user sees it. */
            char path[600];
            char *table_name = word(&rest);
            // :<name> is a dictionary's table, given by its name.
            if (table_name[0] == ':')
                snprintf(path, sizeof path, "%s", table_name + 1);
            else
                snprintf(path, sizeof path, "%s\\%s", data_folder, table_name);
            H table = 0;
            rc = connection2 ? 0 : connect_to(argv[2], &connection2);
            if (!rc)
                rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F(
                    "AdsOpenTable"))(connection2, (U8 *)path, NULL, table_type, char_type, 1, 1,
                                     options_of(word(&rest)), &table);
            result(rc);
            if (!rc) {
                orders[count] = 0;
                tables[count++] = table;
                current = count;
                printf(", \"table\": %d", count);
            }
        } else if (!strcmp(cmd, "begin") || !strcmp(cmd, "commit") || !strcmp(cmd, "rollback")) {
            const char *fn = cmd[0] == 'b'   ? "AdsBeginTransaction"
                             : cmd[0] == 'c' ? "AdsCommitTransaction"
                                             : "AdsRollbackTransaction";
            result(((U32(WINAPI *)(H))F(fn))(connection));
        } else if (!strcmp(cmd, "savepoint") || !strcmp(cmd, "rollbackto")) {
            const char *fn = cmd[0] == 's' ? "AdsCreateSavepoint" : "AdsRollbackTransaction80";
            result(((U32(WINAPI *)(H, U8 *, U32))F(fn))(connection, (U8 *)rest, 0));
        } else if (!strcmp(cmd, "txcount")) {
            U32 n = 0;
            result(((U32(WINAPI *)(H, U32 *))F("AdsGetTransactionCount"))(connection, &n));
            printf(", \"count\": %lu", n);
        } else if (!strcmp(cmd, "snapshot")) {
            result(snapshot(rest));
        } else if (!strcmp(cmd, "use")) {
            on_connection = !strcmp(rest, "conn");
            if (!on_connection) current = atoi(rest);
            result(0);
        } else if (!strcmp(cmd, "openindex")) {
            char path[600];
            snprintf(path, sizeof path, "%s\\%s", data_folder, rest);
            H orders[256];
            U16 n = 256;
            result(((U32(WINAPI *)(H, U8 *, H *, U16 *))F("AdsOpenIndex"))(t, (U8 *)path, orders,
                                                                           &n));
        } else if (!strcmp(cmd, "writeall")) {
            result(((U32(WINAPI *)(void))F("AdsWriteAllRecords"))());
        } else if (!strcmp(cmd, "goto") || !strcmp(cmd, "lock") || !strcmp(cmd, "unlock")) {
            const char *fn = cmd[0] == 'g' ? "AdsGotoRecord" : cmd[0] == 'l' ? "AdsLockRecord"
                                                                              : "AdsUnlockRecord";
            result(((U32(WINAPI *)(H, U32))F(fn))(t, strtoul(rest, NULL, 10)));
        } else if (!strcmp(cmd, "otop") || !strcmp(cmd, "obottom")) {
            H moving = orders[current - 1] ? orders[current - 1] : t;
            result(((Op)F(cmd[1] == 't' ? "AdsGotoTop" : "AdsGotoBottom"))(moving));
        } else if (!strcmp(cmd, "oskip")) {
            H moving = orders[current - 1] ? orders[current - 1] : t;
            result(((U32(WINAPI *)(H, S32))F("AdsSkip"))(moving, strtol(rest, NULL, 10)));
        } else if (!strcmp(cmd, "seek")) {
            H moving = orders[current - 1] ? orders[current - 1] : t;
            U16 soft = (U16)atoi(word(&rest)), found = 77;
            unescape(rest);
            result(((U32(WINAPI *)(H, U8 *, U16, U16, U16, U16 *))F("AdsSeek"))(
                moving, (U8 *)rest, (U16)strlen(rest), 2, soft, &found));
            printf(", \"found\": %u", found);
        } else if (!strcmp(cmd, "pos")) {
            U32 recno = 0;
            U16 bof = 77, eof = 77, found = 77;
            rc = ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordNum"))(t, 2, &recno);
            U32 r2 = ((U32(WINAPI *)(H, U16 *))F("AdsAtBOF"))(t, &bof);
            U32 r3 = ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(t, &eof);
            U32 r4 = ((U32(WINAPI *)(H, U16 *))F("AdsIsFound"))(t, &found);
            result(0);
            printf(", \"pos\": [%lu, %u, %u, %u], \"rcs\": [%lu, %lu, %lu, %lu]", recno, bof,
                   eof, found, rc, r2, r3, r4);
        } else if (!strcmp(cmd, "scope") || !strcmp(cmd, "clearscope") ||
                   !strcmp(cmd, "getscope")) {
            H order = orders[current - 1];
            U16 option = (U16)atoi(word(&rest));
            if (cmd[0] == 's') {
                unescape(rest);
                result(((U32(WINAPI *)(H, U16, U8 *, U16, U16))F("AdsSetScope"))(
                    order, option, (U8 *)rest, (U16)strlen(rest), 2));
            } else if (cmd[0] == 'c') {
                result(((U32(WINAPI *)(H, U16))F("AdsClearScope"))(order, option));
            } else {
                U8 buf[300];
                U16 len = sizeof buf;
                rc = ((U32(WINAPI *)(H, U16, U8 *, U16 *))F("AdsGetScope"))(order, option, buf,
                                                                            &len);
                result(rc);
                hex("scope", buf, rc ? 0 : len);
            }
        } else if (!strcmp(cmd, "relation") || !strcmp(cmd, "scopedrelation")) {
            char *which = word(&rest);
            H child = 0;
            if (*which == 't') {
                child = tables[atoi(which + 1) - 1];
            } else if (atoi(which) > 0) {
                int n = atoi(which);
                child = orders[n - 1] ? orders[n - 1] : tables[n - 1];
            }
            unescape(rest);
            U8 *expr = strcmp(rest, "-") ? (U8 *)rest : NULL;
            result(((U32(WINAPI *)(H, H, U8 *))F(cmd[0] == 'r' ? "AdsSetRelation"
                                                                : "AdsSetScopedRelation"))(
                t, child, expr));
        } else if (!strcmp(cmd, "skip")) {
            result(((U32(WINAPI *)(H, S32))F("AdsSkip"))(t, strtol(rest, NULL, 10)));
        } else if (!strcmp(cmd, "islocked")) {
            U16 locked = 0;
            result(((U32(WINAPI *)(H, U32, U16 *))F("AdsIsRecordLocked"))(
                t, strtoul(rest, NULL, 10), &locked));
            printf(", \"locked\": %u", locked);
        } else if (!strcmp(cmd, "tablelocked")) {
            U16 locked = 0;
            result(((U32(WINAPI *)(H, U16 *))F("AdsIsTableLocked"))(t, &locked));
            printf(", \"locked\": %u", locked);
        } else if (!strcmp(cmd, "locks")) {
            U32 locks[256];
            U16 n = 256;
            rc = ((U32(WINAPI *)(H, U32 *, U16 *))F("AdsGetAllLocks"))(t, locks, &n);
            result(rc);
            if (rc) n = 0;
            printf(", \"locks\": [");
            for (U16 k = 0; k < n && k < 256; k++) printf("%s%lu", k ? ", " : "", locks[k]);
            printf("]");
        } else if (!strcmp(cmd, "set")) {
            char *field = word(&rest);
            char *kind = word(&rest);
            result(set(t, field_arg(field), kind, rest));
        } else if (!strcmp(cmd, "get")) {
            U8 *field = field_arg(word(&rest));
            U16 null = 0;
            U32 nrc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsIsNull"))(t, field, &null);
            static U8 buf[70000];
            U32 len = sizeof buf;
            result(((U32(WINAPI *)(H, U8 *, U8 *, U32 *, U16))F("AdsGetField"))(t, field, buf,
                                                                                &len, 0));
            printf(", \"null\": %u", nrc ? 99 : null);
            hex("value", buf, len < sizeof buf ? len : 0);
        } else if (!strcmp(cmd, "record")) {
            static U8 buf[70000];
            U32 len = sizeof buf;
            rc = ((U32(WINAPI *)(H, U8 *, U32 *))F("AdsGetRecord"))(t, buf, &len);
            result(rc);
            hex("record", buf, rc ? 0 : len);
        } else if (!strcmp(cmd, "raw")) {
            U8 *field = field_arg(word(&rest));
            static U8 buf[70000];
            U32 len = sizeof buf, offset = 0, next = 0;
            U16 number = 0, fields = 0;
            rc = ((U32(WINAPI *)(H, U8 *, U32 *))F("AdsGetRecord"))(t, buf, &len);
            if (!rc) rc = ((U32(WINAPI *)(H, U8 *, U32 *))F("AdsGetFieldOffset"))(t, field, &offset);
            if (!rc) rc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsGetFieldNum"))(t, field, &number);
            ((U32(WINAPI *)(H, U16 *))F("AdsGetNumFields"))(t, &fields);
            next = len;
            if (!rc && number < fields)
                ((U32(WINAPI *)(H, U8 *, U32 *))F("AdsGetFieldOffset"))(t, FIELD(number + 1), &next);
            result(rc);
            hex("raw", buf + offset, rc || next < offset ? 0 : next - offset);
        } else if (!strcmp(cmd, "state")) {
            U32 recno = 0, records = 0, autoinc = 0;
            U16 deleted = 0;
            U32 r1 = ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordNum"))(t, 2, &recno);
            U32 r2 = ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(t, 2, &records);
            U32 r3 = ((U32(WINAPI *)(H, U16 *))F("AdsIsRecordDeleted"))(t, &deleted);
            U32 r4 = ((U32(WINAPI *)(H, U32 *))F("AdsGetLastAutoinc"))(t, &autoinc);
            result(0);
            printf(", \"recno\": [%lu, %lu], \"count\": [%lu, %lu], \"deleted\": [%lu, %u], "
                   "\"autoinc\": [%lu, %lu]",
                   r1, recno, r2, records, r3, deleted, r4, autoinc);
        } else if (!strcmp(cmd, "enableenc")) {
            unescape(rest);
            result(((U32(WINAPI *)(H, U8 *))F("AdsEnableEncryption"))(t, (U8 *)rest));
        } else if (!strcmp(cmd, "encstate")) {
            U16 enabled = 77, record = 77, table = 77;
            U32 r1 = ((U32(WINAPI *)(H, U16 *))F("AdsIsEncryptionEnabled"))(t, &enabled);
            U32 r2 = ((U32(WINAPI *)(H, U16 *))F("AdsIsRecordEncrypted"))(t, &record);
            U32 r3 = ((U32(WINAPI *)(H, U16 *))F("AdsIsTableEncrypted"))(t, &table);
            result(0);
            printf(", \"enabled\": [%lu, %u], \"record\": [%lu, %u], \"table\": [%lu, %u]", r1,
                   enabled, r2, record, r3, table);
        } else if (!strcmp(cmd, "info")) {
            U32 length = 77;
            U16 fields = 77, block = 77, encrypted = 77, type = 77;
            U32 r1 = ((U32(WINAPI *)(H, U32 *))F("AdsGetRecordLength"))(t, &length);
            U32 r2 = ((U32(WINAPI *)(H, U16 *))F("AdsGetNumFields"))(t, &fields);
            U32 r3 = ((U32(WINAPI *)(H, U16 *))F("AdsGetMemoBlockSize"))(t, &block);
            U32 r4 = ((U32(WINAPI *)(H, U16 *))F("AdsIsTableEncrypted"))(t, &encrypted);
            U32 r5 = ((U32(WINAPI *)(H, U16 *))F("AdsGetTableType"))(t, &type);
            result(0);
            printf(", \"length\": [%lu, %lu], \"fields\": [%lu, %u], \"block\": [%lu, %u], "
                   "\"encrypted\": [%lu, %u], \"type\": [%lu, %u]",
                   r1, length, r2, fields, r3, block, r4, encrypted, r5, type);
        } else if (!strcmp(cmd, "createtable")) {
            char path[600];
            snprintf(path, sizeof path, "%s\\%s", data_folder, word(&rest));
            U16 memo = (U16)strtoul(word(&rest), NULL, 10);
            H table = 0;
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, U32, H *))F(
                "AdsCreateTable71"))(connection, (U8 *)path, NULL, table_type, char_type, 1, 1,
                                     memo, (U8 *)rest, 0, &table);
            result(rc);
            if (!rc) {
                orders[count] = 0;
                tables[count++] = table;
                current = count;
                printf(", \"table\": %d", count);
            }
        } else if (!strcmp(cmd, "createindex")) {
            char *file = word(&rest);
            char *tag = word(&rest);
            U32 options = strtoul(word(&rest), NULL, 0);
            U32 page = strtoul(word(&rest), NULL, 0);
            char *cond = strstr(rest, " FOR ");
            if (cond) {
                *cond = 0;
                cond += 5;
            }
            char path[600];
            snprintf(path, sizeof path, "%s\\%s", data_folder, file);
            H order = 0;
            result(((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, U32, H *))F(
                "AdsCreateIndex61"))(t, strcmp(file, "-") ? (U8 *)path : NULL, (U8 *)tag,
                                     (U8 *)rest, (U8 *)(cond ? cond : ""), (U8 *)"", options,
                                     page, &order));
        } else if (!strcmp(cmd, "deleteindex")) {
            H order = 0;
            rc = ((U32(WINAPI *)(H, U8 *, H *))F("AdsGetIndexHandle"))(t, (U8 *)rest, &order);
            if (!rc) rc = ((U32(WINAPI *)(H))F("AdsDeleteIndex"))(order);
            result(rc);
        } else if (!strcmp(cmd, "aof")) {
            U16 options = (U16)strtoul(word(&rest), NULL, 0);
            result(((U32(WINAPI *)(H, U8 *, U16))F("AdsSetAOF"))(t, (U8 *)rest, options));
        } else if (!strcmp(cmd, "filter")) {
            result(((U32(WINAPI *)(H, U8 *))F("AdsSetFilter"))(t, (U8 *)rest));
        } else if (!strcmp(cmd, "aoflevel")) {
            U16 level = 0, len = 4096;
            static U8 buf[4096];
            rc = ((U32(WINAPI *)(H, U16 *, U8 *, U16 *))F("AdsGetAOFOptLevel"))(t, &level, buf,
                                                                                &len);
            result(rc);
            printf(", \"level\": %u, \"nonopt\": ", level);
            json_bytes(buf, rc ? 0 : len);
        } else if (!strcmp(cmd, "evalaof")) {
            U16 level = 0;
            result(((U32(WINAPI *)(H, U8 *, U16 *))F("AdsEvalAOF"))(t, (U8 *)rest, &level));
            printf(", \"level\": %u", level);
        } else if (!strcmp(cmd, "inaof")) {
            U16 flag = 0;
            result(((U32(WINAPI *)(H, U32, U16 *))F("AdsIsRecordInAOF"))(
                t, strtoul(rest, NULL, 10), &flag));
            printf(", \"flag\": %u", flag);
        } else if (!strcmp(cmd, "custom")) {
            U16 option = (U16)strtoul(word(&rest), NULL, 10);
            U32 records[256], n = 0;
            for (char *w = word(&rest); *w && n < 256; w = word(&rest))
                records[n++] = strtoul(w, NULL, 10);
            result(((U32(WINAPI *)(H, U32, U32 *, U16))F("AdsCustomizeAOF"))(t, n, records,
                                                                             option));
        } else if (!strcmp(cmd, "order")) {
            H order = 0;
            rc = strcmp(rest, "-")
                     ? ((U32(WINAPI *)(H, U8 *, H *))F("AdsGetIndexHandle"))(t, (U8 *)rest, &order)
                     : 0;
            if (!rc) orders[current - 1] = order;
            result(rc);
        } else if (!strcmp(cmd, "rows") || !strcmp(cmd, "count")) {
            H moving = orders[current - 1] ? orders[current - 1] : t;
            int walk = cmd[0] == 'r';
            rc = walk ? ((Op)F("AdsGotoTop"))(moving) : 0;
            result(rc);
            if (walk) printf(", \"rows\": [");
            for (int k = 0; walk && !rc && k < 100000; k++) {
                U16 eof = 1;
                U32 recno = 0;
                ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(t, &eof);
                if (eof) break;
                ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordNum"))(t, 2, &recno);
                printf("%s%lu", k ? ", " : "", recno);
                if (((U32(WINAPI *)(H, S32))F("AdsSkip"))(moving, 1)) break;
            }
            U32 n = 0;
            rc = ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(t, 1, &n);
            printf("%s\"count\": [%lu, %lu]", walk ? "], " : ", ", rc, n);
            if (orders[current - 1]) {
                rc = ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetKeyCount"))(orders[current - 1], 1,
                                                                        &n);
                printf(", \"keys\": [%lu, %lu]", rc, n);
            }
        } else if (!strcmp(cmd, "column")) {
            /* column <field>: AdsGetField of the field in every record from
             * the top to EOF, as text ("values") */
            H moving = orders[current - 1] ? orders[current - 1] : t;
            U8 *field = field_arg(word(&rest));
            rc = ((Op)F("AdsGotoTop"))(moving);
            result(rc);
            printf(", \"values\": [");
            for (int k = 0; !rc && k < 100000; k++) {
                U16 eof = 1;
                ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(t, &eof);
                if (eof) break;
                static U8 buf[70000];
                U32 len = sizeof buf;
                U32 grc = ((U32(WINAPI *)(H, U8 *, U8 *, U32 *, U16))F("AdsGetField"))(t, field, buf, &len, 0);
                printf("%s", k ? ", " : "");
                if (grc) printf("null");
                else json_bytes(buf, len);
                if (((U32(WINAPI *)(H, S32))F("AdsSkip"))(moving, 1)) break;
            }
            printf("]");
        } else if (!strcmp(cmd, "createfts")) {
            char *file = word(&rest), *tag = word(&rest), *field = word(&rest);
            U32 page = strtoul(word(&rest), NULL, 0), min = strtoul(word(&rest), NULL, 0);
            U32 max = strtoul(word(&rest), NULL, 0), options = strtoul(word(&rest), NULL, 0);
            U16 use[4];
            char *list[4];
            for (int i = 0; i < 4; i++) {
                use[i] = (U16)atoi(word(&rest));
                list[i] = word(&rest);
                if (!strcmp(list[i], "-")) list[i] = NULL;
                else unescape(list[i]);
            }
            char path[600];
            snprintf(path, sizeof path, "%s\\%s", data_folder, file);
            result(((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U32, U32, U32, U16, void *, U16, void *, U16,
                                   void *, U16, void *, U8 *, U8 *, U32))F("AdsCreateFTSIndex"))(
                t, strcmp(file, "-") ? (U8 *)path : NULL, (U8 *)tag, (U8 *)field, page, min, max,
                use[0], list[0], use[1], list[1], use[2], list[2], use[3], list[3], NULL, NULL,
                options));
        } else if (!strcmp(cmd, "reindexfts")) {
            result(((U32(WINAPI *)(H, U32))F("AdsReindexFTS"))(t, strtoul(rest, NULL, 0)));
        } else if (!strcmp(cmd, "reindex61")) {
            result(((U32(WINAPI *)(H, U32))F("AdsReindex61"))(t, strtoul(rest, NULL, 0)));
        } else if (!strcmp(cmd, "pack120")) {
            U32 size = strtoul(word(&rest), NULL, 0);
            result(((U32(WINAPI *)(H, U32, U32))F("AdsPackTable120"))(t, size,
                                                                      strtoul(rest, NULL, 0)));
        } else if (!strcmp(cmd, "conn")) {
            /* conn <function> [n]: a function of the connection, with a
             * flag if given */
            char *fn = word(&rest);
            result(*rest ? ((U32(WINAPI *)(H, U16))F(fn))(connection, (U16)atoi(rest))
                         : ((U32(WINAPI *)(H))F(fn))(connection));
        } else if (!strcmp(cmd, "setbin")) {
            static U8 bytes[70000];
            char *field = word(&rest);
            U16 type = (U16)strtoul(word(&rest), NULL, 0);
            U32 total = strtoul(word(&rest), NULL, 0);
            U32 offset = strtoul(word(&rest), NULL, 0);
            size_t n = from_hex(rest, bytes);
            result(((U32(WINAPI *)(H, U8 *, U16, U32, U32, U8 *, U32))F("AdsSetBinary"))(
                t, field_arg(field), type, total, offset, bytes, (U32)n));
        } else if (!strcmp(cmd, "getopt")) {
            U8 *field = field_arg(word(&rest));
            U16 option = (U16)strtoul(word(&rest), NULL, 0);
            U32 len = *rest ? strtoul(rest, NULL, 0) : 200;
            static U8 buf[70000];
            memset(buf, '#', sizeof buf);
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U32 *, U16))F("AdsGetField"))(t, field, buf, &len,
                                                                              option);
            result(rc);
            printf(", \"len\": %lu", len);
            hex("value", buf, len < 600 ? len : 0);
        } else if (!strcmp(cmd, "handletype")) {
            U16 kind = 77;
            H obj = t;
            if (*rest) {
                obj = 0;
                U32 orc = ((U32(WINAPI *)(H, U8 *, H *))F("AdsGetIndexHandle"))(t, (U8 *)rest, &obj);
                printf("\"order\": %lu, ", orc);
            }
            result(((U32(WINAPI *)(H, U16 *))F("AdsGetHandleType"))(obj, &kind));
            printf(", \"type\": %u", kind);
        } else if (!strcmp(cmd, "lasterror")) {
            /* AdsGetLastError as it stands, after a call that succeeded */
            U32 code = 0;
            U8 text[1200];
            U16 len = sizeof text;
            ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
            result(0);
            printf(", \"code\": %lu, \"text\": ", code);
            /* With no error the buffer and its length are left as they were. */
            json_bytes(text, code ? len : 0);
        } else if (!strcmp(cmd, "links")) {
            U16 n = 77;
            result(((U32(WINAPI *)(H, U16 *))F("AdsGetNumActiveLinks"))(connection, &n));
            printf(", \"links\": %u", n);
        } else if (!strcmp(cmd, "transfree")) {
            U16 flag = 77;
            result(((U32(WINAPI *)(H, U16))F("AdsSetTableTransactionFree"))(t, (U16)atoi(rest)));
            U32 r2 = ((U32(WINAPI *)(H, U16 *))F("AdsIsTableTransactionFree"))(t, &flag);
            printf(", \"free\": [%lu, %u]", r2, flag);
        } else if (!strcmp(cmd, "keycolumn")) {
            U8 buf[512];
            U16 len = (U16)(*rest ? atoi(rest) : sizeof buf);
            rc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsGetKeyColumn"))(t, buf, &len);
            result(rc);
            printf(", \"len\": %u, \"value\": ", len);
            json_bytes(buf, rc ? 0 : len);
        } else if (!strcmp(cmd, "lookup")) {
            /* lookup <tag> <type> <key>: AdsLookupKey with the type's
             * number (1 raw, hex; 2 string; 4 double) */
            H order = 0;
            char *tag = word(&rest);
            U16 type = (U16)atoi(word(&rest)), found = 77, n = 0;
            static U8 key[1024];
            double d = 0;
            if (type == 1) n = (U16)from_hex(rest, key);
            else if (type == 4) d = strtod(rest, NULL), memcpy(key, &d, 8), n = 8;
            else unescape(rest), n = (U16)strlen(rest), memcpy(key, rest, n);
            rc = ((U32(WINAPI *)(H, U8 *, H *))F("AdsGetIndexHandle"))(t, (U8 *)tag, &order);
            if (!rc)
                rc = ((U32(WINAPI *)(H, U8 *, U16, U16, U16 *))F("AdsLookupKey"))(order, key, n,
                                                                                  type, &found);
            result(rc);
            printf(", \"found\": %u", found);
        } else if (!strcmp(cmd, "keyfromrecord")) {
            /* keyfromrecord <tag> [hex record]: AdsBuildKeyFromRecord on the
             * current record (AdsGetRecord) or the record given */
            H order = 0;
            static U8 record[65536], key[1024];
            U32 len = sizeof record;
            U16 klen = sizeof key;
            char *tag = word(&rest);
            if (*rest) len = (U32)from_hex(rest, record);
            else ((U32(WINAPI *)(H, U8 *, U32 *))F("AdsGetRecord"))(t, record, &len);
            rc = ((U32(WINAPI *)(H, U8 *, H *))F("AdsGetIndexHandle"))(t, (U8 *)tag, &order);
            if (!rc)
                rc = ((U32(WINAPI *)(H, U8 *, U32, U8 *, U16 *))F("AdsBuildKeyFromRecord"))(
                    order, record, len, key, &klen);
            result(rc);
            hex("key", key, rc ? 0 : klen);
        } else if (!strcmp(cmd, "setrecord")) {
            static U8 record[65536];
            U32 len = (U32)from_hex(word(&rest), record);
            if (*rest) len = strtoul(rest, NULL, 0);
            result(((U32(WINAPI *)(H, U8 *, U32))F("AdsSetRecord"))(t, record, len));
        } else if (!strcmp(cmd, "fieldraw")) {
            static U8 bytes[65536];
            char *field = word(&rest);
            U32 len = (U32)from_hex(rest, bytes);
            result(((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetFieldRaw"))(t, field_arg(field),
                                                                            bytes, len));
        } else if (!strcmp(cmd, "vfpnull")) {
            /* vfpnull <field> <options>: AdsAccessVfpSystemField on the
             * current record ("flag") */
            static U8 record[65536];
            U32 len = sizeof record;
            U16 flag = 77;
            char *field = word(&rest);
            ((U32(WINAPI *)(H, U8 *, U32 *))F("AdsGetRecord"))(t, record, &len);
            result(((U32(WINAPI *)(H, U8 *, U8 *, U32, U16 *))F("AdsAccessVfpSystemField"))(
                t, field_arg(field), record, strtoul(rest, NULL, 0), &flag));
            printf(", \"flag\": %u", flag);
        } else if (!strcmp(cmd, "rawkey")) {
            /* rawkey <tag> <options> [<field>=<value>;...]: AdsInitRawKey,
             * AdsSetField on the order for each field (an empty value is
             * AdsSetEmpty), AdsBuildRawKey100 ("key") */
            H order = 0;
            static U8 key[1024];
            U16 klen = sizeof key;
            char *tag = word(&rest);
            U32 options = strtoul(word(&rest), NULL, 0);
            rc = ((U32(WINAPI *)(H, U8 *, H *))F("AdsGetIndexHandle"))(t, (U8 *)tag, &order);
            if (!rc) rc = ((U32(WINAPI *)(H))F("AdsInitRawKey"))(order);
            for (char *item = strtok(rest, ";"); !rc && item; item = strtok(NULL, ";")) {
                char *value = strchr(item, '=');
                if (!value) continue;
                *value++ = 0;
                unescape(value);
                rc = *value ? ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetField"))(
                                  order, field_arg(item), (U8 *)value, strlen(value))
                            : ((U32(WINAPI *)(H, U8 *))F("AdsSetEmpty"))(order, field_arg(item));
            }
            if (!rc)
                rc = ((U32(WINAPI *)(H, U8 *, U16 *, U32))F("AdsBuildRawKey100"))(order, key, &klen,
                                                                                  options);
            result(rc);
            hex("key", key, rc ? 0 : klen);
        } else if (!strcmp(cmd, "stepkey")) {
            /* stepkey <tag> <direction> <hex key>: AdsStepIndexKey ("key") */
            H order = 0;
            static U8 key[1024];
            char *tag = word(&rest);
            S16 direction = (S16)atoi(word(&rest));
            U16 n = (U16)from_hex(rest, key);
            rc = ((U32(WINAPI *)(H, U8 *, H *))F("AdsGetIndexHandle"))(t, (U8 *)tag, &order);
            if (!rc)
                rc = ((U32(WINAPI *)(H, U8 *, U16, S16))F("AdsStepIndexKey"))(order, key, n,
                                                                              direction);
            result(rc);
            hex("key", key, n);
        } else if (!strcmp(cmd, "locate")) {
            U16 forward = (U16)atoi(word(&rest)), found = 77;
            result(((U32(WINAPI *)(H, U8 *, U16, U16 *))F("AdsLocate"))(t, (U8 *)rest, forward,
                                                                       &found));
            printf(", \"found\": %u", found);
        } else if (!strcmp(cmd, "continue")) {
            U16 found = 77;
            result(((U32(WINAPI *)(H, U16 *))F("AdsContinue"))(t, &found));
            printf(", \"found\": %u", found);
        } else if (!strcmp(cmd, "filetobinary")) {
            char *field = word(&rest);
            U16 type = (U16)atoi(word(&rest));
            char path[600];
            snprintf(path, sizeof path, "%s\\%s", data_folder, rest);
            result(((U32(WINAPI *)(H, U8 *, U16, U8 *))F("AdsFileToBinary"))(t, field_arg(field),
                                                                             type, (U8 *)path));
        } else if (!strcmp(cmd, "cpstring")) {
            char *field = word(&rest);
            U32 page = strtoul(word(&rest), NULL, 0);
            size_t n = unescape(rest);
            result(((U32(WINAPI *)(H, U32, U8 *, U8 *, U32))F("AdsSetStringFromCodePage"))(
                t, page, field_arg(field), (U8 *)rest, (U32)n));
        } else if (!strcmp(cmd, "addcustom") || !strcmp(cmd, "delcustom")) {
            H order = 0;
            rc = ((U32(WINAPI *)(H, U8 *, H *))F("AdsGetIndexHandle"))(t, (U8 *)rest, &order);
            if (!rc)
                rc = ((U32(WINAPI *)(H))F(cmd[0] == 'a' ? "AdsAddCustomKey" : "AdsDeleteCustomKey"))(
                    order);
            result(rc);
        } else if (!strcmp(cmd, "copytable") || !strcmp(cmd, "copystructure")) {
            H from = orders[current - 1] ? orders[current - 1] : t;
            char path[600];
            if (cmd[4] == 't') {
                U16 filter = (U16)atoi(word(&rest));
                snprintf(path, sizeof path, "%s\\%s", data_folder, rest);
                result(((U32(WINAPI *)(H, U16, U8 *))F("AdsCopyTable"))(from, filter, (U8 *)path));
            } else {
                snprintf(path, sizeof path, "%s\\%s", data_folder, rest);
                result(((U32(WINAPI *)(H, U8 *))F("AdsCopyTableStructure"))(t, (U8 *)path));
            }
        } else if (!strcmp(cmd, "convert")) {
            H from = orders[current - 1] ? orders[current - 1] : t;
            U16 filter = (U16)atoi(word(&rest));
            U16 type = type_of(word(&rest));
            char path[600];
            snprintf(path, sizeof path, "%s\\%s", data_folder, rest);
            result(((U32(WINAPI *)(H, U16, U8 *, U16))F("AdsConvertTable"))(from, filter,
                                                                           (U8 *)path, type));
        } else if (!strcmp(cmd, "copycontents")) {
            H from = orders[current - 1] ? orders[current - 1] : t;
            int to = atoi(word(&rest));
            U16 filter = (U16)atoi(rest);
            result(((U32(WINAPI *)(H, H, U16))F("AdsCopyTableContents"))(from, tables[to - 1],
                                                                        filter));
        } else if (!strcmp(cmd, "restructure")) {
            char path[600];
            char *table_name = word(&rest);
            // A name with a colon in front (:t) is given as it is, a
            // dictionary's table.
            if (table_name[0] == ':')
                snprintf(path, sizeof path, "%s", table_name + 1);
            else
                snprintf(path, sizeof path, "%s\\%s", data_folder, table_name);
            U32 block = strtoul(word(&rest), NULL, 0);
            char *add = rest, *del = strchr(add, '|'), *change = NULL;
            if (del) *del++ = 0, change = strchr(del, '|');
            if (change) *change++ = 0;
            result(((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U8 *, U8 *, U8 *, U8 *, U32,
                                   U32))F("AdsRestructureTable120"))(
                connection, (U8 *)path, NULL, table_type, char_type, 1, 1, (U8 *)add,
                (U8 *)(del ? del : ""), (U8 *)(change ? change : ""), (U8 *)"", block, 0));
        } else {
            fprintf(stderr, "unknown command %s\n", cmd);
            return 2;
        }
        printf("}");
        fflush(stdout);
    }
    printf("\n]\n");
    return 0;
}
