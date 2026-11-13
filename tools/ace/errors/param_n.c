/* Records which `Parameter N` the original engine adds to the error text of
 * calls given a NULL pointer, or an option it refuses, for
 * fixtures/errors/param_n.json.
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o param_n.exe param_n.c
 *     wine param_n.exe <ace32.dll> <folder with indexed.adt, indexed.adi and indexed_table.add as a Windows path> <cases file>
 *
 * (see param_n.sh). The cases file has one call a line, its name and its
 * arguments, each a word:
 *
 *     t i c s          the handle of a table (indexed.adt), of its order
 *                      BY_NAME, of the connection, of a statement with
 *                      SELECT * FROM indexed prepared
 *     q                the handle of a statement with SELECT * FROM indexed
 *                      WHERE name = :p prepared
 *     0 1 99 ...       a number
 *     str:id           a pointer to the text after the colon
 *     pbuf pbuf2       a pointer to a buffer of 400 bytes (another one)
 *     pl16 pl16b pl32  a pointer to a 16 or 32 bits number, 200 (another)
 *     p16 p32 p64      a pointer to a number, 0
 *     ph               a pointer to an array of 8 handles
 *     w                the handle of indexed.adt opened for writing
 *     a                the handle of all_types.adt (a field of each type) opened
 *                      for writing
 *     x y v            the handle of a DBF table of FoxPro (xcdx.dbf), Clipper
 *                      (yntx.dbf) or VFP (vvfp.dbf) type, with the fields n
 *                      Numeric(8,2), d Date, c Character(10), l Logical and a
 *                      record to set; opened for writing
 *     pbuft:text       as pbuf, with the text at its start (a ~ is a blank)
 *     pbufh:hex        as pbuf, with the bytes the hex digits say at its start
 *     n                a statement that has nothing prepared, made anew for
 *                      each call
 *     d                the connection to the dictionary indexed_table.add,
 *                      with no user
 *     da               a connection to it as ADSSYS
 *     k                the handle of a cursor, SELECT * FROM indexed
 *     pstr:text        as str:text, but a pointer that can be NULL
 *     pwstr:text       a pointer to the text as 16 bits characters, which can
 *                      be NULL (a ~ in a text is a space)
 *
 * A line that starts with = (the call's name) runs as it is only. One that
 * starts with + also records what the call left in its out-buffers: "len" (the
 * 16 bits length), "v32" (the 32 bits number) and "buf" (the first 32 bytes of the
 * buffer, in hex). One that starts with @ runs as it is only and records the
 * record of its first argument, a table, as the call left it ("rec": the bytes
 * AdsGetRecord gives, in hex).
 *
 * Each call runs as it is and then once for each of its pointers (those that
 * start with p, but not str:) replaced by NULL; the result of each is
 * {"call": <the line>, "null": <the number of the argument made NULL, 1 for
 * the first, 0 for none>, "rc": <rc>, "last": <AdsGetLastError's code>,
 * "text": <its text>}. Each call is made after a failure of another code
 * (AdsGetFieldType of a field that is not there) so that a call that leaves
 * the last error shows that. Calls are made on the table at its first
 * record. */
#include <windows.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;
static HMODULE ace;
#define F(name) ((void *)GetProcAddress(ace, name))

static H conn, dict, admin, table, writable, alltypes, order, stmt, params, cursor, fresh;
static int first = 1;
static int record, rawdump;
static H dbf[3]; /* x y v */

static H call(void *fn, int n, H *a) {
    switch (n) {
    case 0: return ((U32(WINAPI *)(void))fn)();
    case 1: return ((U32(WINAPI *)(H))fn)(a[0]);
    case 2: return ((U32(WINAPI *)(H, H))fn)(a[0], a[1]);
    case 3: return ((U32(WINAPI *)(H, H, H))fn)(a[0], a[1], a[2]);
    case 4: return ((U32(WINAPI *)(H, H, H, H))fn)(a[0], a[1], a[2], a[3]);
    case 5: return ((U32(WINAPI *)(H, H, H, H, H))fn)(a[0], a[1], a[2], a[3], a[4]);
    case 6: return ((U32(WINAPI *)(H, H, H, H, H, H))fn)(a[0], a[1], a[2], a[3], a[4], a[5]);
    default: return ((U32(WINAPI *)(H, H, H, H, H, H, H))fn)(a[0], a[1], a[2], a[3], a[4], a[5], a[6]);
    }
}

static void json(const U8 *s) {
    putchar('"');
    for (; *s; s++) {
        if (*s == '"' || *s == '\\') printf("\\%c", *s);
        else if (*s < 32 || *s >= 127) printf("\\u%04x", *s);
        else putchar(*s);
    }
    putchar('"');
}

static int is_pointer(const char *w) { return w[0] == 'p'; }

