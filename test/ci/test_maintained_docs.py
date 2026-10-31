"""Keep maintained deployment, entrypoint and SQLite examples aligned with their contracts."""
import configparser
from pathlib import Path, PurePosixPath
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]


class MaintainedDocs(unittest.TestCase):
    def test_samba_export_is_consistent_and_outside_private_service_directories(self):
        samba = configparser.ConfigParser(interpolation=None, delimiters=('=',))
        samba.read(ROOT / 'deploy/samba/smb.conf.example')
        export = PurePosixPath(samba['zxs']['path'])
        self.assertTrue(export.is_absolute())
        self.assertNotIn('..', export.parts)

        text = (ROOT / 'docs/components/smb.md').read_text()
        self.assertEqual(re.findall(r'^\s*path\s*=\s*(/\S+)\s*$', text, re.MULTILINE),
                         [str(export)])
        self.assertIn(f'`{export}`', (ROOT / 'docs/operations/deploy/systemd.md').read_text())

        unit = configparser.ConfigParser(interpolation=None, delimiters=('=',))
        unit.read(ROOT / 'deploy/systemd/zxs.service')
        service = unit['Service']
        for setting, base in (('StateDirectory', '/var/lib'), ('RuntimeDirectory', '/run')):
            private = PurePosixPath(base, service[setting])
            with self.subTest(directory=private):
                self.assertFalse(export.is_relative_to(private))
                self.assertFalse(private.is_relative_to(export))
                self.assertEqual(service[f'{setting}Mode'], '0700')

    def test_public_listen_default_is_consistent_in_deployment_docs(self):
        unit = configparser.ConfigParser(interpolation=None, delimiters=('=',))
        unit.read(ROOT / 'deploy/systemd/zxs.service')
        self.assertIn('--listen 0.0.0.0:7400', unit['Service']['ExecStart'])
        for relative in ('docs/components/server.md', 'docs/getting-started.md',
                         'docs/operations/deploy/systemd.md', 'docs/21-configuration.md'):
            with self.subTest(document=relative):
                text = (ROOT / relative).read_text()
                self.assertIn('0.0.0.0:7400', text)
                self.assertNotIn('HTTP 监听地址必须是回环地址', text)
                self.assertNotIn('默认只监听本机', text)
        client = (ROOT / 'docs/components/client.md').read_text()
        self.assertIn('不限制 HTTP 主机为回环', client)
        self.assertIn('HTTPS 仍验证服务器证书', client)

    def test_engineering_entrypoints_and_test_layout(self):
        text = (ROOT / 'docs/20-engineering.md').read_text()
        self.assertNotRegex(text, r'\bzxs offline-(?:check|export)\b')
        for command in ('offline-check', 'offline-export'):
            self.assertIn(f'`zxs-rescue {command} ', text)
        self.assertNotIn('`tests/`', text)
        self.assertNotIn('模块测试靠近实现', text)
        self.assertIn('根 `test/`', text)
        self.assertNotIn('admin/', text)
        self.assertIn('rescue/', text)
        rescue = (ROOT / 'src/rescue/src/main.rs').read_text()
        for variant in ('OfflineCheck', 'OfflineExport'):
            self.assertIn(f'    {variant} {{', rescue)
        self.assertNotIn('本机 zxs → 服务启动 / 初始化', text)

    def test_sqlite_type_examples_are_supported_and_compile_checked(self):
        text = (ROOT / 'src/foundation/data-storage/sqlite-access/src/query_engine/types/mod.rs').read_text()
        for removed in ('query!', 'query_as!', 'PgHasArrayType', '#[sqlx(',
                        '#[derive(sqlite_access::Type)]', 'PostgreSQL', 'postgres'):
            self.assertNotIn(removed, text)
        self.assertNotRegex(text, r'```rust,(?:ignore|text)')
        for trait in ('Type<Sqlite>', "Encode<'q, Sqlite>", "Decode<'r, Sqlite>"):
            self.assertIn(trait + ' for UserId', text)
        self.assertIn('sqlite_access::query_scalar("SELECT ?")', text)
        self.assertIn('.bind(UserId(42))', text)


if __name__ == '__main__':
    unittest.main()
