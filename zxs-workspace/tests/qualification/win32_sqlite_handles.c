/*
 * Opt-in read-only qualification fixture, not part of any package/build/test target.
 * A tester separately builds and verifies THIS standalone x64 Windows executable
 * with the official SQLite 3.54.1 amalgamation statically linked (omit loadable
 * extensions). No helper DLL, SQL, Connection, pager, DB creation or recovery.
 * Run only with: --synthetic-dir D:\controlled\zipws-handle-fixture
 * Use a quiescent disposable fixed local NTFS folder, never a live state root.
 * The operator supplies two distinct single-link files, witness.sqlite and
 * witness.sqlite-journal, each containing ASCII "zipws synthetic handle witness"
 * followed by one LF. The fixture never writes or removes anything; OS read
 * atime may change. Inability to meet a prerequisite is not a passing result.
 */
#if !defined(_WIN64)
#error This fixture requires 64-bit Windows.
#endif
#ifndef _WIN32_WINNT
#define _WIN32_WINNT 0x0A00
#endif
#define SQLITE_OMIT_LOAD_EXTENSION 1
#include <sqlite3.c>  /* supplied independently by the tester, not vendored or found at runtime */
#include <windows.h>
#include <winternl.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <wchar.h>
#if SQLITE_VERSION_NUMBER != 3054001
#error Review native VFS behavior before qualifying a different SQLite release.
#endif

#define CAP 2048
#define DEPTH 32
static const WCHAR *const names[] = {L"witness.sqlite", L"witness.sqlite-journal"};
static const char message[] = "zipws synthetic handle witness\n";

typedef NTSTATUS (NTAPI *CreateNt)(PHANDLE, ACCESS_MASK, POBJECT_ATTRIBUTES, PIO_STATUS_BLOCK,
    PLARGE_INTEGER, ULONG, ULONG, ULONG, ULONG, PVOID, ULONG);
typedef NTSTATUS (NTAPI *VolumeNt)(HANDLE, PIO_STATUS_BLOCK, PVOID, ULONG, ULONG);
typedef NTSTATUS (NTAPI *ListNt)(HANDLE, HANDLE, PIO_APC_ROUTINE, PVOID, PIO_STATUS_BLOCK,
    PVOID, ULONG, ULONG, BOOLEAN, PUNICODE_STRING, BOOLEAN);
static CreateNt op; static VolumeNt vol; static ListNt list;

/* Every child lookup originates from its held parent. FILE_OPEN only, no
   reparse or data recall; don't grant DELETE sharing on our test witnesses. */
static HANDLE existing(HANDLE parent, WCHAR *name, int dir) {
    UNICODE_STRING s;
    OBJECT_ATTRIBUTES oa = {0}; IO_STATUS_BLOCK io = {0};
    HANDLE h = NULL;
    size_t n = wcslen(name);
    if (!n || n >= CAP) return NULL;
    s.Length = (USHORT)(n * sizeof(WCHAR)); s.MaximumLength = s.Length; s.Buffer = name;
    oa.Length = sizeof(oa); oa.RootDirectory = parent; oa.ObjectName = &s;
    ACCESS_MASK access = SYNCHRONIZE | FILE_READ_ATTRIBUTES | (dir ? FILE_LIST_DIRECTORY | FILE_TRAVERSE : FILE_READ_DATA);
    NTSTATUS status = op(&h, access, &oa, &io, NULL, 0, dir ? FILE_SHARE_READ|FILE_SHARE_WRITE : FILE_SHARE_READ,
                         1, 0x00200000|0x00400000|0x20|(dir ? 0 : 0x40), NULL, 0);
    if (status != 0 || io.Status != 0 || io.Information != 1 || !h) {
        if (h && h != INVALID_HANDLE_VALUE) CloseHandle(h);
        return NULL;
    }
    return h;
}

static int identity(HANDLE h, int directory, FILE_ID_INFO *id) {
    FILE_STANDARD_INFO st; FILE_BASIC_INFO b;
    static const unsigned char zero[16] = {0};
    if (!h || GetFileType(h) != FILE_TYPE_DISK ||
        !GetFileInformationByHandleEx(h, FileStandardInfo, &st, sizeof(st)) ||
        st.DeletePending || !st.NumberOfLinks || !!st.Directory != directory ||
        !GetFileInformationByHandleEx(h, FileBasicInfo, &b, sizeof(b)) ||
        !b.FileAttributes || (b.FileAttributes & (0x40|0x400|0x1000|0x40000|0x400000)) ||
        !!(b.FileAttributes & FILE_ATTRIBUTE_DIRECTORY) != directory ||
        !GetFileInformationByHandleEx(h, FileIdInfo, id, sizeof(*id)) ||
        !id->VolumeSerialNumber || !memcmp(id->FileId.Identifier, zero, 16)) return 0;
    return directory || (st.NumberOfLinks == 1 && st.EndOfFile.QuadPart == sizeof(message)-1 &&
            !(b.FileAttributes & ~(FILE_ATTRIBUTE_NORMAL | FILE_ATTRIBUTE_ARCHIVE | FILE_ATTRIBUTE_NOT_CONTENT_INDEXED)));
}