/* Runs the call with the pointer of argument `null` (1-based) NULL. */
static void run(const char *line, char words[][64], int nwords, int null) {
    U8 buf[400], buf2[400];
    U16 l16 = 200, l16b = 200, v16 = 0;
    U32 l32 = 200, v32 = 0;
    U32 v64[2] = {0, 0};
    H handles[8];
    char strings[8][64];
    unsigned short wstrings[8][64];
    H args[8];
    memset(buf, 0, sizeof buf);
    memset(buf2, 0, sizeof buf2);
    memset(handles, 0, sizeof handles);
    for (int i = 1; i < nwords; i++) {
        const char *w = words[i];
        H v;
        if (!strcmp(w, "t")) v = table;
        else if (!strcmp(w, "i")) v = order;
        else if (!strcmp(w, "c")) v = conn;
        else if (!strcmp(w, "d")) v = dict;
        else if (!strcmp(w, "da")) v = admin;
        else if (!strcmp(w, "w")) v = writable;
        else if (!strcmp(w, "a")) v = alltypes;
        else if (!strcmp(w, "x")) v = dbf[0];
        else if (!strcmp(w, "y")) v = dbf[1];
        else if (!strcmp(w, "v")) v = dbf[2];
        else if (!strcmp(w, "n")) {
            ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(conn, &fresh);
            v = fresh;
        } else if (!strcmp(w, "k")) v = cursor;
        else if (!strcmp(w, "s")) v = stmt;
        else if (!strcmp(w, "q")) v = params;
        else if (!strcmp(w, "pbuf")) v = (H)buf;
        else if (!strcmp(w, "pbuf2")) v = (H)buf2;
        else if (!strncmp(w, "pbuft:", 6)) {
            for (int k = 0; w[6 + k] && k < 390; k++) buf[k] = w[6 + k] == '~' ? ' ' : w[6 + k];
            v = (H)buf;
        } else if (!strncmp(w, "pbufh:", 6)) {
            for (int k = 0; w[6 + 2 * k] && w[7 + 2 * k] && k < 390; k++) {
                char two[3] = {w[6 + 2 * k], w[7 + 2 * k], 0};
                buf[k] = (U8)strtoul(two, NULL, 16);
            }
            v = (H)buf;
        }
        else if (!strcmp(w, "pl16b")) v = (H)&l16b;
        else if (!strcmp(w, "pl16")) v = (H)&l16;
        else if (!strcmp(w, "pl32")) v = (H)&l32;
        else if (!strcmp(w, "p16")) v = (H)&v16;
        else if (!strcmp(w, "p32")) v = (H)&v32;
        else if (!strcmp(w, "p64")) v = (H)v64;
        else if (!strcmp(w, "ph")) v = (H)handles;
        else if (!strncmp(w, "str:", 4) || !strncmp(w, "pstr:", 5)) {
            snprintf(strings[i], 64, "%s", w + (w[0] == 'p' ? 5 : 4));
            for (char *c = strings[i]; *c; c++) if (*c == '~') *c = ' ';
            v = (H)strings[i];
        } else if (!strncmp(w, "pwstr:", 6)) {
            int n = 0;
            for (const char *c = w + 6; *c && n < 63; c++) wstrings[i][n++] = *c == '~' ? ' ' : *c;
            wstrings[i][n] = 0;
            v = (H)wstrings[i];
        } else v = (H)strtoul(w, NULL, 10);
        if (i == null) v = 0;
        args[i - 1] = v;
    }
    ((U32(WINAPI *)(H))F("AdsGotoTop"))(table);
    ((U32(WINAPI *)(H))F("AdsGotoTop"))(writable);
    ((U32(WINAPI *)(H))F("AdsGotoTop"))(alltypes);
    for (int i = 0; i < 3; i++)
        if (dbf[i]) ((U32(WINAPI *)(H))F("AdsGotoTop"))(dbf[i]);
    if (cursor) ((U32(WINAPI *)(H))F("AdsGotoTop"))(cursor);
    {
        U16 type;
        ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsGetFieldType"))(table, (U8 *)"nosuch", &type);
    }
    void *fn = F(words[0]);
    if (!fn) {
        printf("%s{\"call\": \"%s\", \"missing\": 1}", first ? "" : ",\n", line);
        first = 0;
        return;
    }
    /* A double is passed by value, as two words that `args` has no room for. */
    U32 rc = !strcmp(words[0], "AdsSetDouble")
                 ? ((U32(WINAPI *)(H, H, double))fn)(args[0], args[1], atof(words[3]))
                 : (U32)call(fn, nwords - 1, args);
    U32 code = 0;
    U8 text[1200] = "";
    U16 len = sizeof text;
    ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
    printf("%s{\"call\": \"%s\", \"null\": %d, \"rc\": %lu, \"last\": %lu, \"text\": ", first ? "" : ",\n",
           line, null, rc, code);
    json(text);
    if (record) {
        printf(", \"len\": %u, \"v32\": %lu, \"buf\": \"", l16, v32);
        for (int i = 0; i < 32; i++) printf("%02x", buf[i]);
        printf("\"");
    }
    if (rawdump) {
        U8 rec[1024];
        U32 n = sizeof rec;
        U32 got = ((U32(WINAPI *)(H, U8 *, U32 *))F("AdsGetRecord"))(args[0], rec, &n);
        printf(", \"rec_rc\": %lu, \"rec\": \"", got);
        if (!got) for (U32 i = 0; i < n; i++) printf("%02x", rec[i]);
        printf("\"");
    }
    printf("}");
    /* A setter's change is not kept for the next call. */
    ((U32(WINAPI *)(H))F("AdsCancelUpdate"))(writable);
    ((U32(WINAPI *)(H))F("AdsCancelUpdate"))(alltypes);
    for (int i = 0; i < 3; i++)
        if (dbf[i]) ((U32(WINAPI *)(H))F("AdsCancelUpdate"))(dbf[i]);
    first = 0;
}

