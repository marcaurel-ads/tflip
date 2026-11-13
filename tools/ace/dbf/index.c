/* Records what an Advantage Client Engine reports about the index files of
 * DBF tables, for fixtures/dbf/indexes.json.
 *
 *     index.exe <ace32.dll> <data folder> <type> <char type> <table> <index,...> ...  > indexes.json
 *
 * Built with i686-w64-mingw32-gcc and run under Wine on copies of the DBF
 * fixtures (see index.sh). Each table is opened as the table type and
 * character type given before it; the orders that open with it (a
 * structural .cdx) are walked, then its index files opened with
 * AdsOpenIndex.
 * For each order: its name, expression, condition, key length and type,
 * flags and key counts; the records and key numbers of a walk to the end
 * and back, with each key as AdsExtractKey gives it; and seeks: each of the
 * first 40 distinct keys as a string (softly and hard), its first character,
 * one past the last key, and for numeric and date orders numbers given as
 * strings and doubles. Each call is a JSON object:
 *
 *     {"table": "ntx/dbf_keys.dbf", "index": "i_char", "case": "...",
 *      "fn": "AdsSeek", "rc": 0, ...}
 *
 * Outputs start as a sentinel (0x5A bytes) and are recorded whatever the
 * return code. Text is recorded as hex. */
#include <windows.h>
#include <stdio.h>
#include <string.h>
#include <stdlib.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;

static HMODULE ace;
#define F(name) GetProcAddress(ace, name)
#define SENTINEL 0x5A

static int first = 1;
static const char *table_name = "";
static char index_name[64] = "";

static void begin(const char *c, const char *fn, U32 rc) {
    printf("%s\n{\"table\": \"%s\", \"index\": \"%s\", \"case\": \"%s\", \"fn\": \"%s\", \"rc\": %lu",
           first ? "[" : ",", table_name, index_name, c, fn, rc);
    first = 0;
}

static void hex(const char *key, const U8 *bytes, size_t n) {
    printf(", \"%s\": \"", key);
    for (size_t i = 0; i < n; i++) printf("%02x", bytes[i]);
    printf("\"");
}

typedef U32(WINAPI *TextOf)(H, U8 *, U16 *);
typedef U32(WINAPI *FlagOf)(H, U16 *);

static void text_of(H order, const char *fn) {
    U8 buf[600];
    memset(buf, SENTINEL, sizeof buf);
    U16 len = 500;
    U32 rc = ((TextOf)F(fn))(order, buf, &len);
    begin("info", fn, rc);
    printf(", \"len\": %u", len);
    hex("value", buf, rc ? 0 : len);
    printf("}");
}

static void flag_of(H order, const char *fn) {
    U16 v = 0x5A5A;
    U32 rc = ((FlagOf)F(fn))(order, &v);
    begin("info", fn, rc);
    printf(", \"value\": %u}", v);
}

static U32 record_num(H t) {
    U32 rec = 0;
    ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordNum"))(t, 2, &rec);
    return rec;
}

/* The current position: record, key number and key. */
static void position(H t, H order, const char *c, const char *fn, U32 rc) {
    U16 eof = 0, bof = 0, found = 0;
    ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(t, &eof);
    ((U32(WINAPI *)(H, U16 *))F("AdsAtBOF"))(t, &bof);
    ((U32(WINAPI *)(H, U16 *))F("AdsIsFound"))(t, &found);
    U32 key_num = 0x5A5A5A5A;
    U32 krc = ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetKeyNum"))(order, 2, &key_num);
    U8 key[300];
    U16 len = sizeof key;
    U32 xrc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsExtractKey"))(order, key, &len);
    begin(c, fn, rc);
    printf(", \"record\": %lu, \"eof\": %u, \"bof\": %u, \"found\": %u, \"key_num_rc\": %lu, "
           "\"key_num\": %lu, \"key_rc\": %lu",
           record_num(t), eof, bof, found, krc, key_num, xrc);
    hex("key", key, xrc ? 0 : len);
    printf("}");
}

static void seek(H t, H order, const char *c, const U8 *key, U16 len, U16 type, U16 how) {
    U16 found = 0x5A5A;
    U32 rc = ((U32(WINAPI *)(H, U8 *, U16, U16, U16, U16 *))F("AdsSeek"))(order, (U8 *)key, len,
                                                                          type, how, &found);
    char cc[200];
    snprintf(cc, sizeof cc, "%s type %u how %u", c, type, how);
    position(t, order, cc, "AdsSeek", rc);
}

