/* Records the key type, key length and seek behaviour of index orders built on
 * an EXPRESSION whose result is numeric, date or logical (not just character),
 * for fixtures/expr/keytype.json and the expr_keys fixture.
 *
 *     keytype.exe <ace32.dll> <folder> build   -- create expr_keys/keys.adt
 *     keytype.exe <ace32.dll> <folder>         -- probe > keytype.json
 *
 * Built with i686-w64-mingw32-gcc and run under Wine (see run_keytype.sh). The
 * table `keys` (a free table) has id/n/d/dt/c/lg columns and these expression
 * orders:
 *   X_NUM   = n + 1        X_DBL  = d * 2       X_DATE = dt + 1
 *   X_LOG   = n > 0        X_UPPER = UPPER(c)   (character control)
 * Rows: (1, 5, 2.5, 2017-11-08, 'abc', T), (2, 10, 4.0, 2018-01-01, 'xyz', F),
 *       (3, -3, 0.0, 1999-12-31, 'mmm', F).
 *
 * The probe records, per order, AdsGetKeyType and AdsGetKeyLength, and seeks
 * X_NUM (a numeric expression) and X_DBL with a double key, recording whether
 * the key was found and the id of the record landed on — so that both the
 * reported type and the seek of a numeric expression order can be matched. */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef long S32;
typedef ULONG_PTR H;

static HMODULE ace;
#define F(name) GetProcAddress(ace, name)
#define FIELD(n) ((U8 *)(ULONG_PTR)(n))
#define ADS_ADT 3
#define ADS_DOUBLEKEY 4
#define ADS_HARDSEEK 0x0002

static int first = 1;

static void begin(const char *op) {
    printf("%s\n{\"op\": \"%s\"", first ? "[" : ",", op);
    first = 0;
}

static U32 setf(H t, int f, const char *v) {
    return ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetField"))(t, FIELD(f), (U8 *)v, strlen(v));
}

static void append(H t, const char *id, const char *n, const char *d, const char *dt,
                   const char *c, const char *lg) {
    ((U32(WINAPI *)(H))F("AdsAppendRecord"))(t);
    setf(t, 1, id);
    setf(t, 2, n);
    setf(t, 3, d);
    setf(t, 4, dt);
    setf(t, 5, c);
    setf(t, 6, lg);
    ((U32(WINAPI *)(H))F("AdsWriteRecord"))(t);
}

typedef U32(WINAPI *CI)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, H *);

static void build(const char *folder) {
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    H conn = 0;
    if (((U32(WINAPI *)(U8 *, U16, H *))F("AdsConnect26"))((U8 *)folder, 1, &conn)) {
        fprintf(stderr, "connect failed\n");
        return;
    }
    char path[MAX_PATH], adi[MAX_PATH];
    snprintf(path, sizeof path, "%s\\keys.adt", folder);
    snprintf(adi, sizeof adi, "%s\\keys.adi", folder);
    H t = 0;
    U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, U32, H *))F(
        "AdsCreateTable71"))(conn, (U8 *)path, NULL, ADS_ADT, 1, 1, 1, 0,
        (U8 *)"id,Integer;n,Integer;d,Double,15,2;dt,Date;c,Character,5;lg,Logical", 0, &t);
    if (rc) { fprintf(stderr, "CreateTable rc=%lu\n", rc); return; }
    CI ci = (CI)F("AdsCreateIndex");
    H h = 0;
    ci(t, (U8 *)adi, (U8 *)"X_NUM", (U8 *)"n + 1", NULL, NULL, 0, &h);
    ci(t, (U8 *)adi, (U8 *)"X_DBL", (U8 *)"d * 2", NULL, NULL, 0, &h);
    ci(t, (U8 *)adi, (U8 *)"X_DATE", (U8 *)"dt + 1", NULL, NULL, 0, &h);
    ci(t, (U8 *)adi, (U8 *)"X_LOG", (U8 *)"n > 0", NULL, NULL, 0, &h);
    ci(t, (U8 *)adi, (U8 *)"X_UPPER", (U8 *)"UPPER(c)", NULL, NULL, 0, &h);
    append(t, "1", "5", "2.5", "2017-11-08", "abc", "T");
    append(t, "2", "10", "4.0", "2018-01-01", "xyz", "F");
    append(t, "3", "-3", "0.0", "1999-12-31", "mmm", "F");
    ((U32(WINAPI *)(H))F("AdsCloseTable"))(t);
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
}

