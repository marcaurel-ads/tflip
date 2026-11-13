/* Records what AdsDisableAutoIncEnforcement / AdsDisableUniqueEnforcement do.
 * Exercises the table API, where the engine enforces AutoInc filling and unique
 * keys, to find the exact behaviour behind the `fixtures/write/enforce`
 * scenario: that with AutoInc enforcement off a client's own value is stored
 * and the table keeps its next value at max(next, value + 1), and that a
 * CANDIDATE order refuses a duplicate key (7057) only while unique enforcement
 * is on, adding it to the index when off.
 *
 * Built with i686-w64-mingw32-gcc and run under Wine:
 *
 *     SDK="$HOME/src/ace-sdk/.../acesdk"
 *     i686-w64-mingw32-gcc -O1 -Wall -I "$SDK" -o enforce.exe enforce.c
 *     WINEPREFIX=~/src/ace-sdk wine enforce.exe '<ace32.dll>' 'Z:\<data folder>'
 *
 * Prints a line per step: the call, its return code, and what was read back. */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef signed long S32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;

static HMODULE ace;
#define F(name) GetProcAddress(ace, name)

#define ADS_ADT 3
#define ADS_UNIQUE 0x00000001
#define ADS_CANDIDATE 0x00000800
#define ADS_IGNORERIGHTS 2
#define ADS_EXCLUSIVE 0x00000001

static H conn, t;

