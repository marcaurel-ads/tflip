/* Adds to a copy of fixtures/dictionary/new/indexed_table.add the kinds of
 * object that dictionary has none of, for fixtures/errors/param_n.json: a
 * group, a view, a procedure, a trigger, a link, a publication with an
 * article, and a subscription.
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o param_n_dict.exe param_n_dict.c
 *     wine param_n_dict.exe <ace32.dll> <dictionary indexed_table.add as a Windows path>
 *
 * (see param_n.sh, which keeps the result in fixtures/errors/param_n/). */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;
static HMODULE ace;
#define F(name) ((void *)GetProcAddress(ace, name))

static void check(const char *what, U32 rc) {
    if (!rc) return;
    U32 code = 0;
    U8 text[1200] = "";
    U16 len = sizeof text;
    ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
    fprintf(stderr, "%s: %lu %s\n", what, rc, text);
}

int main(int argc, char **argv) {
    if (argc != 3) return fprintf(stderr, "usage: param_n_dict.exe <ace32.dll> <dictionary>\n"), 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) return fprintf(stderr, "cannot load %s\n", argv[1]), 1;
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    H c = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
        (U8 *)argv[2], 1, (U8 *)"ADSSYS", (U8 *)"", 0, &c);
    check("connect", rc);
    if (rc) return 1;
    check("group", ((U32(WINAPI *)(H, U8 *, U8 *))F("AdsDDCreateUserGroup"))(c, (U8 *)"grp", (U8 *)"a group"));
    check("view", ((U32(WINAPI *)(H, U8 *, U8 *, U8 *))F("AdsDDAddView"))(
                      c, (U8 *)"vw", (U8 *)"a view", (U8 *)"SELECT id FROM t"));
    check("procedure", ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U32, U8 *, U8 *, U8 *))F("AdsDDAddProcedure"))(
                           c, (U8 *)"prc", (U8 *)"lib.dll", (U8 *)"go", 1, NULL, NULL, NULL));
    /* An after-insert trigger in a DLL (ADS_TRIGTYPE_AFTER, _INSERT, ADS_TRIG_WIN32DLL). */
    check("trigger", ((U32(WINAPI *)(H, U8 *, U8 *, U32, U32, U32, U8 *, U8 *, U32, U8 *, U32))F(
                          "AdsDDCreateTrigger"))(c, (U8 *)"trg", (U8 *)"t", 4, 1, 1, (U8 *)"lib.dll",
                                                 (U8 *)"go", 1, (U8 *)"a trigger", 0));
    check("link", ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U32))F("AdsDDCreateLink"))(
                      c, (U8 *)"lnk", (U8 *)"other.add", (U8 *)"adssys", (U8 *)"", 1));
    check("publication", ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsDDCreatePublication"))(
                             c, (U8 *)"pub", (U8 *)"a publication", 0));
    check("article", ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U32))F("AdsDDCreateArticle"))(
                         c, (U8 *)"pub", (U8 *)"t", (U8 *)"id", (U8 *)"id > 5", 0));
    check("subscription", ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U8 *, U16, U8 *, U32))F(
                               "AdsDDCreateSubscription"))(c, (U8 *)"sub", (U8 *)"pub", (U8 *)"\\\\srv\\t.add",
                                                           (U8 *)"u", (U8 *)"pw", (U8 *)"q.add", 1,
                                                           (U8 *)"a subscription", 0));
    /* The group's rights on a table (ADS_DD_TABLE_OBJECT 1), a view (6), a procedure (10) and a link (12). */
    {
        U32(WINAPI * grant)(H, U16, U8 *, U8 *, U8 *, U32) = F("AdsDDGrantPermission");
        check("grant table", grant(c, 1, (U8 *)"t", NULL, (U8 *)"grp", 0x3));
        check("grant view", grant(c, 6, (U8 *)"vw", NULL, (U8 *)"grp", 0x1));
        check("grant procedure", grant(c, 10, (U8 *)"prc", NULL, (U8 *)"grp", 0x4));
        check("grant link", grant(c, 12, (U8 *)"lnk", NULL, (U8 *)"grp", 0x40));
    }
    /* A group for each bit of rights, to see what each is called in the group's rights properties. */
    {
        U32(WINAPI * grant)(H, U16, U8 *, U8 *, U8 *, U32) = F("AdsDDGrantPermission");
        U32 bits[] = {0x1, 0x2, 0x4, 0x8, 0x10, 0x20, 0x40, 0x80, 0x100, 0x200, 0x80000000, 0x8FFFFFFF, 0x13, 0x23, 0x33, 0x32, 0x31, 0x30, 0x102, 0x82, 0x80000033, 0xFFFFFFFF};
        for (int i = 0; i < 22; i++) {
            char name[16];
            sprintf(name, "g%lx", bits[i]);
            check("group", ((U32(WINAPI *)(H, U8 *, U8 *))F("AdsDDCreateUserGroup"))(c, (U8 *)name, (U8 *)""));
            check("grant", grant(c, 1, (U8 *)"t", NULL, (U8 *)name, bits[i]));
            check("grant view", grant(c, 6, (U8 *)"vw", NULL, (U8 *)name, bits[i]));
            check("grant procedure", grant(c, 10, (U8 *)"prc", NULL, (U8 *)name, bits[i]));
        }
    }
    /* A rule between a parent table with a primary key and a child table with a tag. */
    {
        const char *sql[] = {"CREATE TABLE pt (id INTEGER CONSTRAINT pk PRIMARY KEY, s CHAR(10))",
                             "CREATE TABLE ct (id INTEGER, pid INTEGER)",
                             "CREATE INDEX cpid ON ct (pid)"};
        for (int i = 0; i < 3; i++) {
            H st = 0, cur = 0;
            check("statement", ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(c, &st));
            check(sql[i], ((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(st, (U8 *)sql[i], &cur));
            ((U32(WINAPI *)(H))F("AdsCloseSQLStatement"))(st);
        }
    }
    check("ri", ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U8 *, U16, U16))F("AdsDDCreateRefIntegrity"))(
                    c, (U8 *)"ri", (U8 *)"", (U8 *)"pt", (U8 *)"pk", (U8 *)"ct", (U8 *)"cpid", 2, 2));
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(c);
    return 0;
}
