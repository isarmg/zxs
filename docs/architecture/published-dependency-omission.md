# Published dependency omission

This candidate keeps the current disk format and decision protocol. Enabling it
requires the real scrub/service/CI gates in [ADR 0005](../adr/0005-incremental-publication-integrity.md);
source presence alone is not a passed gate.

## Authority and object origin

Only complete required-member publication with its still-valid metadata epoch creates
`PublishedStateBase`. One detached preparation captures that exact receipt,
State allocation, roots, sequence, validated configuration, continuous writer
epoch, and its periodic owner's actual completed scrub receipt. Neither a public
report nor an existing object name creates this authority. Reopen, recovery,
member/binding change, uncertain authoritative publication, and observed shared
metadata faults invalidate it. An embedded Engine without an actual periodic
owner still takes the complete dependency path. A failure in the disposable system-root SQLite index observes no required-member metadata and does not revoke this authority. It disables that accelerator, retains its specific cause, and leaves the completed publication and metadata epoch unchanged.

A captured scrub receipt keeps its original start/max-age and completion/interval
deadlines. A healthy refresh under the same owner does not cancel an unexpired
permit or extend its deadlines. Failed, cancelled, superseded or incomplete runs
change a private admission generation; later success cannot revive an older
permit. A scheduled plan captures the original weak owner, admission generation
and metadata epoch. Its terminal run revokes that admission before reporting
progress or waiting for a limiter or Engine finish; a publication already holding
the Engine cannot race ahead of this observation. Finish accepts only the same
captured generation and epoch: stale successes cannot restore coverage, and late
terminal outcomes cannot erase a newer generation's or epoch's successful scan.
Healthy coverage is still issued only at the Engine finish boundary. Stop/drop
and replacement revoke the original owner. Local version proof or payload
unavailability remains separate from shared metadata authority.

Snapshot encoder output and history `PreparedUpdate.created` are newly encoded
objects even when their hash equals an earlier object's hash. They always use
real immutable-byte comparison and file/directory barriers on every required
member. History `retained` comes from the exact successful catalog root, not hash
set subtraction. With a valid permit, untouched histories and old catalog/snapshot
nodes are not re-read or reinstalled. Changed histories with an actual
[encoder-origin descriptor](history-encoder-descriptors.md) path-copy their field
maps without reading the old graph. A qualified descriptor miss explicitly reads
the exact old graph; source failures propagate with their original cause. Without
authority the complete old dependency path remains, including exact-hash
reconstruction where previously legal. Raw graph admission alone never creates
an encoder-origin descriptor.

## Final admission and concurrent revocation

Preparation checks its original permit before returning. Decision validates its
normal frozen Engine inputs and prepared payloads, writes all candidate objects
and PREPARE records, then performs one final admission **before first ACCEPT**.
The short critical section locks schedule state before metadata generation and
checks the original owner, admission generation, epoch and deadlines together.
It contains no filesystem I/O or fsync and consumes the permit. The full-install
path uses the same final boundary with its original post-store metadata epoch,
so a shared fault observed during PREPARE cannot bypass admission merely because
no periodic owner was present.

That admission is the logical start of the irreversible decision, not a claim
that a durable ACCEPT and revocation happen simultaneously. If stop, expiry or
an observed shared fault wins before admission, no ACCEPT is written; PREPARE
records may remain and the reserved slot is not reused. If admission wins, the
already-started decision must finish or retain blocked/InDoubt recovery ownership.
Subsequent stop/fault revokes future reuse, not the in-progress decision. A later
observed metadata fault also prevents minting a new successful reuse receipt.
The service can revoke promptly without waiting for the Engine or a disk barrier.

## Detection and remaining work

An unrelated write no longer promises to discover or repair silent damage in an
untouched dependency. Actual reads, full scrubs and cold/recovery/export admission
retain their real byte, membership, graph and payload checks; no healthy copy
means explicit failure. Known faults revoke shared provenance even when a later
healthy replica succeeds. Pending missing-source metadata selection remains legal
without asserting that its payload is readable.

This removes unchanged dependency read/install work, not every full traversal:
descriptor misses and unqualified history source closures are still read and
visited, and other documented business, projection, index and payload work
remains. Descriptors retain projected keys and nodes, not raw Values; their extra
residency is O(current projected key bytes + nodes), shared across generations,
not O(delta) or fixed RAM. Report warm hits, misses, cold, scrub, repair and
end-to-end costs separately; do not infer O(delta) for the whole Engine.
