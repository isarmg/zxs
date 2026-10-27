# Typed error boundaries

`storage_contract::ErrorCode` is the closed identity shared by first-party
contract, naming, engine, HTTP server and client code. Constructors and internal
comparisons take enum variants. `as_str`, serialization and display are the only
places that produce the documented uppercase wire spelling. There is no
`From<&str>` compatibility conversion for internal callers.

The server registry exhaustively maps identities to public aliases and HTTP
statuses. Adding a code requires an explicit policy decision; neither an unknown
string nor a new enum variant silently receives HTTP 400. Existing aliases,
status values, retry decisions and HTTP 500 diagnostic suppression are preserved.

## Boundary validation

HTTP error-envelope deserialization and client response decoding validate code
strings once. An unknown remote code is an explicit invalid-response error,
retaining the JSON decoder source, actual HTTP status, explicit retry advice and
request identifier. The enum decoder borrows the JSON value. A response without
an error code retains the existing generic HTTP-error classification. Client
mutation outcome classification still uses the actual HTTP status, never the
parse-error code. Internal code uses the validated enum.

`storage_contract::Error` retains concrete I/O, JSON and decimal-overflow causes
through `std::error::Error::source`. Domain validation messages need no fabricated
source. Fixed checksum-header slices use `expect` after the existing external
length check; malformed external lengths and payloads still return errors.

## Engine sources

`storage_engine::Error` is separate from the portable contract error. Filesystem,
SQLite, payload verification, timestamp, digest and integer parsing errors retain
their concrete causes. Contextual reclassification wraps `Box<Error>`, not a
trait object, and retains the previous public message, code and explicit retry
policy. Directory-name decoding retains the rejected `OsString`. A published
but not durably synchronized filesystem operation retains its original I/O
source and `DURABILITY_UNKNOWN` classification.

Index failures retain the underlying database/platform cause. The existing
SQLite corruption classifier alone decides whether quarantine/rebuild is safe;
permission, busy, schema and I/O failures do not become corruption. A failed
optional history-index read disables that acceleration and retains its concrete
source. A post-publication update failure likewise becomes owned index health,
not a failed mutation receipt. Close failure retains both causes without replacing
the primary one. This exception is limited to the disposable index; authoritative
publication and workspace persistence failures still propagate. Poisoned engine
locks expose invariant failure rather than becoming a recoverable storage result.

## Server boundaries

`ApiError` is an outbound-only serialized envelope. Its code is `ErrorCode` and
its status is `http::StatusCode`; internal constructors and the registry cannot
carry an invalid raw status. Neither the status field nor its concrete
`ApiErrorSource` is serialized. Request decoding, I/O, task joins, admission,
authentication and formatting adapters retain causes without logging them.
HTTP response completion and SMB response serialization own response logging;
consumed startup/recovery/telemetry failures stay with their decision owner.
After headers are sent, bounded stream failures remain transport errors and are
logged by that stream boundary, never rewritten into JSON. Server library entry
points return the concrete `ServeError`, not boxed dynamic errors.

The public backend's content hash still crosses a real boundary: constructing
an ETag remains fallible and retains `InvalidHeaderValue`. Runtime clock values
can be outside RFC3339's year range, so formatting them also retains its source.
Only constant formats, validated generated header values, internal serializers
and poisoned locks use invariant assertions.

## 平台原语边界

`platform-io` 只为文件系统、SQLite、负载及其他平台操作提供具体错误，
不定义业务错误码或 HTTP JSON 信封。产品使用上述 `storage_contract::ErrorCode`
与服务端 `ApiError`；平台错误通过引擎或服务端的具体来源变体保留，
HTTP/SMB 映射仍属于协议适配层。

## Staging limits

These layers do not claim that all repository error paths are migrated. Other
native-client, authentication, platform and SMB-specific error domains still
need their own source-preservation audit. Existing engine scheduler panic
containment and unrelated fallback/state policies are not changed by these
layers. In particular, this is not a replacement for a whole-repository audit
of ignored results, defensive defaults, finite-state strings or public API
validation contracts.
