"""Run the real maintenance fixture within its dedicated targeted-policy time budget."""
import importlib.util
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    'engineering_guidance_maintenance',
    ROOT / 'test/tools/rust-sources/test_legacy_http_maintenance_integration.py')
MAINTENANCE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MAINTENANCE)
import test_source_layout as PROJECTION


def load_tests(loader, tests, pattern):
    return unittest.TestSuite([
        loader.loadTestsFromTestCase(PROJECTION.HappyPaths),
        loader.loadTestsFromTestCase(PROJECTION.ProjectionIsolation),
        loader.loadTestsFromTestCase(MAINTENANCE.HttpMaintenanceManager),
    ])
