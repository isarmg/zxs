"""Run the existing real source-maintenance fixture for the engineering guide rename."""
import importlib.util
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    'engineering_guidance_maintenance',
    ROOT / 'test/tools/rust-sources/test_legacy_http_maintenance_integration.py')
MAINTENANCE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MAINTENANCE)


def load_tests(loader, tests, pattern):
    return loader.loadTestsFromTestCase(MAINTENANCE.HttpMaintenanceManager)
