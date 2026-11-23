"""Untriggered compile-only evidence for an explicitly approved immutable v21 pair.

In a disposable non-root runner, stage the reviewed upstream commit in adjacent
private `repo` (FETCH_HEAD) and `source` (Git archive) directories. Use
`python3 -I source/zxs/tests/acceptance/rust_smb_move_pair_measure.py --repo …
--source … --work … --commit …`, with absolute paths and a new empty private work
directory. A successful run retains only `measurement.json`: not a guest receipt,
binary archive or an approval. A failed build may record a fixed `failure` category.
Independently approve measured digests before requesting the separate guest
acceptance. No previous schema fallback.
"""

import argparse
import os
from pathlib import Path
import re
import subprocess

FOLDER = Path("zxs/tests/acceptance")
MEASURER = FOLDER / "rust_smb_move_pair_measure.py"
COMPILER = FOLDER / "rust_smb_move_pair.py"
MAX_SOURCE = 32 * 1024


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--commit", required=True)
    args = parser.parse_args()
    os.umask(0o077)
    try:
        if (os.getuid() != os.geteuid() or os.getuid() == 0
                or not re.fullmatch("[0-9a-f]{40}", args.commit) or args.commit == "0" * 40
                or args.repo != args.source.parent / "repo"
                or not (args.source / MEASURER).is_absolute()
                or Path(__file__).resolve(strict=True) != args.source / MEASURER):
            raise ValueError("expected the private reviewed checkout and exact commit")
        env = {"PATH": "/usr/bin:/bin", "LC_ALL": "C", "TZ": "UTC", "GIT_CONFIG_NOSYSTEM": "1",
               "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_TERMINAL_PROMPT": "0", "GIT_NO_REPLACE_OBJECTS": "1"}
        git = ("/usr/bin/git", "-C", args.repo, "-c", "core.hooksPath=/dev/null")

        def read_git(*operands):
            return subprocess.run((*git, *operands), cwd=args.source, env=env, stdin=subprocess.DEVNULL,
                                  stdout=subprocess.PIPE, check=True).stdout

        if read_git("rev-parse", "--verify", "FETCH_HEAD^{commit}") != args.commit.encode("ascii") + b"\n":
            raise ValueError("measurement source differs from its approved commit")
        def reviewed_code(path):
            local = args.source / path
            if local.resolve(strict=True) != local or not local.is_file():
                raise ValueError("expected a regular measurement source file")
            with local.open("rb") as stream:
                raw = stream.read(MAX_SOURCE + 1)
            if len(raw) > MAX_SOURCE or raw != read_git("cat-file", "blob", f"{args.commit}:{path}"):
                raise ValueError("measurement code differs from the approved commit")
            return raw

        reviewed_code(MEASURER)
        compiler = str(args.source / COMPILER)
        library = {"__file__": compiler, "__name__": "_reviewed_pair_compiler"}
        exec(compile(reviewed_code(COMPILER), compiler, "exec"), library)
        library["measure"](args.repo, args.source, args.work, args.commit)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"Pair measurement refused: {error}\n")


if __name__ == "__main__":
    main()