static int equal_id(const FILE_ID_INFO *a, const FILE_ID_INFO *b) {
    return a->VolumeSerialNumber == b->VolumeSerialNumber && !memcmp(a->FileId.Identifier, b->FileId.Identifier, 16);
}
static int exact_name(HANDLE h, const WCHAR *expected) {
    WCHAR got[CAP];
    DWORD n = GetFinalPathNameByHandleW(h, got, CAP, VOLUME_NAME_GUID);
    return n && n < CAP && !wcscmp(got, expected);
}
static int component(const WCHAR *s, size_t n) {
    if (!n || n > 120 || s[n-1] == L' ' || s[n-1] == L'.') return 0;
    for (size_t i=0; i<n; ++i) {
        WCHAR c = s[i]; /* Deliberately narrow ASCII fixture vocabulary, not a product filename policy. */
        if (!((c >= L'A' && c <= L'Z') || (c >= L'a' && c <= L'z') || (c >= L'0' && c <= L'9') ||
              c == L'_' || c == L'-' || c == L'.' || c == L' ')) return 0;
    }
    return 1;
}
static int fixed_ntfs(HANDLE root, WCHAR *guid) {
    struct { ULONG kind, flags; } device = {0}; IO_STATUS_BLOCK io = {0}; WCHAR fs[32];
    DWORD len = GetFinalPathNameByHandleW(root, guid, CAP, VOLUME_NAME_GUID);
    if (len != 49 || wcsncmp(guid, L"\\\\?\\Volume{", 11) || wcscmp(guid+47, L"}\\")) return 0;
    for (size_t i=0;i<36;i++) {
        WCHAR c=guid[11+i];
        if (i==8 || i==13 || i==18 || i==23) { if (c!=L'-') return 0; }
        else if (!((c>=L'0'&&c<=L'9')||(c>=L'a'&&c<=L'f')||(c>=L'A'&&c<=L'F'))) return 0;
    }
    return GetDriveTypeW(guid)==DRIVE_FIXED && !vol(root,&io,&device,sizeof(device),4) && !io.Status &&
           io.Information==sizeof(device) && device.kind==7 && !(device.flags & 0x1055) &&
           GetVolumeInformationByHandleW(root,NULL,0,NULL,NULL,NULL,fs,32) && !wcscmp(fs,L"NTFS");
}
/* FileNamesInformation, one record per call. Reject the first unexpected or
   duplicate name. A dot-only response is not end-of-directory. */
static int exactly_two(HANDLE dir) {
    unsigned seen = 0; BOOLEAN first = TRUE;
    for (;;) {
        struct {ULONG next, index, bytes; WCHAR name[256];} row = {0}; IO_STATUS_BLOCK io = {0};
        NTSTATUS s = list(dir,NULL,NULL,NULL,&io,&row,sizeof(row),12,TRUE,NULL,first);
        if ((ULONG)s == 0x80000006UL) return (seen & 3u) == 3u; /* STATUS_NO_MORE_FILES */
        if (s || io.Status || row.next || row.bytes%2 || row.bytes>sizeof(row.name) ||
            io.Information < 12u+row.bytes || io.Information>sizeof(row)) return 0;
        const WCHAR *allowed[] = {names[0], names[1], L".", L".."};
        unsigned bit = 0;
        for (unsigned i=0;i<4;i++) {
            if (wcslen(allowed[i])*sizeof(WCHAR) == row.bytes && !memcmp(allowed[i],row.name,row.bytes)) bit=1u<<i;
        }
        if (!bit || (seen & bit)) return 0;
        seen |= bit; first = FALSE;
    }
}
static int content(HANDLE h) {
    char data[sizeof(message)]; DWORD n = 0; char extra;
    return ReadFile(h,data,sizeof(message)-1,&n,NULL) && n==sizeof(message)-1 && !memcmp(data,message,n) &&
           ReadFile(h,&extra,1,&n,NULL) && n==0;
}

/* Query one true native main/journal handle once, while BOTH independently
   opened originals remain live. Do not close the borrowed Windows HANDLE. */
static int observe(sqlite3_vfs *v, sqlite3_filename bundle, int role, const WCHAR *path, const FILE_ID_INFO ids[2]) {
    sqlite3_file *f = sqlite3_malloc64((sqlite3_uint64)v->szOsFile);
    if (!f) return 0;
    memset(f,0,(size_t)v->szOsFile);
    int flags = SQLITE_OPEN_READONLY | (role ? SQLITE_OPEN_MAIN_JOURNAL : SQLITE_OPEN_MAIN_DB);
    const char *name = role ? sqlite3_filename_journal(bundle) : sqlite3_filename_database(bundle);
    int returned = 0, ok = 0;
    int rc = v->xOpen(v,name,f,flags,&returned);
    if (rc == SQLITE_OK && returned==SQLITE_OPEN_READONLY && f->pMethods && f->pMethods->xFileControl) {
        HANDLE borrowed = NULL; FILE_ID_INFO actual;
        ok = f->pMethods->xFileControl(f,SQLITE_FCNTL_WIN32_GET_HANDLE,&borrowed)==SQLITE_OK &&
             borrowed && borrowed!=INVALID_HANDLE_VALUE && identity(borrowed,0,&actual) &&
             exact_name(borrowed,path) && equal_id(&actual,&ids[role]) && !equal_id(&actual,&ids[1-role]);
    }
    if (f->pMethods && f->pMethods->xClose(f)!=SQLITE_OK) ok=0;
    sqlite3_free(f);
    return ok;
}

