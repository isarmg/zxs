# Terminal Backup: source-first namespace closure

This change is an atomic source candidate, not an execution receipt. The original
900-second transaction test, both default and fault-injection configurations, and
its ordinary successor/cold-replay assertions remain required. No local build,
test, selector execution, or performance result accompanies this document.

## The missing production boundary

The original terminal fixture must start at a real `create_disk_root` repository,
create its ordinary `nested` and `nested/empty` directories through namespace
publication, and save actual retained BODY versions. A generic repository or a
hand-created directory cannot provide a namespace/root binding.

Fixing that setup alone is insufficient. A new native Backup target has a real
retained directory lock and later a native writer, but it is not an ordinary
Engine Current. It previously had no source-first root/parent/FILE feedback that
ordinary readers could consume. The native first-slot verifier, original FILE
refresh, and ordinary serving tail also used different material address spaces.

## Source and durability graph

| Phase | Actual source | Durable output / next consumer |
| --- | --- | --- |
| Original fence | Complete fixed original-member choices, exact request, original State and target identities | Root feedback, identical and read back on every original member |
| Target root preparation | Retained target lock plus that exact fenced source | Five lifecycle area inodes; existing original RootBindings stay unchanged |
| Namespace planning | Original admitted State, complete registered source chain, actual target component/space limits | Bounded whole-target plan before any namespace preparation |
| Namespace preparation | Same source and target lock | Exclusive private prefix/parent/FILE entities; every prefix's five areas and every parent inode are recorded |
| Feedback closure | All original members retain the same complete layout record | Actual target projection includes root and layout feedback before common decision |
| First candidate | Real admitted Backup decision and its one complete State recipe | Exact first Candidate/hash, namespace feedback and operation result together |
| Published serving | Real first Publication plus full first State and retained target writer | BODY, FILE, owner marker and local version proofs are filled in recorded entities, then installed without replacement |
| Original refresh | Same first Publication, original frozen Config/root/State and actual held occurrences | Canonical published FILE images via the common preserving installer |
| Common activation | Existing complete serving plan, all original/target completions and selectors | Existing native serving switch; no earlier stage installs Engine Current |
| Cold startup | Existing native decision/first Publication and exact retained layouts | Separate Backup payload source; no forged ordinary Current or raw path fallback |

The root/layout feedback is descriptive data, not a writer or adoption token.
Every consumer rebinds it to the independently admitted original source, complete
request/decision, full registration, same held target and recorded entities.
Unrecorded partial creation is preserved and rejected. A missing original-member
feedback copy may be filled only by the still-live exact original-member fence,
then all copies must read back before the product escapes.

## Physical layout and identity

Target BODY/FILE lives below the real source chain, for example
`C02/B--uuid/A--uuid/common-data/nested/file`. Version descriptors, checksums and
local publication proofs live below the same source slice's
`zxs-data/common-data/<history>/versions/<version>`. Disk-global membership,
locks, native packages and first-publication evidence remain separate.

Every prefix occurrence binds its own five lifecycle areas. Parent occurrences
bind their logical EntryId, destination/source chain and exact parent entity.
The whole first Candidate carries those occurrences. Installation is parent-first
NOREPLACE from the originally recorded private stage. A same-name/same-bytes
replacement, both-stage-and-final ambiguity, missing entity, wrong source chain,
or unknown child is an error. No legacy histories/layout adoption is introduced.

The actual held descriptor remains the I/O root across decided renames; published
names are checked against its recorded inode. The bounded inventory allows only
expected entries and the existing bounded atomic-file temporary pattern.

## Complete material and scope

`PublishedBackupLayout`, `PublishedBackupOriginal` and `PublishedBackupPayload`
are disjoint private products of the actual Backup decision/Publication. They do
not implement ordinary `ObservedContainerReplica` or issue Engine Current.
The shared local-publication verification kernel receives a narrow Backup reader;
its descriptor/checksum/commit/publication checks are not duplicated or bypassed.

All BODY reads retain the source lifecycle FD, verify the complete content and
checksum commitment, and retain named inode/stamp checks. Original FILE refresh
uses the existing database installer and original owner/BODY preflight. All
recorded target files are verified before the first public target name is moved;
failures can retain decided partial durable work but cannot activate serving.

Ordinary successors use each target's authenticated direct source. The existing
payload-copy kernel now keeps the already-installed donor FD until all actual
children consume it. With `[P01, B01, C02, C03]`, both C02 and C03 consume B01,
not C03 consuming the immediately preceding C02. Complete route/source checks,
copy/hash validation, cancellation, FD reservation and name/root checks remain.

## Resource and format boundaries

The existing global limits and the test's 900-second timeout are unchanged.
New explicit local limits are 100,000 held namespace entries, 16 MiB whole layout
feedback, 256 KiB root feedback, 96 MiB prepared serving metadata, 32 MiB serving
manifest/completion, and 4 MiB original FILE completion. Actual name/space/FD
budgets are checked before their corresponding creation or copy. Fixed-width
zero entities are confined to a private pre-I/O budget shape and never admitted
as physical evidence.

The native target projection now requires complete root/layout feedback. Missing
fields or old partial native feedback do not gain defaults or inferred entities.
There is no migration of existing pools or unrecorded stages. The existing
transition/first-publication codecs still define the canonical hash domains;
text at their pre-existing public report boundaries is not a new source of trust.

This first closure supports the real whole ordinary Automatic FILE transaction
and its ordinary parent/empty-directory occurrences on one fixed primary chain.
Manual/Draft, deletion lifecycle, directory-Git whole units, and mixed-primary
whole-target material are explicitly rejected by this isolated preparation path.
It does not claim those modes, arbitrary generation transitions, or disk-name
renaming are implemented.

## Required evidence

The existing exact test remains:

`backup_preparation::transaction_serving::same_backup_switch_serves_and_replays_normal_successor_publications`

It retains actual original and latest BODY, native first Publication, all original
selectors, exact retry, C02/C03 replacement/fanout, a normal successor, cold reopen,
and a further ordinary write. Additional assertions inspect source-first five-area
and parent/FILE inode feedback, real old/new BODY paths, empty directory retention,
and replacement of an empty target parent with a different inode before final
activation. That replacement must be rejected without changing original selectors
or operation State; restoring the original inode then uses the same decision.

The fault-injection run also cuts at `after_backup_bootstrap_selected`: the full
root-extension selection is durable but no common selector exchange/completion
has occurred. It drops the original Engine, rejects ordinary open and an equal
empty-parent inode replacement through explicit cold resume, restores the same
inode, and completes the original operation without another preparation or
Publication. The same original/latest BODY, fanout, successor and later cold-write
assertions then run. The default configuration retains the live completion path.
Both runs still have exactly the original 900-second limit.

Live first activation and unfinished cold resume use the same `BackupServingImage`
recipe and entity/material verifier. Their read-only native source consumes the
actual first Publication, original State commitment, complete first State,
selected target layout and held root from `BackupStartupAdmission`; no prepared
writer/Current is synthesized. Only verified completion bytes escape the cold
adapter. The bounded layout descriptions are retained once by the real first
candidate, rather than decoding the native authority again for each target.
Completed cold startup intentionally keeps its ordinary successor-replay branch
and does not reapply first-image checks after later legitimate writes.

The existing cost/scope fixtures obtain their registry through the actual typed
preparation entry rather than constructing fields. CI adds exact default/FI
transaction selection and Engine default/lean checks for every changed production
boundary, preserving prior selection groups and their bounded projections. No
assertion in this document substitutes for the eventual hosted execution result.
