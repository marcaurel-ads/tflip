/* Probes SQL GRANT/REVOKE and AdsDDGrantPermission/AdsDDRevokePermission with
 * object type 0 (ADS_DD_UNKNOWN_OBJECT, the object found by name across every
 * kind), driven by a script (grants.txt). Black-box: only the official ACE is
 * called.
 *
 *     grants.exe <ace64.dll> <out folder> <script>  > fixtures/dictionary/grants.json
 *
 * The script is a list of lines; `#` starts a comment. `@ name` starts a
 * scenario: a new empty dictionary <out>\<name>\db.add (`@= name` connects to
 * an existing one instead, as ADSSYS). Then, on it:
 *
 *     user NAME            AdsDDCreateUser (password pw)
 *     group NAME           AdsDDCreateUserGroup
 *     member GROUP USER    AdsDDAddUserToGroup
 *     table NAME           copies <out>\t.adt to NAME.adt, AdsDDAddTable
 *     view NAME SQL        AdsDDAddView
 *     proc NAME            AdsDDAddProcedure (.\x.dll, Proc, in a,I; out b,I;)
 *     link NAME            AdsDDCreateLink (to db.add as bob/pw, global)
 *     pub NAME             AdsDDCreatePublication
 *     sql STATEMENT        AdsExecuteSQLDirect
 *     dump STATEMENT       the same, printing the cursor's rows
 *     snap NAME            copies db.add to NAME.add beside it
 *     grant T|NAME|PARENT|GRANTEE|BITS    AdsDDGrantPermission
 *     revoke T|NAME|PARENT|GRANTEE|BITS   AdsDDRevokePermission
 *     get GRANTEE|T|NAME|PARENT|INHERITED AdsDDGetPermissions
 *
 * (an empty NAME or PARENT is an empty string; `-` is NULL). Each line prints
 * one JSON object {"scenario", "op", "rc"} (plus "error" for failed SQL, "mask"
 * for get, "names"/"rows" for dump); tflip replays them and compares its
 * dictionaries with the ones this leaves (see grants.sh). Built with
 * x86_64-w64-mingw32-gcc and run under Wine. */
#include <windows.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;

static HMODULE ace;
#define F(name) GetProcAddress(ace, name)
#define ADS_ADT  3
#define ADS_ANSI 1

static int first = 1;
static const char *out;
static char scenario[64];
static H c;

static void jstr(const char *s) {
    if (!s) { printf("null"); return; }
    putchar('"');
    for (; *s; s++) {
        if (*s == '"' || *s == '\\') printf("\\%c", *s);
        else if ((U8)*s < 32 || (U8)*s > 126) printf("\\u%04x", (U8)*s);
        else putchar(*s);
    }
    putchar('"');
}

static void begin(const char *op) {
    printf("%s\n{\"scenario\": ", first ? "[" : ",");
    jstr(scenario);
    printf(", \"op\": ");
    jstr(op);
    first = 0;
}

static void last_error(void) {
    U8 text[1024];
    U16 len = sizeof text;
    U32 code = 0;
    text[0] = 0;
    ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
    printf(", \"error\": ");
    jstr((char *)text);
}

/* Prints the rc of a non-SQL step (and the engine's error text if it failed). */
static void done(const char *op, U32 rc) {
    begin(op);
    printf(", \"rc\": %lu", (unsigned long)rc);
    if (rc) last_error();
    printf("}");
}

/* Copies <out>\t.adt to <out>\<scenario>\<name>.adt, freed from its dictionary
 * (see replops.c), so AddTable binds a fresh copy. */
static void copy_table(const char *name) {
    char src[MAX_PATH], dst[MAX_PATH];
    snprintf(src, sizeof src, "%s\\t.adt", out);
    snprintf(dst, sizeof dst, "%s\\%s\\%s.adt", out, scenario, name);
    CopyFileA(src, dst, FALSE);
    FILE *f = fopen(dst, "r+b");
    if (f) {
        U32 one = 1, zero = 0;
        char blank[260] = {0};
        fseek(f, 0x14, SEEK_SET); fwrite(&one, 4, 1, f);
        fseek(f, 0x5c, SEEK_SET); fwrite(blank, sizeof blank, 1, f);
        fseek(f, 0x160, SEEK_SET); fwrite(&zero, 4, 1, f);
        fclose(f);
    }
}

