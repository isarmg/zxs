"""Run the dynamically discovered fast suite; reject an empty suite."""
from pathlib import Path
import sys
import unittest


def main():
    directory = Path(__file__).resolve().parents[2] / "test/publication-model"
    suite = unittest.defaultTestLoader.discover(str(directory), pattern="test_*.py")
    if suite.countTestCases() == 0:
        print("No publication model self-tests discovered.", file=sys.stderr)
        return 2
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    return 0 if result.wasSuccessful() else 1


if __name__ == "__main__":
    raise SystemExit(main())
