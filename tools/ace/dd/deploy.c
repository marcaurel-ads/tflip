/* Probes AdsDDDeployDatabase of the real Advantage Client Engine: a source
 * dictionary in <root>\<scenario>\S, free tables in <root>\<scenario>\D.
 *
 *     deploy.exe <ace32.dll> <root> prepare|run [scenario prefix]
 *
 * `prepare` makes the folders of every scenario (the source's tables are made
 * on a free connection and added to a dictionary newdb.add; the destination's
 * tables, as the scenario says, are made beside them); `run` calls the
 * function on each and prints one RESULT line per scenario: the folders and
 * options it was given and the result code and error text. deploy.sh copies
 * the folders before and after into fixtures/dictionary/deploy/. The
 * scenarios' flags are explained at `Scenario`. Built with
 * i686-w64-mingw32-gcc and run under Wine.
 *
 * What the scenarios show (see the replay in tflip-engine's
 * dictionary_write/deploy.rs):
 * - the dictionary's tables are looked for in the destination by their
 *   relative paths: one that is not there is made empty, one with other
 *   fields is restructured (the old file stays as .BAK), and its index files
 *   and orders are built where they are missing, and orders the dictionary
 *   does not have are dropped;
 * - a table in a folder is looked for there (sub_a); one that is not there
 *   is made, and the folders with it (subm_a, subm_b), its dictionary's
 *   name one dot longer for each folder down (`..\newdb.add`, `...\`); an index
 *   file the AdsDDAddTable call names is named from the table's folder, or
 *   it is 7041 (`sub\t.adi` for `sub\t.adt`);
 * - the destination gets a dictionary made anew, not a copy: the source's
 *   objects are added to a new one, each with a new id, tables with their
 *   fields and index files first, then views, links, procedures, triggers,
 *   referential integrity rules, publications, packages, functions, groups
 *   and users, each group and user followed by its permissions (x_big);
 *   what names another object, or is enciphered with the object's own id,
 *   is written again (x_users, x_grant, x_ri, x_link); revoked permissions
 *   and the built-in groups of a user are not put back (x_dropped,
 *   x_sqlusers); a trigger's script is trimmed (x_trigger); the settings
 *   the source did not make are made, and the log tells them (x_dbset); the
 *   dictionary's version is the one its users, rights and triggers need
 *   (x_dropped); the source's are renamed adsbackup.*, and adsdeploy.log
 *   tells the steps;
 * - a subscription is added to the new dictionary with its queue table, the
 *   objects the source's ids give them, and the queue's files are made in the
 *   folder (x_sub, x_sub2), but for a queue with a folder in its name, which
 *   has none (x_subdir); a table's rights granted to DB:Public stop it with
 *   5136 after that, and free the queue's table as the others (x_subpub), also
 *   with a database comment (x_dbcpub); the new dictionary's key is another
 *   than the source's (random), so a subscription's password is enciphered anew;
 * - what it drops of a field's properties: its validation message always, and
 *   by a rule not worked out the others (w_*), the limits of a Clipper or
 *   FoxPro table's fields (d_cdx, d_ntx) and of a character field of a Visual
 *   FoxPro table (d_vfp2), whose ADT-format fail table the deployment also
 *   makes (d_vfp); the records of those tables are not moved (d_cdx: none
 *   deleted), and those of one with memo values go to a fail table with its
 *   own memo file (d_memov, d_vfpmemov: c_fail.adt with a c_fail.fpt);
 * - validation 2 takes the records that break a constraint out of the table
 *   (they are deleted) one constraint at a time, the record validation
 *   first, then each field's NOT NULL, minimum and maximum in field order,
 *   and makes the fail table `fail\<table>_fail.adt` anew for each, so the
 *   records of the last stay in it (v_*, o*, constraint*);
 * - a destination with tables of a dictionary is 5159, and one with the
 *   source's name in it 5128; the server type is 1 or 2, the validation
 *   option 0 or 2 and the backup 0 or 1, else 5019 (the validation's checked
 *   first, then the server type, then the backup);
 * - a table of the dictionary without an index file next to which the
 *   destination has a production index stops the deployment with the result
 *   0, an empty dictionary in the destination and the source left as it is
 *   (drop_index, dbf_extra_index). */
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
#define ADS_NTX 1
#define ADS_CDX 2
#define ADS_ADT 3
#define ADS_VFP 4
#define ADS_ANSI 1
#define ADS_COMPOUND 2

static char root[MAX_PATH];

static void oops(const char *what, U32 rc) {
    U8 text[1024];
    U16 len = sizeof text;
    U32 code = 0;
    text[0] = 0;
    ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
    printf("%s: rc=%lu last=%lu %s\n", what, rc, code, text);
}

static H free_connection(const char *dir) {
    H c = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
        (U8 *)dir, 1, NULL, NULL, 0, &c);
    if (rc) oops("connect", rc);
    return c;
}

static void disconnect(H c) { ((U32(WINAPI *)(H))F("AdsDisconnect"))(c); }

/* Makes <dir>\<file> with three records (1, "one" ...), and an index. */
static void make_table(H c, const char *dir, const char *file, U16 type, const char *fields,
                       const char *index, const char *tag) {
    char path[MAX_PATH];
    snprintf(path, sizeof path, "%s\\%s", dir, file);
    for (char *slash = strchr(path + strlen(dir) + 1, '\\'); slash;
         slash = strchr(slash + 1, '\\')) {
        *slash = 0;
        CreateDirectoryA(path, NULL);
        *slash = '\\';
    }
    H t = 0;
    U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *))F(
        "AdsCreateTable"))(c, (U8 *)path, NULL, type, ADS_ANSI, 0, 1, 0, (U8 *)fields, &t);
    if (rc) { oops(file, rc); return; }
    const char *names[] = {"one", "two", "three"};
    for (int i = 0; i < 3; i++) {
        ((U32(WINAPI *)(H))F("AdsAppendRecord"))(t);
        char id[8];
        snprintf(id, sizeof id, "%d", i + 1);
        ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetField"))(t, (U8 *)"id", (U8 *)id,
                                                              strlen(id));
        ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetField"))(t, (U8 *)"name", (U8 *)names[i],
                                                              strlen(names[i]));
        if (!strncmp(file, "mv", 2)) {
            /* A file starting with "mv" has memo values: its memo fields are m or notes. */
            char memo[40];
            snprintf(memo, sizeof memo, "memo value %d", i + 1);
            ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetField"))(t, (U8 *)"m", (U8 *)memo,
                                                                  strlen(memo));
            ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetField"))(t, (U8 *)"notes", (U8 *)memo,
                                                                  strlen(memo));
        }
        ((U32(WINAPI *)(H))F("AdsWriteRecord"))(t);
    }
    if (!strncmp(file, "nl", 2)) {
        ((U32(WINAPI *)(H))F("AdsAppendRecord"))(t);
        ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetField"))(t, (U8 *)"id", (U8 *)"4", 1);
        ((U32(WINAPI *)(H))F("AdsWriteRecord"))(t);
    }
    if (index) {
        char ipath[MAX_PATH];
        snprintf(ipath, sizeof ipath, "%s\\%s", dir, index);
        H i = 0;
        rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, U32, H *))F(
            "AdsCreateIndex61"))(t, (U8 *)ipath, (U8 *)tag, (U8 *)"id", (U8 *)"", (U8 *)"",
                                 tag ? ADS_COMPOUND : 0, 0, &i);
        if (rc) oops(index, rc);
    }
    ((U32(WINAPI *)(H))F("AdsCloseTable"))(t);
}

static H dictionary(const char *dir, const char *name) {
    char path[MAX_PATH];
    snprintf(path, sizeof path, "%s\\%s", dir, name);
    H c = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, H *))F("AdsDDCreate"))((U8 *)path, 0, NULL, &c);
    if (rc) oops("AdsDDCreate", rc);
    return c;
}

static void add_table(H c, const char *name, const char *file, U16 type, const char *index) {
    U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U8 *, U8 *))F("AdsDDAddTable"))(
        c, (U8 *)name, (U8 *)file, type, ADS_ANSI, (U8 *)index, (U8 *)"");
    if (rc) oops(name, rc);
}

static void folders(const char *scenario, char *s, char *d) {
    char top[MAX_PATH];
    snprintf(top, sizeof top, "%s\\%s", root, scenario);
    CreateDirectoryA(top, NULL);
    snprintf(s, MAX_PATH, "%s\\S", top);
    snprintf(d, MAX_PATH, "%s\\D", top);
    CreateDirectoryA(s, NULL);
    CreateDirectoryA(d, NULL);
}

/* A table of a scenario: made in a folder, then added to the dictionary
 * as `name` when it is in the source folder. */
typedef struct {
    const char *name, *file;
    U16 type;
    const char *fields, *index, *tag;
} Tab;