int wmain(int argc, wchar_t **argv) {
    if (argc!=3 || wcscmp(argv[1],L"--synthetic-dir")) {
        fputs("Requires --synthetic-dir and an existing dedicated local fixture.\n",stderr); return 2;
    }
    WCHAR *input=argv[2]; size_t n=wcslen(input);
    if (n<4 || n>=CAP || input[0]<L'A' || input[0]>L'Z' || input[1]!=L':' || input[2]!=L'\\') return 2;
    HMODULE kernel=GetModuleHandleW(L"ntdll.dll"); /* already loaded system DLL, never a test helper */
    if (!kernel) return 2;
    op=(CreateNt)GetProcAddress(kernel,"NtCreateFile"); vol=(VolumeNt)GetProcAddress(kernel,"NtQueryVolumeInformationFile");
    list=(ListNt)GetProcAddress(kernel,"NtQueryDirectoryFile");
    if (!op || !vol || !list) return 2;
    HANDLE chain[DEPTH+1]={0}, files[2]={0}; int depth=0, ok=0;
    WCHAR base[CAP], part[121], paths[2][CAP], ntroot[]=L"\\??\\A:\\", drive[]=L"A:\\";
    char encoded[3][CAP*4]; sqlite3_filename bundle=NULL; FILE_ID_INFO rootid, ids[2];
    const WCHAR *p=input+3; ntroot[4]=drive[0]=input[0];
    if (GetDriveTypeW(drive)!=DRIVE_FIXED || !(chain[0]=existing(NULL,ntroot,1)) ||
        !identity(chain[0],1,&rootid) || !fixed_ntfs(chain[0],base)) goto end;
    for (;;) {
        const WCHAR *sep=wcschr(p,L'\\'); size_t k=sep ? (size_t)(sep-p) : wcslen(p);
        if (!component(p,k) || depth==DEPTH || wcslen(base)+k+2>=CAP) goto end;
        memcpy(part,p,k*sizeof(WCHAR)); part[k]=0;
        chain[depth+1]=existing(chain[depth],part,1); ++depth;
        FILE_ID_INFO d;
        if (!identity(chain[depth],1,&d) || d.VolumeSerialNumber!=rootid.VolumeSerialNumber) goto end;
        if (depth>1) wcscat_s(base,CAP,L"\\");
        wcscat_s(base,CAP,part);
        if (!exact_name(chain[depth],base)) goto end;
        if (!sep) break;
        p=sep+1;
    }
    if (wcscmp(part,L"zipws-handle-fixture") || wcslen(base)+40>=CAP || !exactly_two(chain[depth])) goto end;
    for (int i=0;i<2;i++) {
        WCHAR leaf[32]; wcscpy_s(leaf,32,names[i]);
        files[i]=existing(chain[depth],leaf,0);
        swprintf_s(paths[i],CAP,L"%ls\\%ls",base,leaf);
        if (!identity(files[i],0,&ids[i]) || ids[i].VolumeSerialNumber!=rootid.VolumeSerialNumber ||
            !exact_name(files[i],paths[i]) || !content(files[i]) ||
            !WideCharToMultiByte(CP_UTF8,WC_ERR_INVALID_CHARS,paths[i],-1,encoded[i],CAP*4,NULL,NULL)) goto end;
    }
    if (equal_id(&ids[0],&ids[1])) goto end;
    snprintf(encoded[2],sizeof(encoded[2]),"%s-wal",encoded[0]); /* Bundle metadata only, never opened. */
    bundle=sqlite3_create_filename(encoded[0],encoded[1],encoded[2],0,NULL);
    sqlite3_vfs *v=sqlite3_vfs_find("win32");
    if (!v || !v->xOpen || v->szOsFile<(int)sizeof(sqlite3_file) || v->szOsFile>65536 || !bundle) goto end;
    for (int i=0;i<2;i++) {
        if (!exact_name(chain[depth],base) || !observe(v,bundle,i,paths[i],ids) || !exact_name(files[i],paths[i])) goto end;
    }
    ok=exactly_two(chain[depth]) && exact_name(chain[depth],base);
end:
    sqlite3_free_filename(bundle);
    for (int i=0;i<2;i++) if (files[i] && !CloseHandle(files[i])) ok=0;
    for (int i=depth;i>=0;i--) if (chain[i] && !CloseHandle(chain[i])) ok=0;
    if (!ok) { fputs("No native handle qualification result.\n",stderr); return 1; }
    puts("Observed both exact existing Win32 VFS handles; each differed from the other same-byte file.");
    return 0;
}
