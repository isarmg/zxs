"""Translate exact Linux edit-session evidence into one durable per-container Submit."""
from dataclasses import dataclass

from .container_plan import Baseline, DeleteIntent, LocalFile, _id, plan_submit
from .container_spool import capture_submit
from .linux_edit import NamespaceEvent
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
        if type(self.baseline) is not Baseline or member_name(self.path)[-1] != self.baseline.name:
            raise Rejected("local binding lacks original display name and baseline")


def _select(cut, originals, new_identities):
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
    retired, replaced, deletions, witnessed = set(), set(), [], {}
    for event in cut.namespace_events:
        if type(event) is not NamespaceEvent:
            raise Rejected("untyped Linux namespace event")
        local, path = _native(event.local_identity), _path(event.original_path)
        if local in retired or (local in witnessed and witnessed[local] != path):
            raise Rejected("namespace chain disagrees with the exact recorded original")
        previous = bound.get(local)
        if type(event.local_birth) is not bool or (previous is None) != event.local_birth:
            raise Rejected("namespace mutation lacks an original binding or positive native birth")
        if previous and previous[1] != path:
            raise Rejected("namespace action did not name the original container path")
        if not previous and path in seen_paths:
            raise Rejected("unexpected native identity at a bound path")
        if event.kind == "rename" and event.replacement_identity is None:
            destination = _path(event.destination_path)
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
                baseline = previous[0].baseline
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

    created = {}
    for local, container in new_identities:
        _native(local)
        _id(container)
        if local in reserved or local in retired or local in created or container in container_ids:
            raise Rejected("new container identity overlaps the original namespace journal")
        created[local] = container
        container_ids.add(container)

    selected, current = {}, {}
    for original, path in bound.values():
        old = original.baseline
        current[old.container_id] = (path, old)
    seen_members, used_new = set(), set()
    for member in cut.changed_files:
        local, path = _native(member.local_identity), _path(member.path)
        if local in retired or local in seen_members or (local in witnessed and witnessed[local] != path):
            raise Rejected("ambiguous exact changed-file membership")
        seen_members.add(local)
        previous = bound.get(local)
        if previous:
            if previous[1] != path:
                raise Rejected("modified original file moved without a journal event")
            container = previous[0].baseline.container_id
        else:
            if local not in created or path in seen_paths:
                raise Rejected("changed file has no exact original binding or explicit new container")
            container = created[local]
            used_new.add(local)
            current[container] = (path, None)
            seen_paths.add(path)
        selected[container] = member
    if created.keys() != used_new:
        raise Rejected("explicit new container lacks an exact changed-file member")
    forced = replaced & current.keys()
    if not forced <= selected.keys():
        raise Rejected("replaced original lacks its final complete changed-file member")
    return selected, current, tuple(deletions), tuple(sorted(forced))


def capture_linux_submit(state, owner, *, submission, destination, originals,
                         new_identities=(), branch_intents=(), admit_created=False):
    """Use a stopped original Linux cut. Neither namespace events nor borrowed FDs go to ZXS."""
    originals, new_identities, branch_intents = tuple(originals), tuple(new_identities), tuple(branch_intents)
    if type(admit_created) is not bool:
        raise Rejected("invalid explicit new file selection")
    selected_state = None

    def select(cut):
        nonlocal selected_state
        exact = {id(row): row for row in cut.changed_files}
        births = {}
        for row in cut.created_files:
            if exact.get(id(row)) is not row or row.local_identity in births:
                raise Rejected("unattested native file creation")
            births[row.local_identity] = row
        proposed = dict(new_identities)
        if len(proposed) != len(new_identities) or not proposed.keys() <= births.keys():
            raise Rejected("container ID allocation needs a proved native creation")
        if admit_created:
            for key in sorted(births):
                if key not in proposed:
                    proposed[key] = new_id()
        chosen, current, deletions, forced = _select(cut, originals, tuple(proposed.items()))
        selected_state = (frozenset(chosen), current, deletions, forced)
        return chosen

    def build(captured):
        expected, current, deletions, forced = selected_state
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
        return plan_submit((item.baseline for item in originals), observed, deletions,
                           branch_intents=branch_intents, attested_complete=True, force_save=forced)

    return capture_submit(state, owner, submission=submission, destination=destination,
                          select_cut=select, build_plan=build)
