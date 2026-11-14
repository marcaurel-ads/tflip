// Linked into the build against the original only (run.sh --sap,
// -Wl,--wrap=AdsConnect60): after a connect succeeds, the connection's folder
// (a dictionary's own folder) becomes the current directory.
//
// The original resolves a relative table or index name against the current
// directory, not the connection's folder (fixtures/errors/index_files.json),
// and OpenADS's tests name their tables relatively ("data") while making
// them in a temporary folder of their own. Without this, the original opens
// whatever file of that name another test left in the shared working folder:
// abi_custom_key_test's "data" was another test's table with only a field
// ID, so its order on TAG failed with 3011. The tests connect with AdsConnect60
// only; with two connections, the later one's folder wins.
#include <windows.h>

#include <cstdint>
#include <cstring>

typedef uint64_t H;
typedef uint32_t U32;
typedef uint16_t U16;
typedef uint8_t U8;

extern "C" {

U32 __real_AdsConnect60(U8 *, U16, U8 *, U8 *, U32, H *);
U32 __wrap_AdsConnect60(U8 *path, U16 server, U8 *user, U8 *password, U32 options,
                        H *conn) {
    U32 rc = __real_AdsConnect60(path, server, user, password, options, conn);
    if (rc != 0 || path == nullptr) return rc;
    char dir[MAX_PATH];
    std::strncpy(dir, reinterpret_cast<const char *>(path), sizeof dir - 1);
    dir[sizeof dir - 1] = 0;
    std::size_t len = std::strlen(dir);
    if (len > 4 && _stricmp(dir + len - 4, ".add") == 0) {
        char *slash = std::strrchr(dir, '\\');
        char *other = std::strrchr(dir, '/');
        if (other > slash) slash = other;
        if (slash == nullptr) return rc;
        *slash = 0;
    }
    SetCurrentDirectoryA(dir);
    return rc;
}
}
