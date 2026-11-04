# Restrictive bootstrap locator primitive

This batch contains source and 28 test functions for a local stop ledger. It has
not been compiled or executed locally. GitHub's experimental membership workflow
must run the exact source before any validation claim. It is not connected to
Engine::open, discover, lifecycle publication, or HTTP/Web operations.

The store owns a protected ZXS control-state directory descriptor and a dedicated
advisory lock. It writes canonical immutable generation records, then an exact
head, with real file and directory barriers. Records are never deleted or
replaced. Reload streams the directory and validates the full previous-hash
chain one bounded object at a time. There is no total generation-count limit.
Missing/corrupt heads, missing records, orphan newer records, unfinished atomic
files, ambiguous identities and changed requests return management-only errors.
An identical interrupted request can re-establish its exact durable barriers;
recovery never chooses an older apparently valid record instead.

The ledger can only restrict candidate metadata roots. Its read-probe addresses,
observed directory identities and stop hints are not membership or registration
authority. It never calculates N/Q or grants writes. Selection performs no root
stat/open/canonicalize, including when Config still contains a stopped target.
Same-filesystem directories are supported. Known equal directory identities and
ancestor/descendant addresses are rejected; control/storage identity equality is
also rejected even when their diagnostic paths differ. This does not prove the
absence of unknown aliases or protect against external rollback of the entire
control directory.

Required integration remains:

- Consume the restriction before the first target stat/open and prevent later
  seal/fence discovery from widening it. Missing/uncertain hints require an
  explicit retained-root recovery path, never a Config fallback scan.
- Establish the initial locator only from already authenticated held resources
  or an explicitly authorized recovery request. Supplying a held descriptor alone
  does not prove its registration, and the store constructor assumes its caller
  has selected the actual control-state directory.
- Wait for durable stop publication before intent/fence; verify stop context
  against real authority after restricted discovery. Re-establish the exact
  system-side barriers during terminal recovery without touching the target.
- Implement paginated root/stop objects and certified new-binding or restore
  expansion. The current 128 KiB record bound and roots-per-record allocation
  bound are temporary format capabilities, not final pool-size requirements.
- Implement terminal-pool handling before stopping the final retained metadata
  root. This primitive cannot create a zero-quorum serving pool.

Tests exercise real temporary directories, stale Config selection with a present
target and zero target probe callbacks, preserved records, held-directory
replacement, canonical/hash/binding errors, exact retries and injected boundaries
before/after record/head publication. These are source-level fault scenarios and
normal fsync crash assumptions, not physical power-cut or production-discovery
verification. No real registered directory or operating-system device is touched.
