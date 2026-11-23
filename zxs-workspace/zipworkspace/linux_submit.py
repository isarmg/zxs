"""Translate exact Linux edit-session evidence into one durable per-container Submit."""
from dataclasses import dataclass

from .container_plan import (Baseline, BranchIntent, DeleteIntent, IgnoredOriginalNameConflict, LocalFile,
                             _id, plan_submit)
from .container_spool import capture_submit
from .linux_edit import NamespaceEvent
from .linux_upload_ignore import RULE_FILE
from .model import Rejected, member_name, new_id


def _native(identity):
    if (type(identity) is not tuple or len(identity) != 2
            or any(type(part) is not int for part in identity)
            or identity[0] < 0 or identity[1] <= 0):
        raise Rejected("invalid exact local inode identity")
    return identity


def _path(value):
    member_name(value)
    return value


@dataclass(frozen=True)
class OriginalBinding:
    """From the controller's pre-edit exact native bind, never a scan of WORK."""
    local_identity: tuple
    path: str
    baseline: Baseline

    def __post_init__(self):
        _native(self.local_identity)
        if (type(self.baseline) is not Baseline or self.path == RULE_FILE
                or member_name(self.path)[-1] != self.baseline.name):
            raise Rejected("local binding lacks original display name and baseline")


def _select(cut, originals, new_identities, *, admit_created):
    ignored = cut.upload_ignore.matches
    bound = {}
    seen_paths, container_ids = set(), set()
    for original in originals:
        if (type(original) is not OriginalBinding or original.local_identity in bound
                or original.path in seen_paths or original.baseline.container_id in container_ids):
            raise Rejected("ambiguous pre-edit container binding")
        bound[original.local_identity] = (original, original.path)
        seen_paths.add(original.path)
        container_ids.add(original.baseline.container_id)
    reserved = set(bound)
    retired, replaced, local_only = set(), set(), set()
    deletions, witnessed, kept = [], {}, {}
    for event in cut.namespace_events:
        if type(event) is not NamespaceEvent:
            raise Rejected("untyped Linux namespace event")
        local, path = _native(event.local_identity), _path(event.original_path)
        if local in retired or (local in witnessed and witnessed[local] != path):
            raise Rejected("namespace chain disagrees with the exact recorded original")
        previous = bound.get(local)
        if type(event.local_birth) is not bool or (previous is not None and event.local_birth):
            raise Rejected("namespace mutation conflicts with its original binding")
        if previous is None and not event.local_birth:
            if local not in local_only and not ignored(path):
                raise Rejected("namespace mutation lacks an original binding or positive native birth")
            local_only.add(local)  # An unbound existing local-only file never acquires a container.
        if path == RULE_FILE:
            local_only.add(local)  # Even a renamed rules inode cannot become an upload.
        if previous and previous[1] != path:
            raise Rejected("namespace action did not name the original container path")
        if not previous and path in seen_paths:
            raise Rejected("unexpected native identity at a bound path")
        if event.kind == "rename" and event.replacement_identity is None:
            destination = _path(event.destination_path)
            if destination == RULE_FILE:
                if previous is not None:
                    raise Rejected("an original container cannot become .zxsignore")
                local_only.add(local)
            if destination in seen_paths:
                raise Rejected("native rename conflicts with an existing container binding")
            witnessed[local] = destination
            if previous:
                seen_paths.remove(path)
                seen_paths.add(destination)
                bound[local] = (previous[0], destination)
        elif event.kind == "unlink" and event.destination_path is None and event.replacement_identity is None:
            retired.add(local)
            if previous:
                del bound[local]
                seen_paths.remove(path)
                original = previous[0]
                baseline = original.baseline
                if local in local_only or ignored(path):
                    kept[baseline.container_id] = (original, local, path)
                else:
                    deletions.append(DeleteIntent(baseline.container_id, baseline.revision))
        elif event.kind == "replace" and event.destination_path == path and previous:
            new = _native(event.replacement_identity)
            if new in reserved or new in retired or new == local:
                raise Rejected("replacement did not use an unbound native editor temp")
            # The accepted provider event attests that this exact unbound inode
            # was born in-session and installed over the explicitly bound target.
            retired.add(local)
            del bound[local]
            bound[new] = previous
            reserved.add(new)
            witnessed[new] = path
            replaced.add(previous[0].baseline.container_id)
        else:
            raise Rejected("unsupported or unbound Linux namespace event")

    # Validate the complete native birth set, then allocate IDs only for the
    # exact unbound identities still eligible at their final paths.
    files = cut.changed_files
    exact = {id(row): row for row in files}
    births = {}
    for row in cut.created_files:
        local = _native(row.local_identity)
        if exact.get(id(row)) is not row or local in births:
            raise Rejected("unattested native file creation")
        births[local] = row
    eligible = {local for local, row in births.items()
                if local not in local_only and not ignored(row.path)}
    proposed = dict(new_identities)
    if len(proposed) != len(new_identities) or not proposed.keys() <= eligible:
        raise Rejected("container ID allocation needs a proved, non-ignored native creation")
    if admit_created:
        for local in sorted(eligible - proposed.keys()):
            proposed[local] = new_id()
    created = {}
    for local, container in proposed.items():
        _native(local)
        _id(container)
        if local in reserved or local in retired or local in created or container in container_ids:
            raise Rejected("new container identity overlaps the original namespace journal")
        created[local] = container
        container_ids.add(container)

    selected, current = {}, {}
    for local, (original, path) in bound.items():
        old = original.baseline
        if local in local_only or ignored(path):
            kept[old.container_id] = (original, local, path)
        else:
            current[old.container_id] = (path, old)
    for container, (original, _identity, _retained_name) in kept.items():
        current[container] = (original.path, original.baseline)
    seen_members, used_new = set(), set()
    for member in files:
        local, path = _native(member.local_identity), _path(member.path)
        if local in retired or local in seen_members or (local in witnessed and witnessed[local] != path):
            raise Rejected("ambiguous exact changed-file membership")
        seen_members.add(local)
        previous = bound.get(local)
        if previous:
            if previous[1] != path:
                raise Rejected("modified original file moved without a journal event")
            container = previous[0].baseline.container_id
            if container in kept:
                continue
        else:
            if local in local_only or ignored(path):
                continue
            if local not in created or path in seen_paths:
                raise Rejected("changed file has no exact original binding or explicit new container")
            container = created[local]
            used_new.add(local)
            current[container] = (path, None)
            seen_paths.add(path)
        selected[container] = member
    if created.keys() != used_new:
        raise Rejected("explicit new container lacks an exact changed-file member")
    projected = {tuple(part.casefold() for part in member_name(path)) for path, _ in current.values()}
    if len(projected) != len(current):
        moved = {original.path: path for original, local, path in kept.values()
                 if local == original.local_identity and path != original.path and ignored(path)}
        for path, old in current.values():
            if old is None and path in moved:
                raise IgnoredOriginalNameConflict(path, moved[path])
        raise Rejected("destination collides with a retained original container name")
    forced = (replaced & current.keys()) - kept.keys()
    if not forced <= selected.keys():
        raise Rejected("replaced original lacks its final complete changed-file member")
    return selected, current, tuple(deletions), tuple(sorted(forced)), frozenset(kept)