#define TNONE {NULL, NULL, 0, NULL, NULL, NULL}
#define ID_NAME "id,Integer;name,Character,10"
#define T_PLAIN {"t", "t.adt", ADS_ADT, ID_NAME, NULL, NULL}
#define T_INDEXED {"t", "t.adt", ADS_ADT, ID_NAME, "t.adi", "byid"}

typedef struct {
    const char *name;
    Tab src[4], dst[4];
    U16 types, validate, backup;
    /* o: the destination holds a dictionary, s: the source is in the destination, n: no
     * destination folder, m: no source dictionary, v: a view, a user and a group (V, G, U:
     * each alone), c: a constraint on t.id, w: an administrator password on the source, p: a
     * destination password, q: a source password, k: the source's files copied to the
     * destination, e: an empty dictionary there, z: other files and a folder there, b: a
     * backup folder with a file there, f: a fail table there */
    const char *flags;
    /* Statements run on the source dictionary after its tables are added, one per line: SQL,
     * or "!group g", "!user g u" (g may be empty), "!link l", "!pub p", "!sub s p", "!art p t",
     * "!ri name parent parentindex child childindex", "!dbn id value size" and "!dbt id text"
     * (database properties), "!tprop table id text", "!fprop table field id text" and "!fnull
     * table field" (table and field properties). A table whose file starts with "nl" has a
     * fourth record with no name. */
    const char *steps;
} Scenario;

static int has(const Scenario *sc, char flag) { return sc->flags && strchr(sc->flags, flag); }

static void make_all(const char *dir, const Tab *tabs) {
    H c = free_connection(dir);
    for (int i = 0; i < 4 && tabs[i].name; i++)
        make_table(c, dir, tabs[i].file, tabs[i].type, tabs[i].fields, tabs[i].index,
                   tabs[i].tag);
    disconnect(c);
}

static void add_all(H c, const Tab *tabs) {
    for (int i = 0; i < 4 && tabs[i].name; i++)
    {
        /* An index file the table's folder holds is named from there. */
        const char *index = tabs[i].index ? tabs[i].index : "";
        const char *slash = strrchr(tabs[i].file, '\\');
        if (slash && !strncmp(index, tabs[i].file, slash + 1 - tabs[i].file))
            index += slash + 1 - tabs[i].file;
        add_table(c, tabs[i].name, tabs[i].file, tabs[i].type, index);
    }
}

static void write_file(const char *dir, const char *name, const char *text) {
    char path[MAX_PATH];
    snprintf(path, sizeof path, "%s\\%s", dir, name);
    FILE *f = fopen(path, "wb");
    if (f) { fputs(text, f); fclose(f); }
}

static const char *cur_s;

static void run_steps(H c, const char *steps) {
    char *copy = strdup(steps);
    H stmt = 0;
    ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(c, &stmt);
    char *save = NULL;
    for (char *line = strtok_r(copy, "\n", &save); line; line = strtok_r(NULL, "\n", &save)) {
        U32 rc = 0;
        if (line[0] != '!') {
            H cur = 0;
            rc = ((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(stmt, (U8 *)line, &cur);
            if (cur) ((U32(WINAPI *)(H))F("AdsCloseTable"))(cur);
        } else if (!strncmp(line, "!dbn ", 5)) {
            /* a database property that is a number: "!dbn <id> <value> <size>" */
            unsigned id, value, size;
            sscanf(line + 5, "%u %u %u", &id, &value, &size);
            rc = ((U32(WINAPI *)(H, U16, void *, U16))F("AdsDDSetDatabaseProperty"))(
                c, (U16)id, &value, (U16)size);
        } else if (!strncmp(line, "!dbt ", 5)) {
            /* a database property that is text: "!dbt <id> <text>" */
            unsigned id;
            char text[200];
            sscanf(line + 5, "%u %199[^\n]", &id, text);
            rc = ((U32(WINAPI *)(H, U16, void *, U16))F("AdsDDSetDatabaseProperty"))(
                c, (U16)id, text, (U16)strlen(text));
        } else if (!strncmp(line, "!tprop ", 7)) {
            /* a table property that is text: "!tprop <table> <id> <text>" */
            char table[64], text[200];
            unsigned id;
            sscanf(line + 7, "%63s %u %199[^\n]", table, &id, text);
            rc = ((U32(WINAPI *)(H, U8 *, U16, void *, U16, U16, U8 *))F("AdsDDSetTableProperty"))(
                c, (U8 *)table, (U16)id, text, (U16)(strlen(text) + 1), 0, NULL);
        } else if (!strncmp(line, "!fprop ", 7)) {
            /* a field property that is text: "!fprop <table> <field> <id> <text>" */
            char table[64], field[64], text[200];
            unsigned id;
            sscanf(line + 7, "%63s %63s %u %199[^\n]", table, field, &id, text);
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, void *, U16, U16, U8 *))F(
                "AdsDDSetFieldProperty"))(c, (U8 *)table, (U8 *)field, (U16)id, text,
                                          (U16)(strlen(text) + 1), 0, NULL);
        } else if (!strncmp(line, "!fnull ", 7)) {
            /* a field that cannot be NULL: "!fnull <table> <field>" */
            char table[64], field[64];
            U16 no = 0;
            sscanf(line + 7, "%63s %63s", table, field);
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, void *, U16, U16, U8 *))F(
                "AdsDDSetFieldProperty"))(c, (U8 *)table, (U8 *)field, 301, &no, 2, 0, NULL);
        } else if (!strncmp(line, "!group ", 7)) {
            rc = ((U32(WINAPI *)(H, U8 *, U8 *))F("AdsDDCreateUserGroup"))(c, (U8 *)line + 7,
                                                                           (U8 *)"a group");
        } else if (!strncmp(line, "!user ", 6)) {
            char *g = line + 6, *u = strchr(g, ' ');
            *u++ = 0;
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *))F("AdsDDCreateUser"))(
                c, (U8 *)g, (U8 *)u, (U8 *)"pw", (U8 *)"a user");
        } else if (!strncmp(line, "!ri ", 4)) {
            char *f[5];
            f[0] = strtok(line + 4, " ");
            for (int i = 1; i < 5; i++) f[i] = strtok(NULL, " ");
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U8 *, U16, U16, U8 *, U8 *))F(
                "AdsDDCreateRefIntegrity62"))(c, (U8 *)f[0], NULL, (U8 *)f[1], (U8 *)f[2],
                                              (U8 *)f[3], (U8 *)f[4], 2, 2, (U8 *)"no parent",
                                              (U8 *)"cascade failed");
        } else if (!strncmp(line, "!link ", 6)) {
            char other[MAX_PATH];
            snprintf(other, sizeof other, "%s\\other.add", cur_s);
            if (GetFileAttributesA(other) == INVALID_FILE_ATTRIBUTES)
                disconnect(dictionary(cur_s, "other.add"));
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U32))F("AdsDDCreateLink"))(
                c, (U8 *)line + 6, (U8 *)"other.add", (U8 *)"adssys", (U8 *)"", 1);
        } else if (!strncmp(line, "!pub ", 5)) {
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsDDCreatePublication"))(
                c, (U8 *)line + 5, (U8 *)"", 0);
        } else if (!strncmp(line, "!art ", 5)) {
            char *p = line + 5, *t = strchr(p, ' ');
            *t++ = 0;
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U32))F("AdsDDCreateArticle"))(
                c, (U8 *)p, (U8 *)t, (U8 *)"id", (U8 *)"", 0);
        } else if (!strncmp(line, "!sub ", 5)) {
            char *s = line + 5, *p = strchr(s, ' ');
            *p++ = 0;
            /* "!sub <name> <publication> [queue [comment]]": the queue is q.add if none is given */
            char *queue = strchr(p, ' ');
            char *comment = "";
            if (queue) {
                *queue++ = 0;
                comment = strchr(queue, ' ');
                if (comment) *comment++ = 0; else comment = "";
            } else queue = "q.add";
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U8 *, U16, U8 *, U32))F(
                "AdsDDCreateSubscription"))(c, (U8 *)s, (U8 *)p, (U8 *)"\\\\srv\\t.add",
                                            (U8 *)"u", (U8 *)"pw", (U8 *)queue, 0, (U8 *)comment, 0);
        }
        if (rc) {
            printf("STEP FAILED %s\n", line);
            oops(line, rc);
        }
    }
    ((U32(WINAPI *)(H))F("AdsCloseSQLStatement"))(stmt);
    free(copy);
}

