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
        self.executions = []

    def execute(self, query, parameters):
        self.query = query
        self.parameters = parameters
        self.executions.append((query, parameters))

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


class PackRuleReconciliationTest(unittest.TestCase):
    def test_missing_rules_directory_disables_pack_owned_rules(self):
        with tempfile.TemporaryDirectory() as directory:
            packs_dir = Path(directory)
            pack_dir = packs_dir / "core"
            pack_dir.mkdir()
            loader = MODULE.PackLoader("postgresql://unused", packs_dir, "core")
            loader.conn = FakeConnection()
            loader.pack_id = 7
            loader.pack_ref = "core"

            self.assertEqual(loader.upsert_rules({}, {}), {})

            query, parameters = loader.conn.cursor_instance.executions[-1]
            self.assertIn("UPDATE rule", query)
            self.assertIn("enabled = false", query)
            self.assertIn("is_adhoc = false", query)
            self.assertEqual(parameters, (7,))

    def test_yml_rule_is_kept_during_reconciliation(self):
        with tempfile.TemporaryDirectory() as directory:
            packs_dir = Path(directory)
            rules_dir = packs_dir / "core" / "rules"
            rules_dir.mkdir(parents=True)
            (rules_dir / "kept.yml").write_text(
                "ref: kept\ntrigger_ref: event\naction_ref: run\n"
            )
            loader = MODULE.PackLoader("postgresql://unused", packs_dir, "core")
            loader.conn = FakeConnection()
            loader.pack_id = 7
            loader.pack_ref = "core"

            loader.registration_identity = 123
            rule_ids = loader.upsert_rules(
                {"core.event": 11},
                {"core.run": 12},
            )

            query, parameters = loader.conn.cursor_instance.executions[-1]
            self.assertEqual(rule_ids, {"core.kept": 7})
            self.assertIn("ref != ALL(%s)", query)
            self.assertEqual(parameters, (7, ["core.kept"]))
            inserts = [(query, parameters) for query, parameters in loader.conn.cursor_instance.executions
                       if "INSERT INTO rule" in query]
            self.assertEqual(len(inserts), 1)
            self.assertIn("owner_identity", inserts[0][0])
            self.assertEqual(inserts[0][1][-1], 123)


if __name__ == "__main__":
    unittest.main()
