/* Black-box probe of AdsGetKeyFilter: which unique order it picks and the exact
 * filter string it builds for the current record, under several index layouts
 * and options. No disassembly — documented API only.
 *
 *     keyfilter.exe <ace64.dll> <data folder>
 */
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
#define ADS_UNIQUE 0x00000001
#define ADS_ADT 3

static void err(const char *what, U32 rc) {
    U8 t[600]; U16 l = sizeof t; U32 c = 0;
    ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&c, t, &l);
    printf("    %s: rc=%lu %lu %s\n", what, (unsigned long)rc, (unsigned long)c, t);
}

static U32 setf(H t, int f, const char *v) {
    return ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetField"))(t, FIELD(f), (U8 *)v, strlen(v));
}
static U32 append(H t, const char *id, const char *code, const char *name, const char *val) {
    U32 rc = ((U32(WINAPI *)(H))F("AdsAppendRecord"))(t);
    if (!rc) rc = setf(t, 1, id);
    if (!rc) rc = setf(t, 2, code);
    if (!rc) rc = setf(t, 3, name);
    if (!rc) rc = setf(t, 4, val);
    if (!rc) rc = ((U32(WINAPI *)(H))F("AdsWriteRecord"))(t);
    return rc;
}

/* Call AdsGetKeyFilter on the current record and print the filter. */
static void keyfilter(H t, const char *label, U8 *values, U32 options) {
    U8 buf[2048];
    U32 len = sizeof buf;
    U32 rc = ((U32(WINAPI *)(H, U8 *, U32, U8 *, U32 *))F("AdsGetKeyFilter"))
        (t, values, options, buf, &len);
    if (rc) { printf("  %-28s", label); err("GetKeyFilter", rc); return; }
    buf[len < sizeof buf ? len : sizeof buf - 1] = 0;
    printf("  %-28s len=%lu  <<%s>>\n", label, (unsigned long)len, buf);
}