static void prepare(const Scenario *sc) {
    char s[MAX_PATH], d[MAX_PATH];
    folders(sc->name, s, d);
    const char *sdir = has(sc, 's') ? d : s;
    make_all(sdir, sc->src);
    H c = dictionary(sdir, "newdb.add");
    add_all(c, sc->src);
    if (has(sc, 'v')) {
        U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *))F("AdsDDAddView"))(
            c, (U8 *)"v1", (U8 *)"a view", (U8 *)"SELECT id FROM t");
        if (rc) oops("view", rc);
        rc = ((U32(WINAPI *)(H, U8 *, U8 *))F("AdsDDCreateUserGroup"))(c, (U8 *)"grp",
                                                                       (U8 *)"a group");
        if (rc) oops("group", rc);
        rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *))F("AdsDDCreateUser"))(
            c, (U8 *)"grp", (U8 *)"bob", (U8 *)"pw", (U8 *)"a user");
        if (rc) oops("user", rc);
    }
    if (has(sc, 'V')) {
        U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *))F("AdsDDAddView"))(
            c, (U8 *)"v1", (U8 *)"a view", (U8 *)"SELECT id FROM t");
        if (rc) oops("view", rc);
    }
    if (has(sc, 'G')) {
        U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *))F("AdsDDCreateUserGroup"))(c, (U8 *)"grp",
                                                                           (U8 *)"a group");
        if (rc) oops("group", rc);
    }
    if (has(sc, 'U')) {
        U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *))F("AdsDDCreateUser"))(
            c, NULL, (U8 *)"bob", (U8 *)"pw", (U8 *)"a user");
        if (rc) oops("user", rc);
    }
    cur_s = sdir;
    if (sc->steps) run_steps(c, sc->steps);
    if (has(sc, 'c')) {
        U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, void *, U16, U16, U8 *))F(
            "AdsDDSetFieldProperty"))(c, (U8 *)"t", (U8 *)"id", 302, "2", 2, 0, NULL);
        if (rc) oops("min", rc);
    }
    if (has(sc, 'w')) {
        U32 rc = ((U32(WINAPI *)(H, U16, void *, U16))F("AdsDDSetDatabaseProperty"))(
            c, 101, "secret", 7);
        if (rc) oops("adminpw", rc);
    }
    disconnect(c);
    if (has(sc, 'k')) {
        make_all(d, sc->dst);
        const char *ext[] = {"add", "am", "ai"};
        for (int i = 0; i < 3; i++) {
            char from[MAX_PATH], to[MAX_PATH];
            snprintf(from, sizeof from, "%s\\newdb.%s", s, ext[i]);
            snprintf(to, sizeof to, "%s\\newdb.%s", d, ext[i]);
            CopyFileA(from, to, FALSE);
        }
    } else if (has(sc, 'e')) {
        make_all(d, sc->dst);
        disconnect(dictionary(d, "other.add"));
    } else if (has(sc, 'o')) {
        make_all(d, sc->dst);
        c = dictionary(d, "other.add");
        add_all(c, sc->dst);
        disconnect(c);
    } else if (!has(sc, 's')) {
        make_all(d, sc->dst);
    }
    if (has(sc, 'z')) {
        char sub[MAX_PATH];
        write_file(d, "notes.txt", "notes");
        snprintf(sub, sizeof sub, "%s\\inner", d);
        CreateDirectoryA(sub, NULL);
        write_file(sub, "inner.txt", "inner");
    }
    if (has(sc, 'b')) {
        char sub[MAX_PATH];
        snprintf(sub, sizeof sub, "%s\\backup", d);
        CreateDirectoryA(sub, NULL);
        write_file(sub, "t.adt", "old backup");
        write_file(sub, "stale.txt", "stale");
    }
    if (has(sc, 'f')) {
        char sub[MAX_PATH];
        snprintf(sub, sizeof sub, "%s\\fail", d);
        CreateDirectoryA(sub, NULL);
        write_file(sub, "t_fail.adt", "old fail");
    }
}

