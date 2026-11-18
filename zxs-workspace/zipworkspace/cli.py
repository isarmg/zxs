from __future__ import annotations
import argparse
from dataclasses import asdict
import json
from pathlib import Path
import sqlite3
import sys
import zipfile
from .archive import read_archive
from .model import Rejected
from . import worktree as w
from . import lifecycle, refs, opening


def main(args=None):
    p = argparse.ArgumentParser(prog="zip-workspace", description="Use ordinary working directories and explicitly publish complete ZIP directory commits.")
    commands = p.add_subparsers(dest="action", required=True)
    def add(command, fields, help):
        s = commands.add_parser(command, help=help)
        for f in fields.split(): s.add_argument(f, type=Path)
        return s
    add("init", "worktree store state", "register an existing directory; do not commit it")
    add("inspect", "archive", "fully validate a typed .zxs repository archive")
    add("checkout", "archive worktree store state", "check out the selected directory snapshot, never a guessed head")
    add("import-zip", "archive worktree", "import an ordinary unencrypted Store .zip into a new directory")
    add("status", "state", "show this checkout's recorded base and pending original")
    auto=add("checkout-auto","archive parent store state","allocate a collision-safe fresh local checkout name")
    auto.add_argument("name")
    add("checkout-status","state","inspect the original checkout identity, destination and phase")
    add("resume-checkout","state","resume only the exact original checkout").add_argument("operation")
    add("history", "archive store", "follow exact directory-level commit ancestry")
    add("branches", "archive store", "list complete observed directory heads and exact missing ancestors")
    for command, field in (("checkout-branch","branch"), ("checkout-node","operation")):
        add(command, "archive store worktree state", "check out an exact directory branch head or historical node").add_argument(field)
    add("changes", "state", "compare actual ordinary working files to their original directory snapshot")
    for action in ("hide", "restore", "resume-visibility"):
        vis = add(action, "state", "move or verify only this exact clean local worktree")
        vis.add_argument("--writers-stopped", action="store_true", required=True)
        if action == "resume-visibility": vis.add_argument("operation")
    add("fork-pending", "state", "explicitly fork an exact candidate after a verified branch conflict").add_argument("operation")
    c = add("commit", "state", "freeze and publish all ordinary files as one .zxs archive")
    c.add_argument("--writers-stopped", action="store_true", required=True, help="confirm all external worktree writers have stopped")
    c.add_argument("--fork", action="store_true", help="explicitly start a new branch from the selected base")
    add("resume", "state", "continue or verify only this original operation").add_argument("operation")
    ns = p.parse_args(args)
    try:
        if ns.action == "init":
            w.enroll(ns.worktree, ns.store, ns.state); result = w.status(ns.state)
        elif ns.action == "inspect":
            marker, tree = read_archive(ns.archive)
            result = {"commit": asdict(marker), "entries": len(tree.rows), "data_bytes": tree.total}
        elif ns.action == "checkout":
            marker = w.checkout(ns.archive, ns.worktree, ns.store, ns.state); result = {"checked_out": marker.operation}
        elif ns.action == "import-zip":
            tree = w.import_zip(ns.archive, ns.worktree); result = {"imported_entries": len(tree.rows)}
        elif ns.action == "checkout-auto":
            path, marker = opening.begin(ns.archive,ns.parent,ns.name,ns.store,ns.state,automatic=True)
            result = {"checked_out":marker.operation,"worktree":str(path),"original":opening.status(ns.state)}
        elif ns.action == "checkout-status": result = opening.status(ns.state)
        elif ns.action == "resume-checkout":
            path, marker = opening.finish(ns.state,ns.operation)
            result = {"original_checked_out_node":marker.operation,"original_destination":str(path)}
        elif ns.action == "status": result = w.status(ns.state)
        elif ns.action == "branches":
            result = refs.Catalog(ns.archive, ns.store).branches()
        elif ns.action in ("checkout-branch", "checkout-node"):
            catalog = refs.Catalog(ns.archive, ns.store)
            node = catalog.select_branch(ns.branch) if ns.action == "checkout-branch" else catalog.select_node(ns.operation)
            result = {"checked_out":refs.checkout(node, ns.worktree, ns.store, ns.state).operation}
        elif ns.action == "changes": result = lifecycle.changes(ns.state)
        elif ns.action in ("hide", "restore"): result = lifecycle.start(ns.state, ns.action)
        elif ns.action == "resume-visibility": result = lifecycle.finish(ns.state, ns.operation)
        elif ns.action == "fork-pending":
            target, marker = w.fork_pending(ns.state, ns.operation)
            result = {"committed_archive":str(target), "commit":asdict(marker)}
        elif ns.action == "history":
            chain, missing = w.history(ns.archive, ns.store); result = {"commits": [asdict(x) for x in chain], "missing_parent": missing}
        else:
            target, marker = w.finish(ns.state, ns.operation) if ns.action == "resume" else w.commit(ns.state, fork=ns.fork)
            result = {"committed_archive": str(target), "commit": asdict(marker)}
        print(json.dumps(result, ensure_ascii=False, indent=2))
        return 0
    except (OSError, Rejected, sqlite3.Error, zipfile.BadZipFile, UnicodeError, ValueError) as err:
        print(f"zip-workspace: {err}", file=sys.stderr); return 1

if __name__ == "__main__": raise SystemExit(main())