int main(int argc, char **argv) {
    if (argc < 4) return fprintf(stderr, "usage: param_n.exe <ace32.dll> <folder> <cases>\n"), 2;
    setvbuf(stdout, NULL, _IONBF, 0);
    ace = LoadLibraryA(argv[1]);
    if (!ace) return fprintf(stderr, "cannot load %s\n", argv[1]), 1;
    if (((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))((U8 *)argv[2], 1, NULL,
                                                                           NULL, 0, &conn))
        return fprintf(stderr, "connect\n"), 1;
    char path[600];
    snprintf(path, sizeof path, "%s\\indexed_table.add", argv[2]);
    if (((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))((U8 *)path, 1, NULL, NULL, 0,
                                                                            &dict))
        return fprintf(stderr, "connect to the dictionary\n"), 1;
    snprintf(path, sizeof path, "%s\\indexed_table.add", argv[2]);
    if (((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))((U8 *)path, 1, (U8 *)"ADSSYS", NULL,
                                                                            0, &admin))
        return fprintf(stderr, "connect to the dictionary as ADSSYS\n"), 1;
    snprintf(path, sizeof path, "%s\\indexed.adt", argv[2]);
    if (((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
            conn, (U8 *)path, NULL, 3, 1, 1, 1, 2, &table))
        return fprintf(stderr, "open\n"), 1;
    if (((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
            conn, (U8 *)path, (U8 *)"w", 3, 1, 1, 1, 0, &writable))
        return fprintf(stderr, "open for writing\n"), 1;
    snprintf(path, sizeof path, "%s\\all_types.adt", argv[2]);
    if (((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
            conn, (U8 *)path, (U8 *)"a", 3, 1, 1, 1, 0, &alltypes))
        return fprintf(stderr, "open all_types\n"), 1;
    {
        static const char *names[3] = {"xcdx.dbf", "yntx.dbf", "vvfp.dbf"};
        static const U16 types[3] = {2, 1, 4};
        for (int i = 0; i < 3; i++) {
            snprintf(path, sizeof path, "%s\\%s", argv[2], names[i]);
            if (((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, U8 *, U32, H *))F(
                    "AdsCreateTable71"))(conn, (U8 *)path, NULL, types[i], 1, 1, 1, 0,
                                         (U8 *)"n,Numeric,8,2;d,Date;c,Character,10;l,Logical", 0,
                                         &dbf[i]))
                return fprintf(stderr, "create %s\n", names[i]), 1;
            ((U32(WINAPI *)(H))F("AdsAppendRecord"))(dbf[i]);
            ((U32(WINAPI *)(H))F("AdsWriteRecord"))(dbf[i]);
        }
    }
    ((U32(WINAPI *)(H, U8 *, H *))F("AdsGetIndexHandle"))(table, (U8 *)"BY_NAME", &order);
    ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(conn, &stmt);
    ((U32(WINAPI *)(H, U8 *))F("AdsPrepareSQL"))(stmt, (U8 *)"SELECT * FROM indexed");
    ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(conn, &params);
    ((U32(WINAPI *)(H, U8 *))F("AdsPrepareSQL"))(params, (U8 *)"SELECT * FROM indexed WHERE name = :p");
    {
        H direct = 0;
        ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(conn, &direct);
        ((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(direct, (U8 *)"SELECT * FROM indexed", &cursor);
    }
    FILE *cases = fopen(argv[3], "r");
    char line[400];
    printf("[\n");
    while (fgets(line, sizeof line, cases)) {
        line[strcspn(line, "\r\n")] = 0;
        if (!*line || *line == '#') continue;
        char words[10][64];
        int n = 0;
        char copy[400];
        snprintf(copy, sizeof copy, "%s", line);
        for (char *w = strtok(copy, " "); w && n < 10; w = strtok(NULL, " ")) snprintf(words[n++], 64, "%s", w);
        int once = line[0] == '=';
        record = line[0] == '+';
        rawdump = line[0] == '@';
        once |= rawdump;
        if (once || record || rawdump) {
            memmove(line, line + 1, strlen(line));
            memmove(words[0], words[0] + 1, strlen(words[0]));
        }
        run(line, words, n, 0);
        for (int i = 1; i < n && !once; i++)
            if (is_pointer(words[i])) run(line, words, n, i);
    }
    printf("\n]\n");
    return 0;
}