static void every_order(H t, H order) {
    U16 len = sizeof index_name;
    ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsGetIndexName"))(order, (U8 *)index_name, &len);
    text_of(order, "AdsGetIndexName");
    text_of(order, "AdsGetIndexExpr");
    text_of(order, "AdsGetIndexCondition");
    flag_of(order, "AdsGetKeyLength");
    flag_of(order, "AdsGetKeyType");
    flag_of(order, "AdsIsIndexUnique");
    flag_of(order, "AdsIsIndexDescending");
    flag_of(order, "AdsIsIndexCompound");
    for (U16 option = 1; option <= 2; option++) {
        U32 n = 0x5A5A5A5A;
        U32 rc = ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetKeyCount"))(order, option, &n);
        begin(option == 1 ? "respect filters" : "ignore filters", "AdsGetKeyCount", rc);
        printf(", \"value\": %lu}", n);
    }
    U16 key_type = 0;
    ((U32(WINAPI *)(H, U16 *))F("AdsGetKeyType"))(order, &key_type);

    /* A walk to the end and back. Keys are kept for the seeks. */
    static U8 keys[40][300];
    static U16 key_lens[40];
    int distinct = 0;
    U32 rc = ((U32(WINAPI *)(H))F("AdsGotoTop"))(order);
    position(t, order, "top", "AdsGotoTop", rc);
    for (int i = 0; i < 4000; i++) {
        U16 eof = 1;
        ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(t, &eof);
        if (eof) break;
        U8 key[300];
        U16 klen = sizeof key;
        if (!((U32(WINAPI *)(H, U8 *, U16 *))F("AdsExtractKey"))(order, key, &klen) && distinct < 40 &&
            (distinct == 0 || key_lens[distinct - 1] != klen ||
             memcmp(keys[distinct - 1], key, klen))) {
            memcpy(keys[distinct], key, klen);
            key_lens[distinct++] = klen;
        }
        rc = ((U32(WINAPI *)(H, long))F("AdsSkip"))(order, 1);
        position(t, order, "forward", "AdsSkip", rc);
    }
    rc = ((U32(WINAPI *)(H))F("AdsGotoBottom"))(order);
    position(t, order, "bottom", "AdsGotoBottom", rc);
    for (int i = 0; i < 4000; i++) {
        U16 bof = 1;
        ((U32(WINAPI *)(H, U16 *))F("AdsAtBOF"))(t, &bof);
        if (bof) break;
        rc = ((U32(WINAPI *)(H, long))F("AdsSkip"))(order, -1);
        position(t, order, "backward", "AdsSkip", rc);
    }

    /* Seeks. */
    for (int k = 0; k < distinct; k++) {
        char c[40];
        snprintf(c, sizeof c, "key %d", k);
        seek(t, order, c, keys[k], key_lens[k], 2, 1);
        seek(t, order, c, keys[k], key_lens[k], 2, 2);
        seek(t, order, c, keys[k], key_lens[k], 1, 1);
        snprintf(c, sizeof c, "key %d first", k);
        seek(t, order, c, keys[k], 1, 2, 1);
        seek(t, order, c, keys[k], 1, 2, 4);
    }
    seek(t, order, "past", (const U8 *)"\xff\xff\xff", 3, 2, 1);
    seek(t, order, "past", (const U8 *)"\xff\xff\xff", 3, 2, 2);
    if (key_type != 4) {
        const char *texts[] = {"1.5", "-1.5", "0", "5", "2017-11-08", "11/08/2017", "20171108", "T"};
        for (int k = 0; k < 8; k++) {
            char c[60];
            snprintf(c, sizeof c, "text %s", texts[k]);
            seek(t, order, c, (const U8 *)texts[k], (U16)strlen(texts[k]), 2, 1);
        }
        double doubles[] = {1.5, -1.5, 0, 5, 2458066};
        for (int k = 0; k < 5; k++) {
            char c[60];
            snprintf(c, sizeof c, "double %g", doubles[k]);
            seek(t, order, c, (const U8 *)&doubles[k], 8, 4, 1);
        }
    }
    index_name[0] = 0;
}

int main(int argc, char **argv) {
    ace = LoadLibraryA(argv[1]);
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    H conn = 0;
    ((U32(WINAPI *)(char *, U16, H *))F("AdsConnect26"))(argv[2], 1, &conn);
    for (int a = 3; a + 3 < argc; a += 4) {
        U16 type = (U16)atoi(argv[a]), char_type = (U16)atoi(argv[a + 1]);
        table_name = argv[a + 2];
        char path[600];
        snprintf(path, sizeof path, "%s\\%s", argv[2], argv[a + 2]);
        for (char *p = path; *p; p++)
            if (*p == '/') *p = '\\';
        H t = 0;
        U32 rc = ((U32(WINAPI *)(H, char *, char *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
            conn, path, NULL, type, char_type, 1, 1, 0, &t);
        begin("open", "AdsOpenTable", rc);
        printf("}");
        if (rc) continue;
        /* The orders that opened with the table (a structural .cdx). */
        U16 open_orders = 0;
        ((U32(WINAPI *)(H, U16 *))F("AdsGetNumIndexes"))(t, &open_orders);
        snprintf(index_name, sizeof index_name, "structural");
        begin("open", "AdsGetNumIndexes", 0);
        printf(", \"orders\": %u}", open_orders);
        index_name[0] = 0;
        for (U16 i = 1; i <= open_orders; i++) {
            H order = 0;
            ((U32(WINAPI *)(H, U16, H *))F("AdsGetIndexHandleByOrder"))(t, i, &order);
            every_order(t, order);
        }
        char files[600];
        snprintf(files, sizeof files, "%s", argv[a + 3]);
        for (char *name = strtok(files, ","); name; name = strtok(NULL, ",")) {
            char index_path[700];
            snprintf(index_path, sizeof index_path, "%s", path);
            char *slash = strrchr(index_path, '\\');
            snprintf(slash + 1, sizeof index_path - (slash + 1 - index_path), "%s", name);
            H orders[50];
            U16 n = 50;
            rc = ((U32(WINAPI *)(H, U8 *, H *, U16 *))F("AdsOpenIndex"))(t, (U8 *)index_path, orders, &n);
            snprintf(index_name, sizeof index_name, "%s", name);
            begin("open", "AdsOpenIndex", rc);
            printf(", \"orders\": %u}", rc ? 0 : n);
            index_name[0] = 0;
            for (U16 i = 0; !rc && i < n; i++) every_order(t, orders[i]);
        }
        ((U32(WINAPI *)(H))F("AdsCloseTable"))(t);
    }
    return printf("\n]\n") < 0;
}