static void sql(const char *text, int dump) {
    H s = 0, cur = 0;
    ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(c, &s);
    U32 rc = ((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(s, (U8 *)text, &cur);
    begin(text);
    printf(", \"rc\": %lu", (unsigned long)rc);
    if (rc) last_error();
    if (rc == 0 && cur && dump) {
        U16 cols = 0;
        ((U32(WINAPI *)(H, U16 *))F("AdsGetNumFields"))(cur, &cols);
        printf(", \"names\": [");
        for (U16 f = 1; f <= cols; f++) {
            U8 nm[200];
            U16 nl = sizeof nm;
            ((U32(WINAPI *)(H, U16, U8 *, U16 *))F("AdsGetFieldName"))(cur, f, nm, &nl);
            if (f > 1) printf(", ");
            jstr((char *)nm);
        }
        printf("], \"rows\": [");
        ((U32(WINAPI *)(H))F("AdsGotoTop"))(cur);
        int firstrow = 1;
        for (;;) {
            U16 eof = 0;
            ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(cur, &eof);
            if (eof) break;
            printf("%s\n  [", firstrow ? "" : ",");
            firstrow = 0;
            for (U16 f = 1; f <= cols; f++) {
                U8 val[512];
                U32 len = sizeof val - 1;
                U16 empty = 0;
                memset(val, 0, sizeof val);
                ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsIsEmpty"))(cur, (U8 *)(ULONG_PTR)f, &empty);
                U32 frc = ((U32(WINAPI *)(H, U8 *, U8 *, U32 *, U16))F("AdsGetField"))
                    (cur, (U8 *)(ULONG_PTR)f, val, &len, 0);
                if (f > 1) printf(", ");
                if (frc == 0 && !empty) {
                    val[len < sizeof val ? len : sizeof val - 1] = 0;
                    for (int k = (int)strlen((char *)val) - 1; k >= 0 && val[k] == ' '; k--)
                        val[k] = 0;
                    jstr((char *)val);
                } else printf("null");
            }
            printf("]");
            ((U32(WINAPI *)(H, long))F("AdsSkip"))(cur, 1);
        }
        printf("]");
    }
    printf("}");
    if (cur) ((U32(WINAPI *)(H))F("AdsCloseTable"))(cur);
    ((U32(WINAPI *)(H))F("AdsCloseSQLStatement"))(s);
}

/* Splits `s` at '|' into up to n fields; "-" becomes NULL. */
static int split(char *s, char **f, int n) {
    int k = 0;
    while (k < n) {
        f[k++] = s;
        char *bar = strchr(s, '|');
        if (!bar) break;
        *bar = 0;
        s = bar + 1;
    }
    for (int i = 0; i < k; i++)
        if (!strcmp(f[i], "-")) f[i] = NULL;
    return k;
}

static void scenario_start(const char *name, int existing) {
    if (c) ((U32(WINAPI *)(H))F("AdsDisconnect"))(c);
    c = 0;
    snprintf(scenario, sizeof scenario, "%s", name);
    char dir[MAX_PATH], path[MAX_PATH];
    snprintf(dir, sizeof dir, "%s\\%s", out, name);
    CreateDirectoryA(dir, NULL);
    snprintf(path, sizeof path, "%s\\db.add", dir);
    U32 rc = existing
        ? ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))
              ((U8 *)path, 1, (U8 *)"ADSSYS", NULL, 0, &c)
        : ((U32(WINAPI *)(U8 *, U16, U8 *, H *))F("AdsDDCreate"))((U8 *)path, 0, NULL, &c);
    if (rc) fprintf(stderr, "%s: connect rc=%lu\n", name, (unsigned long)rc);
}

