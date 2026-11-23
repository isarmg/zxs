/* Internal policy shared by separate Windows-only adapters. No default or permissive ACL. */
#pragma once
#include <winfsp/winfsp.h>
#include <stdint.h>
#include <string.h>
#pragma comment(lib, "Advapi32.lib")

typedef union { ULONG_PTR align; BYTE bytes[256]; } AlignedBuffer;
typedef struct {
    AlignedBuffer sid, file, directory;
    DWORD file_bytes, directory_bytes;
} SessionPolicy;

static PSID bounded_sid(const AlignedBuffer *data, DWORD length, PSID sid)
{
    uintptr_t lo = (uintptr_t)data->bytes, at = (uintptr_t)sid;
    if (length > sizeof data->bytes || at < lo || at-lo > length || length-(at-lo) < 8)
        return NULL;
    BYTE count = *GetSidSubAuthorityCount(sid);
    if (count > SID_MAX_SUB_AUTHORITIES || GetSidLengthRequired(count) > length-(at-lo))
        return NULL;
    return IsValidSid(sid) ? sid : NULL;
}

static BOOL token_flag(HANDLE token, TOKEN_INFORMATION_CLASS kind, DWORD wanted, BOOL nonzero)
{
    DWORD value = 0, n = 0;
    return GetTokenInformation(token, kind, &value, sizeof value, &n) && n == sizeof value &&
        (nonzero ? value != 0 : value == wanted);
}

static PSID local_logon(HANDLE token, AlignedBuffer *logon)
{
    AlignedBuffer label;
    DWORD n = 0;
    if (!token || !token_flag(token, TokenSessionId, 0, TRUE) ||
        !token_flag(token, TokenElevation, 0, FALSE) || !token_flag(token, TokenUIAccess, 0, FALSE) ||
        !token_flag(token, TokenIsAppContainer, 0, FALSE) || IsTokenRestricted(token) ||
        !GetTokenInformation(token, TokenIntegrityLevel, label.bytes, sizeof label.bytes, &n) ||
        n < sizeof(TOKEN_MANDATORY_LABEL))
        return NULL;
    PSID level = bounded_sid(&label, n, ((TOKEN_MANDATORY_LABEL *)label.bytes)->Label.Sid);
    if (!level || !IsWellKnownSid(level, WinMediumLabelSid)) return NULL;
    if (!GetTokenInformation(token, TokenLogonSid, logon->bytes, sizeof logon->bytes, &n) ||
        n < sizeof(TOKEN_GROUPS))
        return NULL;
    const TOKEN_GROUPS *groups = (const TOKEN_GROUPS *)logon->bytes;
    if (groups->GroupCount != 1 ||
        (groups->Groups[0].Attributes & (SE_GROUP_LOGON_ID | SE_GROUP_ENABLED | SE_GROUP_USE_FOR_DENY_ONLY)) !=
            (SE_GROUP_LOGON_ID | SE_GROUP_ENABLED))
        return NULL;
    PSID sid = bounded_sid(logon, n, groups->Groups[0].Sid);
    const SID_IDENTIFIER_AUTHORITY nt = SECURITY_NT_AUTHORITY;
    return sid && !memcmp(GetSidIdentifierAuthority(sid), &nt, sizeof nt) &&
        *GetSidSubAuthorityCount(sid) == 3 && *GetSidSubAuthority(sid, 0) == SECURITY_LOGON_IDS_RID ? sid : NULL;
}

static BOOL descriptor(PSID logon, PSID system, DWORD access, AlignedBuffer *output, DWORD *length)
{
    SECURITY_DESCRIPTOR abs;
    AlignedBuffer a;
    PACL acl = (PACL)a.bytes;
    if (!InitializeAcl(acl, sizeof a.bytes, ACL_REVISION) ||
        !AddAccessAllowedAce(acl, ACL_REVISION, access, logon) ||
        !InitializeSecurityDescriptor(&abs, SECURITY_DESCRIPTOR_REVISION) ||
        !SetSecurityDescriptorOwner(&abs, system, FALSE) ||
        !SetSecurityDescriptorGroup(&abs, system, FALSE) ||
        !SetSecurityDescriptorDacl(&abs, TRUE, acl, FALSE))
        return FALSE;
    *length = sizeof output->bytes;
    return MakeSelfRelativeSD(&abs, output->bytes, length) &&
        *length > 0 && *length <= sizeof output->bytes && IsValidSecurityDescriptor(output->bytes);
}

static BOOL session_policy(SessionPolicy *policy, DWORD file_rights, DWORD directory_rights)
{
    HANDLE token = NULL;
    /* Never choose a process logon if this thread is already impersonating. */
    if (OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, TRUE, &token)) {
        CloseHandle(token); return FALSE;
    }
    if (GetLastError() != ERROR_NO_TOKEN || !OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &token))
        return FALSE;
    AlignedBuffer actual, system;
    PSID sid = local_logon(token, &actual);
    DWORD n = sizeof system.bytes;
    BOOL ok = sid && CopySid(sizeof policy->sid.bytes, policy->sid.bytes, sid) &&
        CreateWellKnownSid(WinLocalSystemSid, NULL, system.bytes, &n) &&
        descriptor(sid, system.bytes, file_rights, &policy->file, &policy->file_bytes) &&
        descriptor(sid, system.bytes, directory_rights, &policy->directory, &policy->directory_bytes);
    CloseHandle(token);
    return ok;
}