static Scenario scenarios[] = {
    {"same", {T_INDEXED}, {T_INDEXED}, 1, 0, 0},
    {"same_backup", {T_INDEXED}, {T_INDEXED}, 1, 0, 1},
    {"missing", {T_INDEXED, {"u", "u.adt", ADS_ADT, ID_NAME, NULL, NULL}}, {T_INDEXED}, 1, 0, 0},
    {"add_field", {{"t", "t.adt", ADS_ADT, ID_NAME ";extra,Integer", NULL, NULL}}, {T_PLAIN}, 1, 0, 0},
    {"drop_field", {{"t", "t.adt", ADS_ADT, "id,Integer", NULL, NULL}}, {T_PLAIN}, 1, 0, 0},
    {"add_index", {T_INDEXED}, {T_PLAIN}, 1, 0, 0},
    {"drop_index", {T_PLAIN}, {T_INDEXED}, 1, 0, 0},
    {"extra_free", {T_PLAIN}, {T_PLAIN, {"x", "x.adt", ADS_ADT, ID_NAME, NULL, NULL}}, 1, 0, 0},
    {"dest_dict", {T_PLAIN}, {T_PLAIN}, 1, 0, 0, "o"},
    {"same_folder", {T_INDEXED}, {TNONE}, 1, 0, 0, "s"},
    {"same_folder_backup", {T_INDEXED}, {TNONE}, 1, 0, 1, "s"},
    {"no_dest", {T_PLAIN}, {T_PLAIN}, 1, 0, 0, "n"},
    {"no_source", {T_PLAIN}, {T_PLAIN}, 1, 0, 0, "m"},
    {"obj_view", {T_PLAIN}, {T_PLAIN}, 1, 0, 0, "V"},
    {"obj_group", {T_PLAIN}, {T_PLAIN}, 1, 0, 0, "G"},
    {"obj_user", {T_PLAIN}, {T_PLAIN}, 1, 0, 0, "U"},
    {"objects", {T_PLAIN}, {T_PLAIN}, 1, 0, 0, "v"},
    {"types0", {T_PLAIN}, {T_PLAIN}, 0, 0, 0},
    {"types2", {T_PLAIN}, {T_PLAIN}, 2, 0, 0},
    {"types3", {T_PLAIN}, {T_PLAIN}, 3, 0, 0},
    {"types4", {T_PLAIN}, {T_PLAIN}, 4, 0, 0},
    {"types8", {T_PLAIN}, {T_PLAIN}, 8, 0, 0},
    {"validate1", {T_PLAIN}, {T_PLAIN}, 1, 1, 0},
    {"validate2", {T_PLAIN}, {T_PLAIN}, 1, 2, 0},
    {"validate3", {T_PLAIN}, {T_PLAIN}, 1, 3, 0},
    {"backup2", {T_PLAIN}, {T_PLAIN}, 1, 0, 2},
    {"constraint", {T_PLAIN}, {T_PLAIN}, 1, 2, 0, "c"},
    {"constraint_nv", {T_PLAIN}, {T_PLAIN}, 1, 0, 0, "c"},
    {"dest_pw", {T_PLAIN}, {T_PLAIN}, 1, 0, 0, "p"},
    {"src_pw_none", {T_PLAIN}, {T_PLAIN}, 1, 0, 0, "w"},
    {"src_pw_given", {T_PLAIN}, {T_PLAIN}, 1, 0, 0, "wq"},
    {"src_pw_users", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "wq", "!group grp\n!user grp bob\n!user  cara"},
    {"src_pw_grant", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "wq", "!group grp\n!user grp bob\nGRANT SELECT ON t TO grp\nGRANT INSERT, UPDATE ON t TO bob"},
    {"src_pw_dropped", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "wq", "!group g1\nGRANT SELECT ON t TO g1\nREVOKE SELECT ON t FROM g1"},
    {"src_pw_link", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "wq", "!link lnk"},
    {"dest_free_dict", {T_PLAIN}, {T_PLAIN}, 1, 0, 0, "e"},
    {"doc_same", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "k"},
    {"doc_same_backup", {T_INDEXED}, {T_INDEXED}, 1, 0, 1, "k"},
    {"backup_extra", {T_PLAIN}, {T_PLAIN, {"x", "x.adt", ADS_ADT, ID_NAME ";m,Memo", "x.adi", "byid"}}, 1, 0, 1},
    {"backup_exists", {T_PLAIN}, {T_PLAIN}, 1, 0, 1, "b"},
    {"fail_exists", {T_PLAIN}, {T_PLAIN}, 1, 2, 0, "cf"},
    {"constraint_backup", {T_PLAIN}, {T_PLAIN}, 1, 2, 1, "c"},
    {"change_field", {{"t", "t.adt", ADS_ADT, "id,Integer;name,Character,20", NULL, NULL}}, {T_PLAIN}, 1, 0, 0},
    {"memo_same", {{"t", "t.adt", ADS_ADT, ID_NAME ";m,Memo", NULL, NULL}}, {{"t", "t.adt", ADS_ADT, ID_NAME ";m,Memo", NULL, NULL}}, 1, 0, 0},
    {"memo_add", {{"t", "t.adt", ADS_ADT, ID_NAME ";m,Memo", NULL, NULL}}, {T_PLAIN}, 1, 0, 0},
    {"memo_missing", {T_PLAIN, {"u", "u.adt", ADS_ADT, ID_NAME ";m,Memo", "u.adi", "byid"}}, {T_PLAIN}, 1, 0, 0},
    {"dbf_same", {{"c", "c.dbf", ADS_CDX, "id,Numeric,5,0;name,Character,10;notes,Memo", "c.cdx", "byid"},
                  {"n", "n.dbf", ADS_NTX, "id,Numeric,5,0;name,Character,10", "n_id.ntx", NULL},
                  {"v", "v.dbf", ADS_VFP, ID_NAME, "v.cdx", "byid"}},
     {{"c", "c.dbf", ADS_CDX, "id,Numeric,5,0;name,Character,10;notes,Memo", "c.cdx", "byid"},
      {"n", "n.dbf", ADS_NTX, "id,Numeric,5,0;name,Character,10", "n_id.ntx", NULL},
      {"v", "v.dbf", ADS_VFP, ID_NAME, "v.cdx", "byid"}}, 1, 0, 1},
    {"dbf_missing", {{"c", "c.dbf", ADS_CDX, "id,Numeric,5,0;name,Character,10;notes,Memo", "c.cdx", "byid"},
                  {"n", "n.dbf", ADS_NTX, "id,Numeric,5,0;name,Character,10", "n_id.ntx", NULL},
                  {"v", "v.dbf", ADS_VFP, ID_NAME, "v.cdx", "byid"}}, {T_PLAIN}, 1, 0, 0},
    {"types5", {T_PLAIN}, {T_PLAIN}, 5, 0, 0},
    {"types6", {T_PLAIN}, {T_PLAIN}, 6, 0, 0},
    {"types7", {T_PLAIN}, {T_PLAIN}, 7, 0, 0},
    {"types65535", {T_PLAIN}, {T_PLAIN}, 65535, 0, 0},
    {"bad_all", {T_PLAIN}, {T_PLAIN}, 9, 9, 9},
    {"bad_all_nodest", {T_PLAIN}, {T_PLAIN}, 9, 9, 9, "n"},
    {"bad_types_nosrc", {T_PLAIN}, {T_PLAIN}, 9, 0, 0, "m"},
    {"bad_validate_backup", {T_PLAIN}, {T_PLAIN}, 1, 9, 9},
    {"backup_misc", {T_PLAIN}, {T_PLAIN}, 1, 0, 1, "ez"},
    {"types9_backup9", {T_PLAIN}, {T_PLAIN}, 9, 0, 9},
    {"types1_backup9_validate1", {T_PLAIN}, {T_PLAIN}, 1, 1, 9},
    {"types0_backup9", {T_PLAIN}, {T_PLAIN}, 0, 0, 9},
    {"idx_other", {{"t", "t.adt", ADS_ADT, ID_NAME, "t.adi", "byid"}}, {{"t", "t.adt", ADS_ADT, ID_NAME, "t.adi", "byid2"}}, 1, 0, 0},
    {"dbf_extra_index", {{"c", "c.dbf", ADS_CDX, "id,Numeric,5,0;name,Character,10", NULL, NULL}}, {{"c", "c.dbf", ADS_CDX, "id,Numeric,5,0;name,Character,10", "c.cdx", "byid"}}, 1, 0, 0},
    {"dbf_drop_field", {{"c", "c.dbf", ADS_CDX, "id,Numeric,5,0", NULL, NULL}}, {{"c", "c.dbf", ADS_CDX, "id,Numeric,5,0;name,Character,10", NULL, NULL}}, 1, 0, 0},
    {"dbf_add_field", {{"c", "c.dbf", ADS_CDX, "id,Numeric,5,0;name,Character,10;n2,Character,5", "c.cdx", "byid"}}, {{"c", "c.dbf", ADS_CDX, "id,Numeric,5,0;name,Character,10", "c.cdx", "byid"}}, 1, 0, 0},
    {"x_users", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "!group grp\n!user grp bob\n!user  cara"},
    {"x_proc", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "CREATE PROCEDURE pr1 (a INTEGER) BEGIN DELETE FROM t WHERE id = a; END"},
    {"x_func", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "CREATE FUNCTION f1 (a INTEGER) RETURNS INTEGER BEGIN RETURN a + 1; END"},
    {"x_pkg", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "CREATE PACKAGE pk1 DESCRIPTION 'a package'\nCREATE FUNCTION pk1.f2 (a INTEGER) RETURNS INTEGER BEGIN RETURN a + 2; END"},
    {"x_trigger", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "CREATE TRIGGER trg1 ON t AFTER INSERT BEGIN INSERT INTO __error SELECT 1 FROM __new WHERE 1 = 0; END"},
    {"x_grant", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "!group grp\n!user grp bob\nGRANT SELECT ON t TO grp\nGRANT INSERT, UPDATE ON t TO bob"},
    {"x_ri", {T_INDEXED, {"u", "u.adt", ADS_ADT, ID_NAME, "u.adi", "bypid"}}, {T_INDEXED, {"u", "u.adt", ADS_ADT, ID_NAME, "u.adi", "bypid"}}, 1, 0, 0, "", "CREATE UNIQUE INDEX t_pk ON t (id)\nEXECUTE PROCEDURE sp_ModifyTableProperty ('t', 'TABLE_PRIMARY_KEY', 't_pk', 'NO_VALIDATE', NULL)\n!ri ri1 t t_pk u bypid"},
    {"x_link", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "!link lnk"},
    {"x_pub", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "!pub pub1\n!art pub1 t"},
    {"x_big", {T_INDEXED, {"u", "u.adt", ADS_ADT, ID_NAME, "u.adi", "bypid"}}, {T_INDEXED, {"u", "u.adt", ADS_ADT, ID_NAME, "u.adi", "bypid"}}, 1, 0, 0, "", "!user  zed\n!group g2\n!group g1\n!user g2 mia\n!user g1 abe\n!user g1 bea\nCREATE PROCEDURE p2 (a INTEGER) BEGIN DELETE FROM t WHERE id = a; END\nCREATE PROCEDURE p1 (a INTEGER) BEGIN DELETE FROM u WHERE id = a; END\nCREATE VIEW v2 AS SELECT id FROM t\nCREATE VIEW v1 AS SELECT id FROM u\nCREATE FUNCTION f2 (a INTEGER) RETURNS INTEGER BEGIN RETURN a + 1; END\nCREATE FUNCTION f1 (a INTEGER) RETURNS INTEGER BEGIN RETURN a + 2; END\nCREATE PACKAGE pk1 DESCRIPTION 'a package'\nCREATE FUNCTION pk1.f3 (a INTEGER) RETURNS INTEGER BEGIN RETURN a + 3; END\nCREATE TRIGGER trg2 ON u AFTER INSERT BEGIN INSERT INTO __error SELECT 1 FROM __new WHERE 1 = 0; END\nCREATE TRIGGER trg1 ON t AFTER INSERT BEGIN INSERT INTO __error SELECT 1 FROM __new WHERE 1 = 0; END\nGRANT SELECT ON u TO g2\nGRANT SELECT ON t TO g1\nGRANT INSERT ON t TO abe\nGRANT SELECT ON t TO zed\nCREATE UNIQUE INDEX t_pk ON t (id)\nEXECUTE PROCEDURE sp_ModifyTableProperty ('t', 'TABLE_PRIMARY_KEY', 't_pk', 'NO_VALIDATE', NULL)\n!ri ri2 t t_pk u bypid\n!link l2\n!link l1\n!pub pb2\n!pub pb1\n!art pb1 t\n!art pb1 u"},
    {"x_desc", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "!group g1\n!user  zed\n!user g1 amy\nCREATE PROCEDURE p1 (a INTEGER) BEGIN DELETE FROM t WHERE id = a; END\nCREATE VIEW v1 AS SELECT id FROM t\nCREATE FUNCTION f1 (a INTEGER) RETURNS INTEGER DESCRIPTION 'func desc' BEGIN RETURN a + 1; END\nCREATE PACKAGE pk1 DESCRIPTION 'pack desc'\nCREATE FUNCTION pk1.f2 (a INTEGER) RETURNS INTEGER DESCRIPTION 'pkg func desc' BEGIN RETURN a + 3; END\nCREATE TRIGGER trg1 ON t AFTER INSERT BEGIN INSERT INTO __error SELECT 1 FROM __new WHERE 1 = 0; END\nEXECUTE PROCEDURE sp_ModifyTableProperty ('t', 'COMMENT', 'table comment', 'NO_VALIDATE', NULL)\nEXECUTE PROCEDURE sp_ModifyFieldProperty ('t', 'id', 'COMMENT', 'field comment', 'NO_VALIDATE', NULL)\nEXECUTE PROCEDURE sp_ModifyDatabase ('COMMENT', 'db comment')\n!link lnk\n!pub pub1"},
    {"x_long", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "CREATE FUNCTION f1 (a INTEGER) RETURNS INTEGER DESCRIPTION 'a func' BEGIN DECLARE x INTEGER; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; x = a + 1; RETURN x; END\nCREATE PROCEDURE p1 (a INTEGER) BEGIN DELETE FROM t WHERE id = a; DELETE FROM t WHERE id = a; DELETE FROM t WHERE id = a; DELETE FROM t WHERE id = a; DELETE FROM t WHERE id = a; DELETE FROM t WHERE id = a; DELETE FROM t WHERE id = a; DELETE FROM t WHERE id = a; DELETE FROM t WHERE id = a; DELETE FROM t WHERE id = a; DELETE FROM t WHERE id = a; DELETE FROM t WHERE id = a; DELETE FROM t WHERE id = a; DELETE FROM t WHERE id = a; DELETE FROM t WHERE id = a; DELETE FROM t WHERE id = a; DELETE FROM t WHERE id = a; DELETE FROM t WHERE id = a; DELETE FROM t WHERE id = a; DELETE FROM t WHERE id = a; END\nCREATE TRIGGER trg1 ON t AFTER INSERT BEGIN INSERT INTO __error SELECT 1 FROM __new WHERE 1 = 0; INSERT INTO __error SELECT 1 FROM __new WHERE 1 = 0; INSERT INTO __error SELECT 1 FROM __new WHERE 1 = 0; INSERT INTO __error SELECT 1 FROM __new WHERE 1 = 0; INSERT INTO __error SELECT 1 FROM __new WHERE 1 = 0; INSERT INTO __error SELECT 1 FROM __new WHERE 1 = 0; INSERT INTO __error SELECT 1 FROM __new WHERE 1 = 0; INSERT INTO __error SELECT 1 FROM __new WHERE 1 = 0; INSERT INTO __error SELECT 1 FROM __new WHERE 1 = 0; INSERT INTO __error SELECT 1 FROM __new WHERE 1 = 0; INSERT INTO __error SELECT 1 FROM __new WHERE 1 = 0; INSERT INTO __error SELECT 1 FROM __new WHERE 1 = 0; END\nCREATE VIEW v1 AS SELECT id AS c0, id AS c1, id AS c2, id AS c3, id AS c4, id AS c5, id AS c6, id AS c7, id AS c8, id AS c9, id AS c10, id AS c11, id AS c12, id AS c13, id AS c14, id AS c15, id AS c16, id AS c17, id AS c18, id AS c19, id AS c20, id AS c21, id AS c22, id AS c23, id AS c24, id AS c25, id AS c26, id AS c27, id AS c28, id AS c29, id AS c30, id AS c31, id AS c32, id AS c33, id AS c34, id AS c35, id AS c36, id AS c37, id AS c38, id AS c39 FROM t\n!group g1\n!user g1 amy"},
    {"x_sqlusers", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "CREATE PROCEDURE p1 (a INTEGER) BEGIN DELETE FROM t WHERE id = a; END\nEXECUTE PROCEDURE sp_CreateGroup ('g2', '')\nEXECUTE PROCEDURE sp_CreateGroup ('g3', 'desc3')\nEXECUTE PROCEDURE sp_CreateUser ('u2', 'pw', '')\nEXECUTE PROCEDURE sp_CreateUser ('u3', 'pw', 'udesc3')\nEXECUTE PROCEDURE sp_AddUserToGroup ('u3', 'g3')\nEXECUTE PROCEDURE sp_AddUserToGroup ('u3', 'g2')\nEXECUTE PROCEDURE sp_AddUserToGroup ('u2', 'DB:Admin')\nGRANT SELECT, INSERT ON t TO g3\nGRANT UPDATE ON t TO u3\nGRANT EXECUTE ON p1 TO g2\nGRANT SELECT (id) ON t TO u2"},
    {"x_dropped", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "CREATE TRIGGER trg1 ON t AFTER INSERT BEGIN INSERT INTO __error SELECT 1 FROM __new WHERE 1 = 0; END\nDROP TRIGGER t.trg1\n!group g1\nGRANT SELECT ON t TO g1\nREVOKE SELECT ON t FROM g1"},
    {"x_pubt", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "GRANT SELECT ON t TO \"DB:Public\""},
    {"x_pubv", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "CREATE VIEW v1 AS SELECT id FROM t\nGRANT SELECT ON v1 TO \"DB:Public\""},
    {"x_pubp", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "CREATE PROCEDURE p1 (a INTEGER) BEGIN DELETE FROM t WHERE id = a; END\nGRANT EXECUTE ON p1 TO \"DB:Public\""},
    {"x_pubf", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "CREATE FUNCTION f1 (a INTEGER) RETURNS INTEGER BEGIN RETURN a + 1; END\nGRANT EXECUTE ON f1 TO \"DB:Public\""},
    {"x_pubk", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "CREATE PACKAGE pk1\nGRANT EXECUTE ON pk1 TO \"DB:Public\""},
    {"x_publ", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "!link lnk\nGRANT ACCESS ON lnk TO \"DB:Public\""},
    {"x_pubb", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "!pub pub1\n!art pub1 t\nGRANT ALTER ON pub1 TO \"DB:Public\""},
    {"x_pubu", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "!user  amy\nGRANT ALTER ON amy TO \"DB:Public\""},
    {"x_pubg", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "!group g1\nGRANT ALTER ON g1 TO \"DB:Public\""},
    {"x_pubd", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "GRANT ALTER DATABASE TO \"DB:Public\""},
    {"x_pubm", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "CREATE VIEW v1 AS SELECT id FROM t\nCREATE PROCEDURE p1 (a INTEGER) BEGIN DELETE FROM t WHERE id = a; END\nCREATE FUNCTION f1 (a INTEGER) RETURNS INTEGER BEGIN RETURN a + 1; END\nGRANT SELECT, UPDATE ON v1 TO \"DB:Public\"\nGRANT EXECUTE, ALTER ON p1 TO \"DB:Public\"\nGRANT DROP ON f1 TO \"DB:Public\"\n!group g1\n!user g1 amy\nGRANT SELECT ON v1 TO amy"},
    {"x_pubt0", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "GRANT SELECT ON t TO \"DB:Public\"\nREVOKE SELECT ON t FROM \"DB:Public\""},
    {"x_pubv0", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "CREATE VIEW v1 AS SELECT id FROM t\nGRANT SELECT ON v1 TO \"DB:Public\"\nREVOKE SELECT ON v1 FROM \"DB:Public\""},
    {"x_pubt2", {T_INDEXED, {"u", "u.adt", ADS_ADT, ID_NAME, "u.adi", "bypid"}}, {T_INDEXED, {"u", "u.adt", ADS_ADT, ID_NAME, "u.adi", "bypid"}}, 1, 0, 0, "", "GRANT SELECT ON u TO \"DB:Public\""},
    {"x_pubtv", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "CREATE VIEW v1 AS SELECT id FROM t\nGRANT SELECT ON t TO \"DB:Public\"\nGRANT SELECT ON v1 TO \"DB:Public\""},
    {"x_pubtbig", {T_INDEXED, {"u", "u.adt", ADS_ADT, ID_NAME, "u.adi", "bypid"}}, {T_INDEXED, {"u", "u.adt", ADS_ADT, ID_NAME, "u.adi", "bypid"}}, 1, 0, 0, "", "!user  zed\n!group g2\n!group g1\n!user g2 mia\n!user g1 abe\n!user g1 bea\nCREATE PROCEDURE p2 (a INTEGER) BEGIN DELETE FROM t WHERE id = a; END\nCREATE PROCEDURE p1 (a INTEGER) BEGIN DELETE FROM u WHERE id = a; END\nCREATE VIEW v2 AS SELECT id FROM t\nCREATE VIEW v1 AS SELECT id FROM u\nCREATE FUNCTION f2 (a INTEGER) RETURNS INTEGER BEGIN RETURN a + 1; END\nCREATE FUNCTION f1 (a INTEGER) RETURNS INTEGER BEGIN RETURN a + 2; END\nCREATE PACKAGE pk1 DESCRIPTION 'a package'\nCREATE FUNCTION pk1.f3 (a INTEGER) RETURNS INTEGER BEGIN RETURN a + 3; END\nCREATE TRIGGER trg2 ON u AFTER INSERT BEGIN INSERT INTO __error SELECT 1 FROM __new WHERE 1 = 0; END\nCREATE TRIGGER trg1 ON t AFTER INSERT BEGIN INSERT INTO __error SELECT 1 FROM __new WHERE 1 = 0; END\nGRANT SELECT ON u TO g2\nGRANT SELECT ON t TO g1\nGRANT INSERT ON t TO abe\nGRANT SELECT ON t TO zed\nCREATE UNIQUE INDEX t_pk ON t (id)\nEXECUTE PROCEDURE sp_ModifyTableProperty ('t', 'TABLE_PRIMARY_KEY', 't_pk', 'NO_VALIDATE', NULL)\n!ri ri2 t t_pk u bypid\n!link l2\n!link l1\n!pub pb2\n!pub pb1\n!art pb1 t\n!art pb1 u\nGRANT SELECT ON v1 TO \"DB:Public\"\nGRANT EXECUTE ON p1 TO \"DB:Public\"\nGRANT SELECT ON t TO \"DB:Public\"\nGRANT SELECT ON u TO \"DB:Public\""},
    {"x_pubtdbset2", {{"t", "t.adt", ADS_ADT, ID_NAME, "t.adi", "byid"}}, {{"t", "t.adt", ADS_ADT, ID_NAME, "t.adi", "byid"}}, 1, 0, 0, "", "!dbn 103 1 2\n!dbn 104 1 2\n!dbn 107 1 2\n!dbn 108 1 2\n!dbn 109 9 2\n!dbn 113 1 2\n!dbn 122 1 2\n!dbn 125 1 2\n!dbn 132 1 2\n!dbn 131 1 4\n!dbn 111 4 2\n!dbn 112 5 2\nGRANT SELECT ON t TO \"DB:Public\""},
    {"x_pubtdbset", {{"t", "t.adt", ADS_ADT, ID_NAME, "t.adi", "byid"}}, {{"t", "t.adt", ADS_ADT, ID_NAME, "t.adi", "byid"}}, 1, 0, 0, "", "!dbn 107 1 2\n!dbn 108 1 2\n!dbn 109 9 2\n!dbn 113 1 2\n!dbn 122 1 2\n!dbn 125 1 2\n!dbn 132 1 2\n!dbn 131 1 4\n!dbn 111 4 2\n!dbn 112 5 2\n!dbn 103 0 2\n!dbn 104 0 2\nGRANT SELECT ON t TO \"DB:Public\""},
    {"x_pubtpw", {{"t", "t.adt", ADS_ADT, ID_NAME, "t.adi", "byid"}}, {{"t", "t.adt", ADS_ADT, ID_NAME, "t.adi", "byid"}}, 1, 0, 0, "wq", "GRANT SELECT ON t TO \"DB:Public\""},
    {"x_pubtri3c", {{"t", "t.adt", ADS_ADT, ID_NAME, "t.adi", "byid"}, {"u", "u.adt", ADS_ADT, ID_NAME, "u.adi", "bypid"}, {"w", "w.adt", ADS_ADT, ID_NAME, "w.adi", "bywid"}}, {{"t", "t.adt", ADS_ADT, ID_NAME, "t.adi", "byid"}, {"u", "u.adt", ADS_ADT, ID_NAME, "u.adi", "bypid"}, {"w", "w.adt", ADS_ADT, ID_NAME, "w.adi", "bywid"}}, 1, 0, 0, "", "CREATE UNIQUE INDEX u_pk ON u (id)\nEXECUTE PROCEDURE sp_ModifyTableProperty ('u', 'TABLE_PRIMARY_KEY', 'u_pk', 'NO_VALIDATE', NULL)\n!ri ri1 u u_pk w bywid\nGRANT SELECT ON t TO \"DB:Public\""},
    {"x_pubtplain", {{"t", "t.adt", ADS_ADT, ID_NAME, NULL, NULL}}, {{"t", "t.adt", ADS_ADT, ID_NAME, NULL, NULL}}, 1, 0, 0, "", "GRANT SELECT ON t TO \"DB:Public\""},
    {"x_pubtmemo", {{"t", "t.adt", ADS_ADT, ID_NAME ";m,Memo", NULL, NULL}}, {{"t", "t.adt", ADS_ADT, ID_NAME ";m,Memo", NULL, NULL}}, 1, 0, 0, "", "GRANT SELECT ON t TO \"DB:Public\""},
    {"x_pubtdbf", {{"c", "c.dbf", ADS_CDX, "id,Numeric,5,0;name,Character,10", "c.cdx", "byid"}}, {{"c", "c.dbf", ADS_CDX, "id,Numeric,5,0;name,Character,10", "c.cdx", "byid"}}, 1, 0, 0, "", "GRANT SELECT ON c TO \"DB:Public\""},
    {"x_pubtpk", {{"t", "t.adt", ADS_ADT, ID_NAME, "t.adi", "byid"}}, {{"t", "t.adt", ADS_ADT, ID_NAME, "t.adi", "byid"}}, 1, 0, 0, "", "CREATE UNIQUE INDEX t_pk ON t (id)\nEXECUTE PROCEDURE sp_ModifyTableProperty ('t', 'TABLE_PRIMARY_KEY', 't_pk', 'NO_VALIDATE', NULL)\nGRANT SELECT ON t TO \"DB:Public\""},
    {"x_pubtri3b", {{"t", "t.adt", ADS_ADT, ID_NAME, "t.adi", "byid"}, {"u", "u.adt", ADS_ADT, ID_NAME, "u.adi", "bypid"}, {"w", "w.adt", ADS_ADT, ID_NAME, "w.adi", "bywid"}}, {{"t", "t.adt", ADS_ADT, ID_NAME, "t.adi", "byid"}, {"u", "u.adt", ADS_ADT, ID_NAME, "u.adi", "bypid"}, {"w", "w.adt", ADS_ADT, ID_NAME, "w.adi", "bywid"}}, 1, 0, 0, "", "CREATE UNIQUE INDEX t_pk ON t (id)\nEXECUTE PROCEDURE sp_ModifyTableProperty ('t', 'TABLE_PRIMARY_KEY', 't_pk', 'NO_VALIDATE', NULL)\n!ri ri1 t t_pk u bypid\nGRANT SELECT ON u TO \"DB:Public\""},
    {"x_pubtri3", {{"t", "t.adt", ADS_ADT, ID_NAME, "t.adi", "byid"}, {"u", "u.adt", ADS_ADT, ID_NAME, "u.adi", "bypid"}, {"w", "w.adt", ADS_ADT, ID_NAME, "w.adi", "bywid"}}, {{"t", "t.adt", ADS_ADT, ID_NAME, "t.adi", "byid"}, {"u", "u.adt", ADS_ADT, ID_NAME, "u.adi", "bypid"}, {"w", "w.adt", ADS_ADT, ID_NAME, "w.adi", "bywid"}}, 1, 0, 0, "", "CREATE UNIQUE INDEX t_pk ON t (id)\nEXECUTE PROCEDURE sp_ModifyTableProperty ('t', 'TABLE_PRIMARY_KEY', 't_pk', 'NO_VALIDATE', NULL)\n!ri ri1 t t_pk u bypid\nGRANT SELECT ON w TO \"DB:Public\""},
    {"x_pubtgrp", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "!group g1\nGRANT SELECT ON t TO \"DB:Public\""},
    {"x_pubtuser", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "!user  zed\nGRANT SELECT ON t TO \"DB:Public\""},
    {"x_pubtpkg", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "CREATE PACKAGE pk1\nCREATE FUNCTION pk1.f3 (a INTEGER) RETURNS INTEGER BEGIN RETURN a + 3; END\nCREATE FUNCTION f1 (a INTEGER) RETURNS INTEGER BEGIN RETURN a + 2; END\nGRANT SELECT ON t TO \"DB:Public\""},
    {"x_pubtproc", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "CREATE PROCEDURE p1 (a INTEGER) BEGIN DELETE FROM t WHERE id = a; END\nGRANT SELECT ON t TO \"DB:Public\""},
    {"x_pubtlink", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "!link lnk\nGRANT SELECT ON t TO \"DB:Public\""},
    {"x_pubtri", {T_INDEXED, {"u", "u.adt", ADS_ADT, ID_NAME, "u.adi", "bypid"}}, {T_INDEXED, {"u", "u.adt", ADS_ADT, ID_NAME, "u.adi", "bypid"}}, 1, 0, 0, "", "CREATE UNIQUE INDEX t_pk ON t (id)\nEXECUTE PROCEDURE sp_ModifyTableProperty ('t', 'TABLE_PRIMARY_KEY', 't_pk', 'NO_VALIDATE', NULL)\n!ri ri1 t t_pk u bypid\nGRANT SELECT ON t TO \"DB:Public\""},
    {"x_pubtpub", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "!pub pb1\n!art pb1 t\nGRANT SELECT ON t TO \"DB:Public\""},
    {"x_pubttr", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "CREATE TRIGGER trg1 ON t AFTER INSERT BEGIN INSERT INTO __error SELECT 1 FROM __new WHERE 1 = 0; END\nGRANT SELECT ON t TO \"DB:Public\""},
    {"x_pubtg", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "!group g1\n!user g1 abe\nGRANT SELECT ON t TO g1\nGRANT INSERT ON t TO abe\nGRANT SELECT ON t TO \"DB:Public\""},
    {"sub_a", {{"t", "sub\\t.adt", ADS_ADT, ID_NAME, NULL, NULL}}, {{"t", "sub\\t.adt", ADS_ADT, ID_NAME, NULL, NULL}}, 1, 0, 0},
    {"sub_b", {{"t", ".\\sub\\t.adt", ADS_ADT, ID_NAME, NULL, NULL}}, {{"t", ".\\sub\\t.adt", ADS_ADT, ID_NAME, NULL, NULL}}, 1, 0, 0},
    {"sub_c", {{"t", "sub/t.adt", ADS_ADT, ID_NAME, NULL, NULL}}, {{"t", "sub/t.adt", ADS_ADT, ID_NAME, NULL, NULL}}, 1, 0, 0},
    {"subm_a", {{"t", "sub\\t.adt", ADS_ADT, ID_NAME, "sub\\t.adi", "byid"}}, {TNONE}, 1, 0, 0},
    {"subm_b", {{"s", "a\\b\\t.adt", ADS_ADT, ID_NAME, NULL, NULL}, T_INDEXED}, {T_INDEXED}, 1, 0, 0},
    {"x_dbset", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "!dbn 107 1 2\n!dbn 108 1 2\n!dbn 109 9 2\n!dbn 113 1 2\n!dbn 122 1 2\n!dbn 125 1 2\n!dbn 132 1 2\n!dbn 131 1 4\n!dbn 111 4 2\n!dbn 112 5 2\n!dbn 103 0 2\n!dbn 104 0 2"},
    {"x_db103a", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "!dbn 103 0 2"},
    {"x_db103b", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "!dbn 103 1 2"},
    {"x_db104a", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "!dbn 104 0 2"},
    {"x_db104b", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "!dbn 104 1 2"},
    {"x_db109", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "!dbn 109 9 2\n!dbn 108 1 2"},
    {"v_max", {T_PLAIN}, {T_PLAIN}, 1, 2, 0, "", "!fprop t id 303 2"},
    {"v_both", {T_PLAIN}, {T_PLAIN}, 1, 2, 0, "", "!fprop t id 302 2\n!fprop t id 303 2"},
    {"v_expr", {T_PLAIN}, {T_PLAIN}, 1, 2, 0, "", "!tprop t 200 id <> 2\n!tprop t 201 not two"},
    {"v_str", {T_PLAIN}, {T_PLAIN}, 1, 2, 0, "", "!fprop t name 302 p"},
    {"v_two", {T_PLAIN, {"u", "u.adt", ADS_ADT, ID_NAME, NULL, NULL}}, {T_PLAIN, {"u", "u.adt", ADS_ADT, ID_NAME, NULL, NULL}}, 1, 2, 0, "", "!fprop t id 302 2\n!fprop u id 303 1"},
    {"v_sub", {{"t", "sub\\t.adt", ADS_ADT, ID_NAME, NULL, NULL}}, {{"t", "sub\\t.adt", ADS_ADT, ID_NAME, NULL, NULL}}, 1, 2, 0, "", "!fprop t id 302 2"},
    {"v_none", {T_PLAIN}, {T_PLAIN}, 1, 2, 0, "", "!fprop t id 302 0"},
    {"v_users", {T_PLAIN}, {T_PLAIN}, 1, 2, 0, "", "!fprop t id 302 2\n!group g1\n!user g1 amy"},
    {"o1", {{"t", "nl.adt", ADS_ADT, ID_NAME, NULL, NULL}}, {{"t", "nl.adt", ADS_ADT, ID_NAME, NULL, NULL}}, 1, 2, 0, "", "!fprop t name 302 p\n!fprop t id 303 2"},
    {"o3", {{"t", "nl.adt", ADS_ADT, ID_NAME, NULL, NULL}}, {{"t", "nl.adt", ADS_ADT, ID_NAME, NULL, NULL}}, 1, 2, 0, "", "!fprop t id 302 2\n!tprop t 200 id <> 3"},
    {"o5", {{"t", "nl.adt", ADS_ADT, ID_NAME, NULL, NULL}}, {{"t", "nl.adt", ADS_ADT, ID_NAME, NULL, NULL}}, 1, 2, 0, "", "!fnull t name\n!fprop t id 302 2"},
    {"o8", {{"t", "nl.adt", ADS_ADT, ID_NAME, NULL, NULL}}, {{"t", "nl.adt", ADS_ADT, ID_NAME, NULL, NULL}}, 1, 2, 0, "", "!fnull t name\n!fprop t name 302 p"},
    {"o10", {{"t", "nl.adt", ADS_ADT, ID_NAME, NULL, NULL}}, {{"t", "nl.adt", ADS_ADT, ID_NAME, NULL, NULL}}, 1, 2, 0, "", "!fprop t name 303 p\n!fnull t name"},
    {"x_sub", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "!pub pub1\n!art pub1 t\n!sub sub1 pub1"},
    {"x_sub2", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "!pub pub1\n!pub pub2\n!sub sa pub1 qa.add a~comment\n!sub sb pub2 qb.adt\n!sub sc pub1 qc"},
    {"x_subdir", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "!pub pub1\n!sub sa pub1 sub\\qa.adt"},
    {"x_subpub", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "!pub pub1\n!sub sa pub1\nGRANT SELECT ON t TO \"DB:Public\""},
    {"x_dbcpub", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "EXECUTE PROCEDURE sp_ModifyDatabase ('COMMENT', 'db comment')\nGRANT SELECT ON t TO \"DB:Public\""},
    {"x_dbcom", {T_INDEXED}, {T_INDEXED}, 1, 0, 0, "", "EXECUTE PROCEDURE sp_ModifyDatabase ('COMMENT', 'db comment')"},
    {"w_msg", {T_PLAIN}, {T_PLAIN}, 1, 0, 0, "", "!fprop t id 304 too low"},
    {"w_min", {T_PLAIN}, {T_PLAIN}, 1, 0, 0, "", "!fprop t id 302 2\n!fprop t id 304 too low"},
    {"w_max", {T_PLAIN}, {T_PLAIN}, 1, 0, 0, "", "!fprop t id 303 9\n!fprop t id 304 too high"},
    {"w_def", {T_PLAIN}, {T_PLAIN}, 1, 0, 0, "", "!fprop t name 300 'x'\n!fprop t name 304 bad name"},
    {"w_null", {T_PLAIN}, {T_PLAIN}, 1, 0, 0, "", "!fnull t id\n!fprop t id 304 needed"},
    {"w_all", {T_PLAIN}, {T_PLAIN}, 1, 0, 0, "", "!fprop t id 300 5\n!fprop t id 302 2\n!fprop t id 303 9\n!fprop t id 304 out of range"},
    {"d_cdx", {{"c", "c.dbf", ADS_CDX, "id,Numeric,5,0;name,Character,10", "c.cdx", "byid"}}, {{"c", "c.dbf", ADS_CDX, "id,Numeric,5,0;name,Character,10", "c.cdx", "byid"}}, 1, 2, 0, "", "!fprop c id 302 2"},
    {"d_cdxmax", {{"c", "c.dbf", ADS_CDX, "id,Numeric,5,0;name,Character,10", "c.cdx", "byid"}}, {{"c", "c.dbf", ADS_CDX, "id,Numeric,5,0;name,Character,10", "c.cdx", "byid"}}, 1, 2, 0, "", "!fprop c id 303 2"},
    {"d_ntx", {{"n", "n.dbf", ADS_NTX, "id,Numeric,5,0;name,Character,10", "n_id.ntx", NULL}}, {{"n", "n.dbf", ADS_NTX, "id,Numeric,5,0;name,Character,10", "n_id.ntx", NULL}}, 1, 2, 0, "", "!fprop n id 302 2"},
    {"d_vfp", {{"v", "v.dbf", ADS_VFP, "id,Numeric,5,0;name,Character,10", "v.cdx", "byid"}}, {{"v", "v.dbf", ADS_VFP, "id,Numeric,5,0;name,Character,10", "v.cdx", "byid"}}, 1, 2, 0, "", "!fprop v id 302 2"},
    {"d_cdxplain", {{"c", "c.dbf", ADS_CDX, "id,Numeric,5,0;name,Character,10", NULL, NULL}}, {{"c", "c.dbf", ADS_CDX, "id,Numeric,5,0;name,Character,10", NULL, NULL}}, 1, 2, 0, "", "!fprop c id 302 2"},
    {"d_memo", {{"t", "t.adt", ADS_ADT, ID_NAME ";m,Memo", "t.adi", "byid"}}, {{"t", "t.adt", ADS_ADT, ID_NAME ";m,Memo", "t.adi", "byid"}}, 1, 2, 0, "", "!fprop t id 302 2"},
    {"d_dbfmemo", {{"c", "c.dbf", ADS_CDX, "id,Numeric,5,0;name,Character,10;notes,Memo", "c.cdx", "byid"}}, {{"c", "c.dbf", ADS_CDX, "id,Numeric,5,0;name,Character,10;notes,Memo", "c.cdx", "byid"}}, 1, 2, 0, "", "!fprop c id 302 2"},
    {"d_memov", {{"t", "mv.adt", ADS_ADT, ID_NAME ";m,Memo", "mv.adi", "byid"}}, {{"t", "mv.adt", ADS_ADT, ID_NAME ";m,Memo", "mv.adi", "byid"}}, 1, 2, 0, "", "!fprop t id 302 2"},
    {"d_vfpmemov", {{"c", "mv.dbf", ADS_VFP, "id,Numeric,5,0;name,Character,10;notes,Memo", "mv.cdx", "byid"}}, {{"c", "mv.dbf", ADS_VFP, "id,Numeric,5,0;name,Character,10;notes,Memo", "mv.cdx", "byid"}}, 1, 2, 0, "", "!fprop c id 302 2"},
    {"d_vfp2", {{"v", "v.dbf", ADS_VFP, "id,Numeric,5,0;name,Character,10", "v.cdx", "byid"}}, {{"v", "v.dbf", ADS_VFP, "id,Numeric,5,0;name,Character,10", "v.cdx", "byid"}}, 1, 2, 0, "", "!fprop v id 302 2\n!fprop v name 302 two"},
    {"d_cdxfail", {{"c", "c.dbf", ADS_CDX, "id,Numeric,5,0;name,Character,10", "c.cdx", "byid"}}, {{"c", "c.dbf", ADS_CDX, "id,Numeric,5,0;name,Character,10", "c.cdx", "byid"}}, 1, 2, 0, "f", "!fprop c id 302 2"},
    {"w_dmin", {T_PLAIN}, {T_PLAIN}, 1, 0, 0, "", "!fprop t id 300 5\n!fprop t id 302 2\n!fprop t id 304 msg"},
    {"w_dmax", {T_PLAIN}, {T_PLAIN}, 1, 0, 0, "", "!fprop t id 300 5\n!fprop t id 303 9\n!fprop t id 304 msg"},
    {"w_mm", {T_PLAIN}, {T_PLAIN}, 1, 0, 0, "", "!fprop t id 302 2\n!fprop t id 303 9\n!fprop t id 304 msg"},
    {"w_dmm", {T_PLAIN}, {T_PLAIN}, 1, 0, 0, "", "!fprop t id 300 5\n!fprop t id 302 2\n!fprop t id 303 9"},
    {"w_allc", {T_PLAIN}, {T_PLAIN}, 1, 0, 0, "", "!fprop t id 1 cmt\n!fprop t id 300 5\n!fprop t id 302 2\n!fprop t id 303 9\n!fprop t id 304 msg"},
    {"w_alln", {T_PLAIN}, {T_PLAIN}, 1, 0, 0, "", "!fnull t id\n!fprop t id 300 5\n!fprop t id 302 2\n!fprop t id 303 9\n!fprop t id 304 msg"},
    {"w_allname", {T_PLAIN}, {T_PLAIN}, 1, 0, 0, "", "!fprop t name 300 'x'\n!fprop t name 302 'a'\n!fprop t name 303 'z'\n!fprop t name 304 msg"},
    {"w_allboth", {T_PLAIN}, {T_PLAIN}, 1, 0, 0, "", "!fprop t id 300 5\n!fprop t id 302 2\n!fprop t id 303 9\n!fprop t id 304 msg\n!fprop t name 300 'x'\n!fprop t name 302 'a'\n!fprop t name 303 'z'\n!fprop t name 304 msg"},
    {"w_two", {T_PLAIN}, {T_PLAIN}, 1, 0, 0, "", "!fprop t id 304 msg one\n!fprop t name 300 'x'\n!fprop t name 302 'a'"},
    {"w_cmt", {T_PLAIN}, {T_PLAIN}, 1, 0, 0, "", "!fprop t id 1 a comment\n!fprop t id 304 msg"},
    {"w_tbl", {T_PLAIN}, {T_PLAIN}, 1, 0, 0, "", "!tprop t 200 id <> 2\n!tprop t 201 table msg"},
    {"src_pw_wrong", {T_PLAIN}, {T_PLAIN}, 1, 0, 0, "q"},
};
#define N ((int)(sizeof scenarios / sizeof scenarios[0]))