static H conn, table;

/* Record a single order's key type and length. */
static void order_info(const char *name) {
    H ord = 0;
    ((U32(WINAPI *)(H, U8 *, H *))F("AdsGetIndexHandle"))(table, (U8 *)name, &ord);
    U16 kt = 0xffff, kl = 0xffff;
    ((U32(WINAPI *)(H, U16 *))F("AdsGetKeyType"))(ord, &kt);
    ((U32(WINAPI *)(H, U16 *))F("AdsGetKeyLength"))(ord, &kl);
    begin("keytype");
    printf(", \"order\": \"%s\", \"key_type\": %u, \"key_length\": %u}", name, kt, kl);
}

/* Seek `name` for the double `key`, record found and the landed record's id. */
static void seek_double(const char *name, double key) {
    H ord = 0;
    ((U32(WINAPI *)(H, U8 *, H *))F("AdsGetIndexHandle"))(table, (U8 *)name, &ord);
    U16 found = 0;
    ((U32(WINAPI *)(H, U8 *, U16, U16, U16, U16 *))F("AdsSeek"))(
        ord, (U8 *)&key, 8, ADS_DOUBLEKEY, ADS_HARDSEEK, &found);
    U8 id[32];
    U32 len = sizeof id - 1;
    memset(id, 0, sizeof id);
    U32 frc = ((U32(WINAPI *)(H, U8 *, U8 *, U32 *, U16))F("AdsGetField"))(
        table, (U8 *)"id", id, &len, 0);
    char op[48];
    snprintf(op, sizeof op, "seek %s %g", name, key);
    begin(op);
    printf(", \"found\": %u, \"id\": ", found);
    if (found && frc == 0) { id[len < sizeof id ? len : sizeof id - 1] = 0; printf("\"%s\"", id); }
    else printf("null");
    printf("}");
}

static void probe(const char *folder) {
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    U32 crc = ((U32(WINAPI *)(U8 *, U16, H *))F("AdsConnect26"))((U8 *)folder, 1, &conn);
    if (crc) {
        fprintf(stderr, "connect rc=%lu folder=%s\n", crc, folder);
        printf("[]\n");
        return;
    }
    char path[MAX_PATH];
    snprintf(path, sizeof path, "%s\\keys.adt", folder);
    U32 orc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
        conn, (U8 *)path, NULL, ADS_ADT, 1, 0, 2 /*ADS_IGNORERIGHTS*/, 0, &table);
    if (orc) {
        fprintf(stderr, "open rc=%lu path=%s\n", orc, path);
        printf("[]\n");
        return;
    }
    const char *orders[] = {"X_NUM", "X_DBL", "X_DATE", "X_LOG", "X_UPPER"};
    for (int i = 0; i < 5; i++) order_info(orders[i]);
    seek_double("X_NUM", 11.0); /* n=10 -> n+1=11 -> id 2 */
    seek_double("X_NUM", 6.0);  /* n=5  -> n+1=6  -> id 1 */
    seek_double("X_NUM", 99.0); /* absent */
    seek_double("X_DBL", 8.0);  /* d=4.0 -> d*2=8 -> id 2 */
    ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
    printf("\n]\n");
}

int main(int argc, char **argv) {
    if (argc < 3) { fprintf(stderr, "usage: keytype.exe <ace32.dll> <folder> [build]\n"); return 2; }
    ace = LoadLibraryA(argv[1]);
    if (!ace) { fprintf(stderr, "cannot load %s\n", argv[1]); return 2; }
    if (argc >= 4 && strcmp(argv[3], "build") == 0) build(argv[2]);
    else probe(argv[2]);
    FreeLibrary(ace);
    return 0;
}
