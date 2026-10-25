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
retaining the JSON decoder source and the actual HTTP status. A response without
an error code retains the existing generic HTTP-error classification. Client
mutation outcome classification still uses the actual HTTP status, never the
parse-error code. Internal code uses the validated enum.

`storage_contract::Error` retains concrete I/O, JSON and decimal-overflow causes
through `std::error::Error::source`. Domain validation messages need no fabricated
source. Fixed checksum-header slices use `expect` after the existing external
length check; malformed external lengths and payloads still return errors.

## Staging limits

This layer establishes typed identities and portable contract sources. It does
not claim that all repository error paths are migrated. Engine-specific
filesystem/SQLite contextual errors still need an engine-local error enum;
those dependencies must not enter the portable contract crate. HTTP/SMB/startup
source adapters and once-only outer-boundary logging are a subsequent layer.
Platform and SMB transport-specific error domains are independent boundaries.
