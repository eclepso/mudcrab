import importlib.util
from pathlib import Path
import sqlite3
import unittest


SPEC = importlib.util.spec_from_file_location("form_key_identity", Path(__file__).parents[1] / "form_key_identity.py")
IDENTITY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(IDENTITY)


class IdentityTests(unittest.TestCase):
    def fixture(self):
        database = sqlite3.connect(":memory:")
        self.addCleanup(database.close)
        database.executescript("""CREATE TABLE pack_identity(singleton INTEGER,version INTEGER,digest BLOB);
            CREATE TABLE pack_plugins(id INTEGER,normalized_name TEXT,display_name TEXT,loaded INTEGER,
            flags INTEGER,header_version_bits INTEGER,masters_json TEXT,content_hash BLOB,supplier_priority INTEGER);""")
        database.execute("INSERT INTO pack_identity VALUES(1,1,?)", (bytes.fromhex("e17fc06e761bece0a527c54d7ef1e91ba9e3265b0df5bf875e52d13e06b95d01"),))
        database.executemany("INSERT INTO pack_plugins VALUES(?,?,?,?,?,?,?,?,?)", [
            (1, "alpha.esl", "Alpha.esl", 1, 2, None, "[]", b"\x11" * 32, 1),
            (2, "skyrim.esm", "Skyrim.esm", 1, 1, None, "[]", b"\x22" * 32, 0),
        ])
        return database

    def test_allocation_is_authenticated_and_distinct_from_supplier_priority(self):
        identity = IDENTITY.PackIdentity(self.fixture())
        self.assertEqual(identity.stored_key("3c:SKYRIM.ESM"), 33554492)
        self.assertEqual(identity.stored_key("abc:Alpha.esl"), 16779964)
        with self.assertRaisesRegex(ValueError, "absent"):
            identity.stored_key("3c:Other.esm")
        with self.assertRaisesRegex(ValueError, "numeric"):
            identity.stored_key(33554492)

    def test_changed_allocation_priority_and_source_proof_are_refused(self):
        for mutation in ("UPDATE pack_plugins SET content_hash=zeroblob(32) WHERE id=1",
                         "UPDATE pack_plugins SET supplier_priority=9 WHERE id=1",
                         "UPDATE pack_plugins SET display_name='Different.esl' WHERE id=1",
                         "UPDATE pack_plugins SET id=256 WHERE id=2"):
            with self.subTest(mutation=mutation):
                database = self.fixture()
                database.execute(mutation)
                with self.assertRaises(ValueError):
                    IDENTITY.PackIdentity(database)

    def test_old_world_and_numeric_documents_require_regeneration(self):
        database = sqlite3.connect(":memory:")
        self.addCleanup(database.close)
        with self.assertRaisesRegex(ValueError, "convert again"):
            IDENTITY.PackIdentity(database)
        for value in (60, 73014446780, "73014446780", "3c:../Skyrim.esm"):
            with self.assertRaises(ValueError):
                IDENTITY.named(value)
        with self.assertRaisesRegex(ValueError, "regenerated"):
            IDENTITY.require_schema({"schema_version": 1})


if __name__ == "__main__":
    unittest.main()