static void step(char *line) {
    char op[2048];
    snprintf(op, sizeof op, "%s", line);
    char *sp = strchr(line, ' ');
    char *arg = sp ? sp + 1 : line + strlen(line);
    if (sp) *sp = 0;
    const char *verb = line;
    U32 rc;
    if (!strcmp(verb, "as") || !strcmp(verb, "asadmin")) {
        /* Goes on as USER (password pw) or as ADSSYS on the scenario's dictionary. */
        char path[MAX_PATH];
        int admin = verb[2] == 'a';
        if (c) ((U32(WINAPI *)(H))F("AdsDisconnect"))(c);
        c = 0;
        snprintf(path, sizeof path, "%s\\%s\\db.add", out, scenario);
        rc = ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))
            ((U8 *)path, 1, (U8 *)(admin ? "ADSSYS" : arg), (U8 *)(admin ? NULL : "pw"), 0, &c);
        done(op, rc);
        return;
    }
    if (!strcmp(verb, "sql")) { sql(arg, 0); return; }
    if (!strcmp(verb, "dump")) { sql(arg, 1); return; }
    if (!strcmp(verb, "snap")) {
        char src[MAX_PATH], dst[MAX_PATH];
        snprintf(src, sizeof src, "%s\\%s\\db.add", out, scenario);
        snprintf(dst, sizeof dst, "%s\\%s\\%s.add", out, scenario, arg);
        done(op, CopyFileA(src, dst, FALSE) ? 0 : 1);
        return;
    }
    if (!strcmp(verb, "user"))
        rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *))F("AdsDDCreateUser"))
            (c, (U8 *)"", (U8 *)arg, (U8 *)"pw", (U8 *)"");
    else if (!strcmp(verb, "group"))
        rc = ((U32(WINAPI *)(H, U8 *, U8 *))F("AdsDDCreateUserGroup"))(c, (U8 *)arg, (U8 *)"");
    else if (!strcmp(verb, "member")) {
        char *user = strchr(arg, ' ');
        *user++ = 0;
        rc = ((U32(WINAPI *)(H, U8 *, U8 *))F("AdsDDAddUserToGroup"))(c, (U8 *)arg, (U8 *)user);
        user[-1] = ' ';
    } else if (!strcmp(verb, "table")) {
        char file[300];
        copy_table(arg);
        snprintf(file, sizeof file, "%s.adt", arg);
        rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U8 *, U8 *))F("AdsDDAddTable"))
            (c, (U8 *)arg, (U8 *)file, ADS_ADT, ADS_ANSI, (U8 *)"", (U8 *)"");
    } else if (!strcmp(verb, "view")) {
        char *text = strchr(arg, ' ');
        *text++ = 0;
        rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *))F("AdsDDAddView"))
            (c, (U8 *)arg, (U8 *)"", (U8 *)text);
        text[-1] = ' ';
    } else if (!strcmp(verb, "proc"))
        rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U32, U8 *, U8 *, U8 *))F("AdsDDAddProcedure"))
            (c, (U8 *)arg, (U8 *)".\\x.dll", (U8 *)"Proc", 1, (U8 *)"a,I;", (U8 *)"b,I;", (U8 *)"");
    else if (!strcmp(verb, "link"))
        rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U32))F("AdsDDCreateLink"))
            (c, (U8 *)arg, (U8 *)"db.add", (U8 *)"bob", (U8 *)"pw", 1);
    else if (!strcmp(verb, "pub"))
        rc = ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsDDCreatePublication"))
            (c, (U8 *)arg, (U8 *)"", 0);
    else if (!strcmp(verb, "verify")) {
        /* Turns the dictionary's access rights verification on or off. */
        U16 on = (U16)atoi(arg);
        rc = ((U32(WINAPI *)(H, U16, void *, U16))F("AdsDDSetDatabaseProperty"))
            (c, 104 /* ADS_DD_VERIFY_ACCESS_RIGHTS */, &on, 2);
    } else if (!strcmp(verb, "sub"))
        rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U8 *, U16, U8 *, U32))
              F("AdsDDCreateSubscription"))
            (c, (U8 *)arg, (U8 *)"pb", (U8 *)"\\\\srv\\t.add", (U8 *)"u", (U8 *)"pw",
             (U8 *)"q.add", 0, (U8 *)"", 0);
    else if (!strcmp(verb, "grant") || !strcmp(verb, "revoke")) {
        char *f[5];
        if (split(arg, f, 5) != 5) { fprintf(stderr, "bad: %s\n", op); return; }
        rc = ((U32(WINAPI *)(H, U16, U8 *, U8 *, U8 *, U32))
              F(verb[0] == 'g' ? "AdsDDGrantPermission" : "AdsDDRevokePermission"))
            (c, (U16)atoi(f[0]), (U8 *)f[1], (U8 *)f[2], (U8 *)f[3], strtoul(f[4], NULL, 0));
    } else if (!strcmp(verb, "get")) {
        char *f[5];
        if (split(arg, f, 5) != 5) { fprintf(stderr, "bad: %s\n", op); return; }
        U32 mask = 0xAAAAAAAA;
        rc = ((U32(WINAPI *)(H, U8 *, U16, U8 *, U8 *, U16, U32 *))F("AdsDDGetPermissions"))
            (c, (U8 *)f[0], (U16)atoi(f[1]), (U8 *)f[2], (U8 *)f[3], (U16)atoi(f[4]), &mask);
        begin(op);
        printf(", \"rc\": %lu, \"mask\": \"0x%lx\"}", (unsigned long)rc, (unsigned long)mask);
        return;
    } else {
        fprintf(stderr, "unknown step: %s\n", op);
        return;
    }
    done(op, rc);
}

int main(int argc, char **argv) {
    if (argc != 4) {
        fprintf(stderr, "usage: grants.exe <ace64.dll> <out folder> <script>\n");
        return 2;
    }
    ace = LoadLibraryA(argv[1]);
    if (!ace) { fprintf(stderr, "cannot load %s\n", argv[1]); return 1; }
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1); /* local server */
    out = argv[2];
    FILE *script = fopen(argv[3], "r");
    if (!script) { fprintf(stderr, "cannot read %s\n", argv[3]); return 1; }
    char line[2048];
    while (fgets(line, sizeof line, script)) {
        line[strcspn(line, "\r\n")] = 0;
        if (!line[0] || line[0] == '#') continue;
        if (line[0] == '@') {
            /* `@= name` connects to an existing <out>\<name>\db.add. */
            if (line[1] == '=') scenario_start(line + 3, 1);
            else scenario_start(line + 2, 0);
            continue;
        }
        step(line);
    }
    if (c) ((U32(WINAPI *)(H))F("AdsDisconnect"))(c);
    printf("\n]\n");
    return 0;
}
