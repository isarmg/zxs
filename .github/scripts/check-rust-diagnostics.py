#!/usr/bin/env python3
"""Run a Cargo JSON build and reject Rust diagnostics from every local package."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import tomllib


CLIPPY_WARNINGS = (
    "clippy::wildcard_enum_match_arm", "clippy::map_err_ignore", "clippy::unreachable",
    "clippy::wildcard_imports", "clippy::too_many_lines", "clippy::excessive_nesting",
    "clippy::let_underscore_untyped", "clippy::fallible_impl_from",
)


def invalid_constant(value):
    raise ValueError(f"not a JSON constant: {value}")


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def read_json(source):
    return json.loads(source, parse_constant=invalid_constant, object_pairs_hook=unique_object)


def clippy_report_path(root, report):
    report = root / report
    if report.resolve().is_relative_to((root / ".github/clippy-warnings-baseline").resolve()):
        raise ValueError("the observation report must not overwrite a reviewed baseline")
    return report


def write_clippy_report(path, candidate, complete=False):
    report = {"complete": complete, "baseline_candidate": candidate}
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    temporary.replace(path)


def clippy_packages(metadata, scope):
    """Identify selected first-party packages using Cargo package IDs."""
    root = Path(metadata["workspace_root"]).resolve()
    members = set(metadata["workspace_members"])
    packages = {}
    for package in metadata["packages"]:
        if package["id"] not in members:
            continue
        manifest = Path(package["manifest_path"]).resolve().relative_to(root)
        if manifest.is_relative_to("src/foundation"):
            continue
        sdk = package["name"] == "smb-vfs"
        if sdk != (scope == "smb-sdk"):
            continue
        identity = package["id"]
        if not isinstance(identity, str) or identity in packages:
            raise ValueError("invalid or duplicate first-party Cargo package ID")
        packages[identity] = manifest.as_posix()
    if not packages or len(set(packages.values())) != len(packages):
        raise ValueError("missing or duplicate selected first-party manifests")
    return packages


class ClippyWarnings:
    """Measure only T03 warnings; a reviewed, exact baseline is required to pass."""

    def __init__(self, root, scope, report, capture, command, metadata):
        if Path(metadata["workspace_root"]).resolve() != root.resolve():
            raise ValueError("Cargo metadata belongs to a different workspace")
        self.packages = clippy_packages(metadata, scope)
        self.baseline = root / ".github/clippy-warnings-baseline" / f"{scope}.json"
        self.report = clippy_report_path(root, report)
        self.capture = capture
        toolchain = tomllib.loads((root / "rust-toolchain.toml").read_text(encoding="utf-8"))
        self.current = {
            "schema_version": 1,
            "scope": scope,
            "toolchain": toolchain["toolchain"]["channel"],
            "command": command,
            "counts": {path: dict.fromkeys(CLIPPY_WARNINGS, 0)
                       for path in sorted(self.packages.values())},
        }

    def record(self, event):
        code = event["message"].get("code")
        identity = event.get("package_id")
        if (not isinstance(code, dict) or code.get("code") not in CLIPPY_WARNINGS
                or not isinstance(identity, str) or identity not in self.packages):
            return False
        self.current["counts"][self.packages[identity]][code["code"]] += 1
        return True

    def write_report(self, complete):
        # A fresh completeness marker binds the report to this build attempt.
        write_clippy_report(self.report, self.current, complete)

    def finish(self, result, output):
        self.write_report(result == 0)
        print("[clippy-warnings] observed counts (not automatically accepted):", file=output)
        print(json.dumps({"complete": result == 0, "baseline_candidate": self.current},
                         indent=2, sort_keys=True), file=output, flush=True)
        if result:
            return result
        if self.capture:
            print("[clippy-warnings] capture only: review the complete CI observation and "
                  f"commit {self.baseline}; capture is not a passing ratchet", file=output)
            return 1
        try:
            baseline = read_json(self.baseline.read_text(encoding="utf-8"))
            self.validate_baseline(baseline)
        except (OSError, ValueError, TypeError, KeyError) as error:
            print(f"[clippy-warnings] reviewed baseline unavailable: {error}; "
                  "review the complete CI observation, never infer counts", file=output)
            return 1
        changed = False
        for package, counts in self.current["counts"].items():
            for code, count in counts.items():
                previous = baseline["counts"][package][code]
                if count != previous:
                    changed = True
                    direction = "rose" if count > previous else "fell"
                    print(f"[clippy-warnings] {direction} {package}:{code}: "
                          f"{previous} -> {count}; review and commit the measured baseline",
                          file=output)
        return int(changed)

    def validate_baseline(self, baseline):
        if not isinstance(baseline, dict) or baseline.keys() != self.current.keys():
            raise ValueError("Clippy baseline schema changed")
        for key in ("schema_version", "scope", "toolchain", "command"):
            if type(baseline[key]) is not type(self.current[key]) or baseline[key] != self.current[key]:
                raise ValueError(f"Clippy baseline {key} changed")
        counts = baseline["counts"]
        if not isinstance(counts, dict) or counts.keys() != self.current["counts"].keys():
            raise ValueError("Clippy baseline package inventory changed")
        for package, values in counts.items():
            if not isinstance(values, dict) or values.keys() != self.current["counts"][package].keys():
                raise ValueError(f"Clippy baseline rule inventory changed: {package}")
            if any(type(count) is not int or count < 0 for count in values.values()):
                raise ValueError(f"Clippy baseline counts must be nonnegative integers: {package}")


class CargoDiagnostics:
    """Validate the build stream independently of Cargo's process exit status."""

    def __init__(self, output, clippy=None):
        self.output = output
        self.finished = False
        self.succeeded = False
        self.warnings = 0
        self.errors = 0
        self.invalid = False
        self.clippy = clippy

    def reject(self, explanation):
        self.invalid = True
        print(f"[rust-diagnostics] {explanation}", file=self.output, flush=True)

    def consume(self, line, number):
        try:
            event = json.loads(line, parse_constant=invalid_constant,
                               object_pairs_hook=unique_object)
        except (ValueError, TypeError):
            self.reject(f"invalid Cargo JSON on line {number}: {line.rstrip()}")
            return
        if not isinstance(event, dict):
            self.reject(f"Cargo message on line {number} must be an object")
            return
        if self.finished:
            self.reject(f"Cargo message after build-finished on line {number}")
        reason = event.get("reason")
        if reason == "compiler-message":
            message = event.get("message")
            if not isinstance(message, dict) or not isinstance(message.get("message"), str):
                self.reject(f"invalid compiler-message on line {number}")
                return
            level = message.get("level")
            if level == "warning":
                if self.clippy is None or not self.clippy.record(event):
                    self.warnings += 1
            elif level in ("error", "failure-note", "ice"):
                self.errors += 1
            elif level not in ("note", "help"):
                self.reject(f"unknown compiler diagnostic level on line {number}: {level!r}")
            rendered = message.get("rendered")
            if rendered is not None and not isinstance(rendered, str):
                self.reject(f"invalid rendered diagnostic on line {number}")
                return
            print(rendered or f"{level}: {message['message']}",
                  file=self.output, end="" if rendered and rendered.endswith("\n") else "\n",
                  flush=True)
        elif reason == "build-finished":
            self.finished = True
            if type(event.get("success")) is not bool:
                self.reject(f"invalid build-finished success on line {number}")
            else:
                self.succeeded = event["success"]
        elif reason not in ("compiler-artifact", "build-script-executed"):
            self.reject(f"unknown Cargo message reason on line {number}: {reason!r}")

    def result(self, returncode):
        if not self.finished:
            self.reject("Cargo did not emit build-finished")
        if returncode != 0 or not self.succeeded or self.invalid or self.warnings or self.errors:
            print("[rust-diagnostics] build rejected: "
                  f"Cargo exit {returncode}, success={self.succeeded}, "
                  f"{self.warnings} Rust warning(s), {self.errors} Rust error(s), "
                  f"invalid stream={self.invalid}", file=self.output, flush=True)
            return returncode if returncode > 0 else 1
        return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--clippy-scope", choices=("quality", "smb-sdk"),
                        help="opt in to the eight first-party T03 warning counts")
    parser.add_argument("--clippy-report", type=Path, help="write the observed CI counts as JSON")
    parser.add_argument("--capture-clippy-baseline", action="store_true",
                        help="capture counts for review; always return failure, never accept a baseline")
    parser.add_argument("command", nargs=argparse.REMAINDER,
                        help="-- cargo check/build/clippy/test --no-run --message-format=json ...")
    args = parser.parse_args()
    command = args.command
    if command[:1] == ["--"]:
        command = command[1:]
    if not command:
        parser.error("a Cargo build command is required")
    if bool(args.clippy_scope) != bool(args.clippy_report):
        parser.error("--clippy-scope and --clippy-report must be supplied together")
    if args.capture_clippy_baseline and not args.clippy_scope:
        parser.error("--capture-clippy-baseline requires an explicit Clippy scope")
    if args.clippy_scope and (command[:2] != ["cargo", "clippy"]
                             or "--message-format=json" not in command):
        parser.error("Clippy warning counts require cargo clippy --message-format=json")
    clippy = None
    if args.clippy_scope:
        try:
            root = Path(__file__).resolve().parents[2]
            write_clippy_report(clippy_report_path(root, args.clippy_report), None)
            metadata = subprocess.run(
                ["cargo", "metadata", "--locked", "--offline", "--no-deps", "--format-version", "1"],
                stdout=subprocess.PIPE, text=True, encoding="utf-8", check=True,
            )
            clippy = ClippyWarnings(root, args.clippy_scope,
                                   args.clippy_report, args.capture_clippy_baseline,
                                   command, read_json(metadata.stdout))
            clippy.write_report(False)
        except (OSError, ValueError, TypeError, KeyError, subprocess.CalledProcessError) as error:
            print(f"[clippy-warnings] could not prepare measurement: {error}", file=sys.stderr)
            return 1
    diagnostics = CargoDiagnostics(sys.stderr, clippy)
    try:
        # Cargo progress and build-script output remain on stderr. Only Cargo's
        # JSON stdout is decoded; rendered Rust diagnostics are printed above.
        with subprocess.Popen(command, stdout=subprocess.PIPE, text=True,
                              encoding="utf-8") as process:
            try:
                for number, line in enumerate(process.stdout, 1):
                    diagnostics.consume(line, number)
            except UnicodeError:
                diagnostics.reject("Cargo stdout is not valid UTF-8")
                process.terminate()
            except KeyboardInterrupt:
                process.terminate()
                return 130
            result = diagnostics.result(process.wait())
            return clippy.finish(result, sys.stderr) if clippy else result
    except OSError as error:
        print(f"[rust-diagnostics] could not run Cargo command: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