static U32 sql(H stmt, const char *s) {
    H cur = 0;
    return ((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(stmt, (U8 *)s, &cur);
}
static U32 append(void) { return ((U32(WINAPI *)(H))F("AdsAppendRecord"))(t); }
static U32 writerec(void) { return ((U32(WINAPI *)(H))F("AdsWriteRecord"))(t); }
static U32 set_long(const char *f, S32 v) {
    return ((U32(WINAPI *)(H, U8 *, S32))F("AdsSetLong"))(t, (U8 *)f, v);
}
static U32 set_str(const char *f, const char *v) {
    return ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetString"))(t, (U8 *)f, (U8 *)v, strlen(v));
}
static S32 get_long(const char *f) {
    S32 v = -1;
    ((U32(WINAPI *)(H, U8 *, S32 *))F("AdsGetLong"))(t, (U8 *)f, &v);
    return v;
}
static U32 count(void) {
    U32 n = 0;
    ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(t, 1, &n);
    return n;
}
static U32 disable_autoinc(void) {
    return ((U32(WINAPI *)(H))F("AdsDisableAutoIncEnforcement"))(conn);
}
static U32 enable_autoinc(void) {
    return ((U32(WINAPI *)(H))F("AdsEnableAutoIncEnforcement"))(conn);
}
static U32 disable_unique(void) {
    return ((U32(WINAPI *)(H))F("AdsDisableUniqueEnforcement"))(conn);
}
static U32 enable_unique(void) {
    return ((U32(WINAPI *)(H))F("AdsEnableUniqueEnforcement"))(conn);
}

int main(int argc, char **argv) {
    if (argc < 3) {
        fprintf(stderr, "usage: enforce.exe <ace32.dll> <data folder>\n");
        return 2;
    }
    ace = LoadLibraryA(argv[1]);
    if (!ace) { fprintf(stderr, "LoadLibrary failed\n"); return 2; }
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    U32 rc = ((U32(WINAPI *)(U8 *, U16, H *))F("AdsConnect26"))((U8 *)argv[2], 1, &conn);
    if (rc) { fprintf(stderr, "AdsConnect26 = %lu\n", rc); return 2; }
    H stmt = 0;
    ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(conn, &stmt);
    char folder[900];
    snprintf(folder, sizeof folder, "%s", argv[2]);

    /* ---- AutoInc ---- */
    sql(stmt, "DROP TABLE ai");
    rc = sql(stmt, "CREATE TABLE ai (k AUTOINC, v CHAR(10))");
    printf("autoinc: CREATE TABLE rc=%lu\n", rc);
    char path[1024];
    snprintf(path, sizeof path, "%s\\ai", folder);
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
        conn, (U8 *)path, NULL, ADS_ADT, 1, 1, ADS_IGNORERIGHTS, ADS_EXCLUSIVE, &t);
    printf("autoinc: open rc=%lu\n", rc);

    append(); set_str("v", "a"); rc = writerec();
    printf("A1 default append: write=%lu k=%ld count=%lu\n", rc, (long)get_long("k"), count());
    append(); set_str("v", "b"); rc = writerec();
    printf("A2 default append: write=%lu k=%ld count=%lu\n", rc, (long)get_long("k"), count());

    /* Explicit set while enforcement is ON. */
    rc = append();
    U32 sl = set_long("k", 500); set_str("v", "c-on");
    U32 wr = writerec();
    printf("A3 enforced set k=500: append=%lu setlong=%lu write=%lu k=%ld count=%lu\n",
           rc, sl, wr, (long)get_long("k"), count());

    rc = disable_autoinc();
    printf("A4 AdsDisableAutoIncEnforcement rc=%lu\n", rc);

    rc = append();
    sl = set_long("k", 100); set_str("v", "d-off");
    wr = writerec();
    printf("A5 disabled set k=100: append=%lu setlong=%lu write=%lu k=%ld count=%lu\n",
           rc, sl, wr, (long)get_long("k"), count());

    /* Append without setting k while disabled: what does the engine fill? */
    append(); set_str("v", "e-off");
    wr = writerec();
    printf("A6 disabled no-set: write=%lu k=%ld count=%lu\n", wr, (long)get_long("k"), count());

    /* Set a value below the current counter while disabled. */
    append(); set_long("k", 7); set_str("v", "f-off");
    wr = writerec();
    printf("A7 disabled set k=7: write=%lu k=%ld count=%lu\n", wr, (long)get_long("k"), count());

    rc = enable_autoinc();
    append(); set_str("v", "g-on");
    wr = writerec();
    printf("A8 re-enabled no-set: enable=%lu write=%lu k=%ld count=%lu\n",
           rc, wr, (long)get_long("k"), count());

    ((U32(WINAPI *)(H))F("AdsCloseTable"))(t);

    /* ---- Unique (CANDIDATE = true unique) ---- */
    sql(stmt, "DROP TABLE uq");
    rc = sql(stmt, "CREATE TABLE uq (k CHAR(4), v CHAR(10))");
    printf("\nunique: CREATE TABLE rc=%lu\n", rc);
    snprintf(path, sizeof path, "%s\\uq", folder);
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
        conn, (U8 *)path, NULL, ADS_ADT, 1, 1, ADS_IGNORERIGHTS, ADS_EXCLUSIVE, &t);
    printf("unique: open rc=%lu\n", rc);
    H idx = 0;
    char ipath[1024];
    snprintf(ipath, sizeof ipath, "%s\\uq.adi", folder);
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, U32, H *))F("AdsCreateIndex61"))(
        t, (U8 *)ipath, (U8 *)"UK", (U8 *)"k", NULL, NULL, ADS_CANDIDATE, 0, &idx);
    printf("unique: CreateIndex CANDIDATE rc=%lu\n", rc);

    append(); set_str("k", "AAAA"); set_str("v", "first"); rc = writerec();
    printf("U1 insert AAAA: write=%lu count=%lu\n", rc, count());

    rc = append(); set_str("k", "AAAA"); set_str("v", "dup-on");
    wr = writerec();
    printf("U2 enforced dup AAAA: append=%lu write=%lu count=%lu\n", rc, wr, count());

    rc = disable_unique();
    printf("U3 AdsDisableUniqueEnforcement rc=%lu\n", rc);

    rc = append(); set_str("k", "AAAA"); set_str("v", "dup-off");
    wr = writerec();
    printf("U4 disabled dup AAAA: append=%lu write=%lu count=%lu\n", rc, wr, count());

    /* Is the duplicate reachable through the unique order? Seek AAAA, then skip. */
    U16 found = 0;
    ((U32(WINAPI *)(H, U8 *, U16, U16, U16, U16 *))F("AdsSeek"))(
        idx, (U8 *)"AAAA", 4, 0 /*char*/, 0 /*eq*/, &found);
    char vbuf[32]; U32 vlen = sizeof vbuf; memset(vbuf, 0, sizeof vbuf);
    ((U32(WINAPI *)(H, U8 *, U8 *, U32 *, U16))F("AdsGetString"))(t, (U8 *)"v", (U8 *)vbuf, &vlen, 0);
    printf("U5 seek AAAA: found=%u v=%s\n", found, vbuf);
    ((U32(WINAPI *)(H, S32))F("AdsSkip"))(idx, 1);
    char vbuf2[32]; vlen = sizeof vbuf2; memset(vbuf2, 0, sizeof vbuf2);
    char kbuf2[32]; U32 klen = sizeof kbuf2; memset(kbuf2, 0, sizeof kbuf2);
    ((U32(WINAPI *)(H, U8 *, U8 *, U32 *, U16))F("AdsGetString"))(t, (U8 *)"k", (U8 *)kbuf2, &klen, 0);
    ((U32(WINAPI *)(H, U8 *, U8 *, U32 *, U16))F("AdsGetString"))(t, (U8 *)"v", (U8 *)vbuf2, &vlen, 0);
    printf("U6 skip +1 after seek: k=%s v=%s\n", kbuf2, vbuf2);

    rc = enable_unique();
    rc = append(); set_str("k", "AAAA"); set_str("v", "dup-on2");
    wr = writerec();
    printf("U7 re-enabled dup AAAA: enable write=%lu count=%lu\n", wr, count());

    /* A plain xBase UNIQUE order (not candidate) for comparison. */
    sql(stmt, "DROP TABLE ux");
    sql(stmt, "CREATE TABLE ux (k CHAR(4), v CHAR(10))");
    snprintf(path, sizeof path, "%s\\ux", folder);
    H t2 = 0;
    ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
        conn, (U8 *)path, NULL, ADS_ADT, 1, 1, ADS_IGNORERIGHTS, ADS_EXCLUSIVE, &t2);
    H idx2 = 0;
    snprintf(ipath, sizeof ipath, "%s\\ux.adi", folder);
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, U32, H *))F("AdsCreateIndex61"))(
        t2, (U8 *)ipath, (U8 *)"UX", (U8 *)"k", NULL, NULL, ADS_UNIQUE, 0, &idx2);
    H save = t; t = t2;
    append(); set_str("k", "BBBB"); set_str("v", "first"); writerec();
    rc = append(); set_str("k", "BBBB"); set_str("v", "dup"); wr = writerec();
    printf("\nX1 xBase UNIQUE enforced dup (ON): append=%lu write=%lu count=%lu\n", rc, wr, count());
    t = save;

    /* Is a duplicate key added to the index when enforcement is off? Compare the
     * index's key count with the record count after inserting a dup with the
     * toggle off from the start. */
    sql(stmt, "DROP TABLE uq2");
    sql(stmt, "CREATE TABLE uq2 (k CHAR(4), v CHAR(10))");
    snprintf(path, sizeof path, "%s\\uq2", folder);
    H t3 = 0;
    ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
        conn, (U8 *)path, NULL, ADS_ADT, 1, 1, ADS_IGNORERIGHTS, ADS_EXCLUSIVE, &t3);
    H idx3 = 0;
    snprintf(ipath, sizeof ipath, "%s\\uq2.adi", folder);
    ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, U32, H *))F("AdsCreateIndex61"))(
        t3, (U8 *)ipath, (U8 *)"UK2", (U8 *)"k", NULL, NULL, ADS_CANDIDATE, 0, &idx3);
    disable_unique();
    t = t3;
    append(); set_str("k", "AAAA"); set_str("v", "one"); writerec();
    append(); set_str("k", "AAAA"); set_str("v", "two"); wr = writerec();
    append(); set_str("k", "BBBB"); set_str("v", "three"); writerec();
    U32 keycount = 0;
    ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetKeyCount"))(idx3, 1, &keycount);
    printf("\nK1 off-dup AAAA,AAAA,BBBB: records=%lu indexkeys=%lu (dup write=%lu)\n",
           count(), keycount, wr);
    /* Seek AAAA and walk the order to list reachable records. */
    U16 found2 = 0;
    ((U32(WINAPI *)(H, U8 *, U16, U16, U16, U16 *))F("AdsSeek"))(
        idx3, (U8 *)"AAAA", 4, 2 /*ADS_STRINGKEY*/, 0 /*eq*/, &found2);
    printf("K2 seek AAAA found=%u; order walk:", found2);
    ((U32(WINAPI *)(H))F("AdsGotoTop"))(idx3);
    for (int i = 0; i < 6; i++) {
        U16 eof = 1;
        ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(idx3, &eof);
        if (eof) break;
        char kb[16]; U32 kl = sizeof kb; memset(kb, 0, sizeof kb);
        ((U32(WINAPI *)(H, U8 *, U8 *, U32 *, U16))F("AdsGetString"))(t3, (U8 *)"k", (U8 *)kb, &kl, 0);
        printf(" %s", kb);
        ((U32(WINAPI *)(H, S32))F("AdsSkip"))(idx3, 1);
    }
    printf("\n");
    t = save;

    fflush(stdout);
    return 0;
}