/* Calls the function and prints one RESULT line: the scenario, the folders it
 * was given (relative to the scenario's), the options, and the result code
 * and the text of the error. */
static void deploy(const Scenario *sc) {
    char s[MAX_PATH], d[MAX_PATH], add[MAX_PATH], dest_rel[64], src_rel[64];
    folders(sc->name, s, d);
    int in_d = has(sc, 's') || has(sc, 'k');
    snprintf(src_rel, sizeof src_rel, "%s/%s.add", in_d ? "D" : "S", has(sc, 'm') ? "nodict" : "newdb");
    snprintf(add, sizeof add, "%s\\%s.add", in_d ? d : s, has(sc, 'm') ? "nodict" : "newdb");
    snprintf(dest_rel, sizeof dest_rel, "%s", has(sc, 'n') ? "D/nodir" : "D");
    if (has(sc, 'n')) snprintf(d, sizeof d, "%s\\nodir", d);
    U32 rc = ((U32(WINAPI *)(U8 *, U8 *, U8 *, U8 *, U16, U16, U16, U32))F(
        "AdsDDDeployDatabase"))((U8 *)d, has(sc, 'p') ? (U8 *)"dpw" : NULL, (U8 *)add,
                                has(sc, 'q') ? (U8 *)"secret" : NULL, sc->types, sc->validate,
                                sc->backup, 0);
    U8 text[1024] = "";
    U16 len = sizeof text;
    U32 code = 0;
    if (rc) ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
    printf("RESULT|%s|%s|%s|%s|%u|%u|%u|%lu|%s\n", sc->name, dest_rel, src_rel,
           has(sc, 'q') ? "secret" : "", sc->types, sc->validate, sc->backup, rc, text);
}

