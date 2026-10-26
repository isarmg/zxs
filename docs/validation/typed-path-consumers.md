# Typed path adapter boundaries

## HTTP and SMB upload admission

- HTTP upload admission parses the external query path into `PortablePath` before
  spool metadata or payload creation. `Backend::upload`, `upload_generation`,
  management forwarding, and the authorization wrapper borrow this same type.
- `EngineBackend` forwards the admitted path to `Engine::prepare_upload_file`.
  The upload plan and staged-file record retain the path and cached lookup key.
- An SMB write handle keeps the existing entry's `PortablePath`; a missing create
  target is parsed before a workspace or payload is allocated. The writer retains
  that type through upload and publication. Canonical-equivalent lookup spelling
  therefore does not become a proposed creation spelling.
- Current credentials, workspace authorization, publication-time reauthorization,
  descriptor ownership, size/hash checks, and fencing checks remain unchanged.

## Namespace and read projections

The workspace namespace wire boundary calls `NamespaceOperation::parse`
explicitly. Its contract error is mapped through the existing engine API error
registry, preserving specific naming codes rather than replacing them with a
JSON decoding error. The operation's typed paths are passed to the application
and engine directly. Creation paths require NFC; existing-path references accept
canonical-equivalent lookup spelling.

Entry responses still expose the established `path`, `name`, and `lookup_key`
wire fields. Internally, projections read `Entry` getters instead of independent
mutable strings. Selected-entry matching and SMB writer publication reuse cached
keys instead of normalizing the admitted entry path again. Reopening a dirty
writer's workspace uses two typed single-entry lookups at the previous publication
and new workspace's frozen heads; the preliminary current-branch authorization
uses its bounded authorization projection. No whole-tree JSON round trip is
needed for those single-entry comparisons.

## Scope still remaining

The older generic SMB lookup/namespace action bridge still carries JSON and raw
paths between some adapter methods. Its validation chain is not claimed to be a
single typed boundary in this change. The follow-on
[shared snapshot change](typed-smb-snapshots.md) removes `finish_publication` and
namespace precondition Tree/JSON round trips with existing immutable commit
handles, preserving entry-id selection and workspace-base binding.
Operation identifiers and actor identifiers also remain
separate work: SMB dispatch, namespace staging, and mutation digest
planning can still revalidate the same raw operation identifier.

## Behavioral coverage

Server tests cover exact namespace naming error codes, rejection before staging,
canonical-equivalent rename source spelling, retained NFC display spelling,
HTTP upload rejection before spool creation, and successful admitted upload.
Existing SMB tests cover frozen snapshot lookup, write-open races, binding/session
revocation, descriptor lifetime, and publication fencing. The web wire consumer
requires no field-format change.

## Verification in the restricted Linux executor

- `cargo check -p storage-server -p web-app`: passed.
- `cargo clippy -p storage-server -p web-app --all-targets --all-features -- -D warnings`:
  passed.
- `cargo fmt -p storage-server -p web-app --check` and `git diff --check`: passed.
- Final runnable test selection: server library 443 passed / 3 ignored; server
  binary 6 passed; HTTP integration 54 passed; login admission 3 passed; startup
  6 passed; telemetry 1 passed; web 255 passed; both documentation-test targets
  passed (zero cases).
- The initial unfiltered run established six server unit failures at Unix socket
  binding with `EPERM`. Two daemon startup tests also waited unsuccessfully for
  their SMB socket: `repeated_normal_daemon_stops_release_same_address_and_socket`
  and `daemon_serves_web_and_liveness_with_missing_or_corrupt_pool`. Those fixtures
  suppress the child process's diagnostic output. These eight cases were not repeated in the final selected run. No permission
  changes or socket workarounds were used. They still require an environment that
  permits the normal Unix socket lifecycle.
- The pinned Samba 4.23 SDK is absent, so the `smb-vfs` module cannot be built
  here. Web tests above are native; after installing the pinned Wasm target,
  `cargo clippy -p web-app --target wasm32-unknown-unknown --all-targets --all-features -- -D warnings`
  also passed. Neither SDK checks nor target requirements were disabled.
- Repository-wide formatting traverses vendored upstream packages and encounters
  missing upstream test modules. All incidental formatting changes outside the
  server consumer files were restored; the targeted formatting check is the
  applicable completed check here.


The incremental dirty-writer candidate `cbc1942c` additionally passed 443 release
library tests (3 ignored; the same six denied Unix listener cases not repeated),
plus strict release all-target/all-feature server and web Clippy. The repeated
same-fixture release lookup measurements and their shared-container noise are
recorded in [typed-smb-baseline.md](typed-smb-baseline.md).
