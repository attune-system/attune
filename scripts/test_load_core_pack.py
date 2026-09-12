#!/usr/bin/env python3

import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path
from types import ModuleType


SCRIPT = Path(__file__).with_name("load_core_pack.py")
PSYCOPG2 = ModuleType("psycopg2")
PSYCOPG2.sql = ModuleType("psycopg2.sql")
PSYCOPG2.extras = ModuleType("psycopg2.extras")
sys.modules["psycopg2"] = PSYCOPG2
sys.modules["psycopg2.sql"] = PSYCOPG2.sql
sys.modules["psycopg2.extras"] = PSYCOPG2.extras
SPEC = importlib.util.spec_from_file_location("load_core_pack", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class FakeCursor:
    def __init__(self):
        self.query = None
        self.parameters = None

    def execute(self, query, parameters):
        self.query = query
        self.parameters = parameters

    def fetchone(self):
        return (7,)

    def close(self):
        pass


class FakeConnection:
    def __init__(self):
        self.cursor_instance = FakeCursor()

    def cursor(self):
        return self.cursor_instance


class PackStoragePathTest(unittest.TestCase):
    def test_upsert_persists_explicit_storage_path(self):
        with tempfile.TemporaryDirectory() as directory:
            packs_dir = Path(directory)
            pack_dir = packs_dir / "core"
            pack_dir.mkdir()
            (pack_dir / "pack.yaml").write_text(
                "ref: core\nlabel: Core\nversion: 1.0.0\n"
            )
            loader = MODULE.PackLoader(
                "postgresql://unused",
                packs_dir,
                "core",
                storage_path=pack_dir,
            )
            loader.conn = FakeConnection()

            loader.upsert_pack()

            cursor = loader.conn.cursor_instance
            self.assertIn("storage_path", cursor.query)
            self.assertIn("pack.active_release IS NULL", cursor.query)
            self.assertEqual(cursor.parameters[-1], str(pack_dir))


if __name__ == "__main__":
    unittest.main()