def capture_linux_submit(state, owner, *, submission, destination, originals,
                         new_identities=(), branch_intents=(), admit_created=False):
    """Use a stopped original Linux cut. Neither namespace events nor borrowed FDs go to ZXS."""
    originals, new_identities, branch_intents = tuple(originals), tuple(new_identities), tuple(branch_intents)
    if type(admit_created) is not bool:
        raise Rejected("invalid explicit new file selection")
    selected_state = None

    def select(cut):
        nonlocal selected_state
        chosen, current, deletions, forced, kept = _select(cut, originals, new_identities,
                                                            admit_created=admit_created)
        selected_state = (frozenset(chosen), current, deletions, forced, kept)
        return chosen

    def build(captured):
        expected, current, deletions, forced, kept = selected_state
        if captured.keys() != expected:
            raise Rejected("private captures do not cover the exact chosen effects")
        observed = []
        for container, (path, old) in current.items():
            if container in captured:
                actual, body = captured[container]
                if actual != path:
                    raise Rejected("sealed file path did not match the final attested effect")
            else:
                body = old.body
            observed.append(LocalFile(container, path, body, old.tags if old else ()))
        intents = (row for row in branch_intents if not (type(row) is BranchIntent and row.container_id in kept))
        return plan_submit((item.baseline for item in originals), observed, deletions,
                           branch_intents=intents, attested_complete=True, force_save=forced)

    return capture_submit(state, owner, submission=submission, destination=destination,
                          select_cut=select, build_plan=build)