/* `fields`: after the deployment, what the new dictionary in the destination says of
 * its tables' fields (the default, can-NULL, minimum, maximum, message and comment
 * properties), one FIELD line each, and of its subscriptions' queues. */
static void dump_fields(const Scenario *sc, int source) {
    char s[MAX_PATH], d[MAX_PATH], path[MAX_PATH];
    folders(sc->name, s, d);
    snprintf(path, sizeof path, "%s\\newdb.add", source ? s : d);
    H c = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
        (U8 *)path, 1, (U8 *)"ADSSYS", (U8 *)"", 0, &c);
    if (rc) { printf("FIELD|%s|connect %lu\n", sc->name, rc); return; }
    for (int t = 0; t < 4 && sc->src[t].name; t++) {
        U8 name[300];
        U16 len = sizeof name;
        H find = 0;
        rc = ((U32(WINAPI *)(H, U16, U8 *, U8 *, U16 *, H *))F("AdsDDFindFirstObject"))(
            c, 4, (U8 *)sc->src[t].name, name, &len, &find);
        while (!rc) {
            name[len] = 0;
            printf("FIELD|%s|%s|%s", sc->name, sc->src[t].name, name);
            const unsigned ids[] = {1, 300, 301, 302, 303, 304};
            for (int i = 0; i < 6; i++) {
                U8 value[300] = "";
                U16 vlen = sizeof value;
                U32 r = ((U32(WINAPI *)(H, U8 *, U8 *, U16, void *, U16 *))F(
                    "AdsDDGetFieldProperty"))(c, (U8 *)sc->src[t].name, name, ids[i], value, &vlen);
                if (r) printf("|%u=E%lu", ids[i], r);
                else if (ids[i] == 301) printf("|%u=%u", ids[i], value[0] | value[1] << 8);
                else { value[vlen < 299 ? vlen : 299] = 0; printf("|%u=%s", ids[i], value); }
            }
            printf("\n");
            len = sizeof name;
            rc = ((U32(WINAPI *)(H, H, U8 *, U16 *))F("AdsDDFindNextObject"))(c, find, name, &len);
        }
        if (find) ((U32(WINAPI *)(H, H))F("AdsDDFindClose"))(c, find);
    }
    disconnect(c);
}

int main(int argc, char **argv) {
    if (argc < 4) return 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) { fprintf(stderr, "cannot load %s\n", argv[1]); return 1; }
    setvbuf(stdout, NULL, _IONBF, 0);
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    snprintf(root, sizeof root, "%s", argv[2]);
    for (int i = 0; i < N; i++) {
        if (argc > 4 && strncmp(argv[4], scenarios[i].name, strlen(argv[4]))) continue;
        if (!strcmp(argv[3], "prepare")) {
            prepare(&scenarios[i]);
        } else if (!strcmp(argv[3], "fields") || !strcmp(argv[3], "sfields")) {
            dump_fields(&scenarios[i], argv[3][0] == 's');
        } else {
            deploy(&scenarios[i]);
        }
    }
    return 0;
}