int main(int argc, char **argv) {
    if (argc < 3) { fprintf(stderr, "usage: keyfilter <dll> <folder>\n"); return 2; }
    const char *folder = argv[2];
    ace = LoadLibraryA(argv[1]);
    if (!ace) return fprintf(stderr, "cannot load %s\n", argv[1]), 1;
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);

    H conn = 0;
    if (((U32(WINAPI *)(U8 *, U16, H *))F("AdsConnect26"))((U8 *)folder, 1, &conn)) return 1;

    char path[MAX_PATH], adi[MAX_PATH];
    snprintf(path, sizeof path, "%s\\kf.adt", folder);
    snprintf(adi, sizeof adi, "%s\\kf.adi", folder);

    H t = 0;
    U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, U32, H *))F(
        "AdsCreateTable71"))(conn, (U8 *)path, NULL, ADS_ADT, 1, 1, 1, 0,
        (U8 *)"id,Integer;code,Character,8;name,Character,10;val,Double,15,2", 0, &t);
    if (rc) { err("CreateTable", rc); return 1; }

    /* Indexes: a non-unique on name, unique on code, unique on id, unique
     * compound on (name;id). Create in this order to see which GetKeyFilter
     * prefers among the unique ones. */
    H hname = 0, hcode = 0, hid = 0, hnameid = 0;
    typedef U32(WINAPI *CI)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, H *);
    CI ci = (CI)F("AdsCreateIndex");
    ci(t, (U8 *)adi, (U8 *)"BYNAME", (U8 *)"name", NULL, NULL, 0, &hname);
    ci(t, (U8 *)adi, (U8 *)"BYCODE", (U8 *)"code", NULL, NULL, ADS_UNIQUE, &hcode);
    ci(t, (U8 *)adi, (U8 *)"BYID", (U8 *)"id", NULL, NULL, ADS_UNIQUE, &hid);
    ci(t, (U8 *)adi, (U8 *)"NAMEID", (U8 *)"name;id", NULL, NULL, ADS_UNIQUE, &hnameid);

    append(t, "1", "AAA", "alpha", "1.5");
    append(t, "2", "BBB", "bravo", "2.25");
    append(t, "3", "CCC", "charlie", "3");

    ((U32(WINAPI *)(H))F("AdsGotoTop"))(t);
    ((U32(WINAPI *)(H, S32))F("AdsSkip"))(t, 1); /* record 2 */

    printf("record 2 (id=2, code=BBB, name=bravo, val=2.25):\n");
    keyfilter(t, "table handle", NULL, 0);
    keyfilter(t, "table odata", NULL, 0x1);
    keyfilter(hid, "BYID order handle", NULL, 0);
    keyfilter(hcode, "BYCODE order handle", NULL, 0);
    keyfilter(hname, "BYNAME (non-unique)", NULL, 0);
    keyfilter(hnameid, "NAMEID odata", NULL, 0x1);

    /* Table 2: the unique indexes created in the reverse order (BYID first),
     * to see whether the pick follows creation order. */
    char p2[MAX_PATH], a2[MAX_PATH];
    snprintf(p2, sizeof p2, "%s\\kf2.adt", folder);
    snprintf(a2, sizeof a2, "%s\\kf2.adi", folder);
    H t2 = 0;
    ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, U32, H *))F("AdsCreateTable71"))
        (conn, (U8 *)p2, NULL, ADS_ADT, 1, 1, 1, 0,
         (U8 *)"id,Integer;code,Character,8;name,Character,10;val,Double,15,2", 0, &t2);
    H h = 0;
    ci(t2, (U8 *)a2, (U8 *)"BYID", (U8 *)"id", NULL, NULL, ADS_UNIQUE, &h);
    ci(t2, (U8 *)a2, (U8 *)"BYCODE", (U8 *)"code", NULL, NULL, ADS_UNIQUE, &h);
    ci(t2, (U8 *)a2, (U8 *)"BYNAME", (U8 *)"name", NULL, NULL, 0, &h);
    append(t2, "1", "AAA", "alpha", "1.5");
    append(t2, "2", "BBB", "bravo", "2.25");
    ((U32(WINAPI *)(H))F("AdsGotoTop"))(t2);
    ((U32(WINAPI *)(H, S32))F("AdsSkip"))(t2, 1);
    printf("table 2 (BYID created first):\n");
    keyfilter(t2, "table2 default", NULL, 0);

    /* Table 3: only a compound unique (code;id), no single-field unique. */
    char p3[MAX_PATH], a3[MAX_PATH];
    snprintf(p3, sizeof p3, "%s\\kf3.adt", folder);
    snprintf(a3, sizeof a3, "%s\\kf3.adi", folder);
    H t3 = 0;
    ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, U32, H *))F("AdsCreateTable71"))
        (conn, (U8 *)p3, NULL, ADS_ADT, 1, 1, 1, 0,
         (U8 *)"id,Integer;code,Character,8;name,Character,10;val,Double,15,2", 0, &t3);
    ci(t3, (U8 *)a3, (U8 *)"BYNAME", (U8 *)"name", NULL, NULL, 0, &h);
    ci(t3, (U8 *)a3, (U8 *)"CID", (U8 *)"code;id", NULL, NULL, ADS_UNIQUE, &h);
    append(t3, "1", "AAA", "alpha", "1.5");
    append(t3, "2", "BBB", "bravo", "2.25");
    ((U32(WINAPI *)(H))F("AdsGotoTop"))(t3);
    ((U32(WINAPI *)(H, S32))F("AdsSkip"))(t3, 1);
    printf("table 3 (only compound unique code;id):\n");
    keyfilter(t3, "table3 default", NULL, 0);
    keyfilter(t3, "table3 odata", NULL, 0x1);

    /* Table 4: no unique index at all (one non-unique). */
    char p4[MAX_PATH], a4[MAX_PATH];
    snprintf(p4, sizeof p4, "%s\\kf4.adt", folder);
    snprintf(a4, sizeof a4, "%s\\kf4.adi", folder);
    H t4 = 0;
    ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, U32, H *))F("AdsCreateTable71"))
        (conn, (U8 *)p4, NULL, ADS_ADT, 1, 1, 1, 0,
         (U8 *)"id,Integer;code,Character,8;name,Character,10;val,Double,15,2", 0, &t4);
    ci(t4, (U8 *)a4, (U8 *)"BYNAME", (U8 *)"name", NULL, NULL, 0, &h);
    append(t4, "1", "AAA", "alpha", "1.5");
    append(t4, "2", "BBB", "bravo", "2.25");
    ((U32(WINAPI *)(H))F("AdsGotoTop"))(t4);
    ((U32(WINAPI *)(H, S32))F("AdsSkip"))(t4, 1);
    printf("table 4 (no unique index):\n");
    keyfilter(t4, "table4 default", NULL, 0);

    /* Table 5: tag names invert the field order — tag ACODE on code, ZID on id.
     * If the pick is alphabetical by tag, it takes ACODE (code). */
    char p5[MAX_PATH], a5[MAX_PATH];
    snprintf(p5, sizeof p5, "%s\\kf5.adt", folder);
    snprintf(a5, sizeof a5, "%s\\kf5.adi", folder);
    H t5 = 0;
    ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, U32, H *))F("AdsCreateTable71"))
        (conn, (U8 *)p5, NULL, ADS_ADT, 1, 1, 1, 0,
         (U8 *)"id,Integer;code,Character,8;name,Character,10;val,Double,15,2", 0, &t5);
    ci(t5, (U8 *)a5, (U8 *)"ZID", (U8 *)"id", NULL, NULL, ADS_UNIQUE, &h);
    ci(t5, (U8 *)a5, (U8 *)"ACODE", (U8 *)"code", NULL, NULL, ADS_UNIQUE, &h);
    append(t5, "1", "AAA", "alpha", "1.5");
    append(t5, "2", "BBB", "bravo", "2.25");
    ((U32(WINAPI *)(H))F("AdsGotoTop"))(t5);
    ((U32(WINAPI *)(H, S32))F("AdsSkip"))(t5, 1);
    printf("table 5 (tag ACODE on code, ZID on id):\n");
    keyfilter(t5, "table5 default", NULL, 0);

    /* Table 6: the opposite — tag AID on id, tag ZCODE on code. Alphabetical by
     * tag would take AID (id); alphabetical by field would take code. */
    char p6[MAX_PATH], a6[MAX_PATH];
    snprintf(p6, sizeof p6, "%s\\kf6.adt", folder);
    snprintf(a6, sizeof a6, "%s\\kf6.adi", folder);
    H t6 = 0;
    ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, U32, H *))F("AdsCreateTable71"))
        (conn, (U8 *)p6, NULL, ADS_ADT, 1, 1, 1, 0,
         (U8 *)"id,Integer;code,Character,8;name,Character,10;val,Double,15,2", 0, &t6);
    ci(t6, (U8 *)a6, (U8 *)"AID", (U8 *)"id", NULL, NULL, ADS_UNIQUE, &h);
    ci(t6, (U8 *)a6, (U8 *)"ZCODE", (U8 *)"code", NULL, NULL, ADS_UNIQUE, &h);
    append(t6, "1", "AAA", "alpha", "1.5");
    append(t6, "2", "BBB", "bravo", "2.25");
    ((U32(WINAPI *)(H))F("AdsGotoTop"))(t6);
    ((U32(WINAPI *)(H, S32))F("AdsSkip"))(t6, 1);
    printf("table 6 (tag AID on id, ZCODE on code):\n");
    keyfilter(t6, "table6 default", NULL, 0);

    ((U32(WINAPI *)(H))F("AdsCloseTable"))(t);
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
    return 0;
}
