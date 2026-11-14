// Linked into the OpenADS tests (run.sh, -Wl,--wrap=<name> for each
// function below): where a test passes a literal 0 for an option that
// ace.h defines no 0 for, the wrapper passes ace.h's value with the meaning
// OpenADS gives 0 (its ace.h and its src/abi/ace_exports.cpp). The original
// rejects these zeros (5013 a filter option, 5019 a seek or resolve option,
// 5027 a character type, 5029 a rights option), so without the wrappers a
// test stops at its setup there, and the run against the original says
// nothing about the checks after it. The same wrappers go into both builds.
#include <cstdint>

typedef uint64_t H;
typedef uint32_t U32;
typedef uint16_t U16;
typedef uint8_t U8;

static U16 or_(U16 v, U16 dflt) { return v ? v : dflt; }
enum : U16 {
    RESPECTFILTERS = 1,
    ANSI = 1,
    CHECKRIGHTS = 1,
    RAWKEY = 1,
    HARDSEEK = 2,
    RESOLVE_DYNAMIC = 2,
};

extern "C" {

U32 __real_AdsCreateTable(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *);
U32 __wrap_AdsCreateTable(H conn, U8 *name, U8 *alias, U16 type, U16 chars, U16 lock,
                          U16 rights, U16 memo, U8 *fields, H *table) {
    return __real_AdsCreateTable(conn, name, alias, type, or_(chars, ANSI), lock,
                                 or_(rights, CHECKRIGHTS), memo, fields, table);
}

U32 __real_AdsOpenTable(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *);
U32 __wrap_AdsOpenTable(H conn, U8 *name, U8 *alias, U16 type, U16 chars, U16 lock,
                        U16 rights, U32 options, H *table) {
    return __real_AdsOpenTable(conn, name, alias, type, or_(chars, ANSI), lock,
                               or_(rights, CHECKRIGHTS), options, table);
}

U32 __real_AdsSeek(H, U8 *, U16, U16, U16, U16 *);
U32 __wrap_AdsSeek(H index, U8 *key, U16 len, U16 data_type, U16 seek_type, U16 *found) {
    return __real_AdsSeek(index, key, len, or_(data_type, RAWKEY), or_(seek_type, HARDSEEK),
                          found);
}

U32 __real_AdsSeekLast(H, U8 *, U16, U16, U16 *);
U32 __wrap_AdsSeekLast(H index, U8 *key, U16 len, U16 data_type, U16 *found) {
    return __real_AdsSeekLast(index, key, len, or_(data_type, RAWKEY), found);
}

U32 __real_AdsSetScope(H, U16, U8 *, U16, U16);
U32 __wrap_AdsSetScope(H index, U16 option, U8 *key, U16 len, U16 data_type) {
    return __real_AdsSetScope(index, option, key, len, or_(data_type, RAWKEY));
}

U32 __real_AdsSetAOF(H, U8 *, U16);
U32 __wrap_AdsSetAOF(H table, U8 *expr, U16 options) {
    return __real_AdsSetAOF(table, expr, or_(options, RESOLVE_DYNAMIC));
}

#define FILTER_OPTION(name)                                        \
    U32 __real_##name(H, U16, U32 *);                              \
    U32 __wrap_##name(H handle, U16 option, U32 *n) {              \
        return __real_##name(handle, or_(option, RESPECTFILTERS), n); \
    }
FILTER_OPTION(AdsGetRecordCount)
FILTER_OPTION(AdsGetKeyCount)
FILTER_OPTION(AdsGetRecordNum)
FILTER_OPTION(AdsGetKeyNum)

}
