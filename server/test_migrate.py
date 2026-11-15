import copy
import json
import tempfile
import unittest
from contextlib import closing
from pathlib import Path
from unittest.mock import patch

from . import db, migrate as m


class Source:
    def __init__(self):
        self.docs = {}
        self.reads = []

    def team(self, tid, code, name="운영 팀"):
        self.docs.update({
            f"joinCodes/{code}": {"teamId": tid}, f"teams/{tid}": {"joinCode": code, "name": name},
            f"teams/{tid}/members/owner": {"displayName": "개설자"},
            f"teams/{tid}/members/peer": {"displayName": "받는 사람"},
            f"teams/{tid}/shares/m1": {"text": "팀 약속", "by": "owner"},
            f"teams/{tid}/projects/p/tasks/t": {"status": "done"},
            f"teams/{tid}/signals/r/private/body": {"text": "받을 팀원에게만", "toMember": "peer"},
            f"teams/{tid}/signals/r/owner/sender": {"uid": "owner"}})

    def snapshot_time(self, path):
        self.snapshot = copy.deepcopy(self.docs)
        return "2026-11-15T04:29:00.123456Z"

    def document(self, path, stamp):
        self.reads.append((path, stamp))
        return self.snapshot.get(path)

    def descendants(self, path, stamp):
        self.reads.append((path + "/", stamp))
        yield from ((k, v) for k, v in self.snapshot.items() if k.startswith(path + "/"))


class TestMigrate(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.file = Path(tmp.name) / "host.sqlite"
        self.src = Source()
        self.src.team("one", "ABC234")
        self.src.team("two", "DEF234")

    def local(self):
        with closing(db.connect(self.file)) as conn:
            return {path: json.loads(fields) for path, fields in conn.execute("SELECT path,fields FROM docs")}

    def test_only_selected_team_including_descendants_and_codes(self):
        out = m.migrate(self.file, ["abc234", "id:one"], self.src)
        self.assertEqual(len(out), 1)
        expected = {k: v for k, v in self.src.docs.items() if k.startswith("teams/one") or k == "joinCodes/ABC234"}
        self.assertEqual(expected, self.local())
        self.assertEqual(len(set(stamp for _, stamp in self.src.reads)), 1)
        self.assertFalse(any("two" in path for path, _ in self.src.reads))

    def test_rerun_preserves_local_changes_and_deleted_rows(self):
        m.migrate(self.file, ["one"], self.src)
        with closing(db.connect(self.file)) as conn:
            conn.execute("BEGIN IMMEDIATE")
            db.put_document(conn, "teams/one", {"closed": True})
            conn.execute("DELETE FROM docs WHERE path='teams/one/shares/m1'")
            conn.commit()
        before = self.local()
        with patch.object(self.src, "snapshot_time", side_effect=AssertionError):
            out = m.migrate(self.file, ["ABC234"], self.src)
        self.assertIn("이미 이전", out[0])
        self.assertEqual(before, self.local())

    def test_failure_for_second_team_rolls_back_first_and_preserves_server(self):
        db.initialize(self.file)
        with closing(db.connect(self.file)) as conn:
            conn.execute("BEGIN IMMEDIATE")
            db.put_document(conn, "teams/two/archive/orphan", {"body": "existing data"})
            conn.commit()
        before = self.local()
        with self.assertRaises(m.MigrationError):
            m.migrate(self.file, ["one", "two"], self.src)
        self.assertEqual(before, self.local())
        with closing(db.connect(self.file)) as conn:
            self.assertEqual(conn.execute("SELECT COUNT(*) FROM imported_teams").fetchone()[0], 0)

    def test_failure_mid_copy_writes_nothing(self):
        def broken(path, stamp):
            yield path + "/members/one", {"uid": "one"}
            raise m.MigrationError("Firestore 429")
        with patch.object(self.src, "descendants", side_effect=broken), self.assertRaises(m.MigrationError):
            m.migrate(self.file, ["one", "two"], self.src)
        self.assertEqual(self.local(), {})
        m.migrate(self.file, ["one", "two"], self.src)
        self.assertEqual(self.local(), self.src.docs)

    def test_closed_tests_and_empty_are_excluded(self):
        self.src.docs["teams/one"] = {"closed": True}
        self.src.docs["joinCodes/ABC234"] = {"closed": True}
        self.src.docs["teams/two"]["name"] = "사이 점검 팀"
        self.src.team("empty", "GHR234")
        for key in [key for key in self.src.docs if key.startswith("teams/empty/members/")]:
            del self.src.docs[key]
        m.migrate(self.file, ["one", "ABC234", "two", "empty"], self.src)
        self.assertEqual(self.local(), {})

    def test_pagination_typed_fields_and_scoping(self):
        source = m.Firestore(m.PROJECT)
        prefix = source.root + "/teams/one/"
        docs = [{"document": {"name": prefix + "members/" + uid,
                              "fields": {"n": {"integerValue": "8"}, "note": {"stringValue": "한글"}}}}
                for uid in ("a", "b", "c")]
        with patch.object(source, "request", side_effect=[docs[:2], docs[2:]]) as request, patch.object(m, "PAGE_SIZE", 2):
            self.assertEqual(list(source.descendants("teams/one", "fixed")),
                             [("teams/one/members/" + uid, {"n": 8, "note": "한글"}) for uid in ("a", "b", "c")])
            path, body = request.call_args_list[-1].args
            self.assertEqual(path, "/teams/one:runQuery")
            self.assertEqual(body["readTime"], "fixed")
            self.assertEqual(body["structuredQuery"]["startAt"]["values"][0]["referenceValue"],
                             prefix + "members/b")
        outside = [{"document": {"name": source.root + "/teams/other/members/u"}}]
        with patch.object(source, "request", return_value=outside), self.assertRaises(m.MigrationError):
            list(source.descendants("teams/one", "fixed"))

    def test_unknown_field_type_does_not_silently_drop_data(self):
        with self.assertRaises(m.MigrationError):
            m.plain_fields({"fields": {"unknown": {"bytesValue": "YWJj"}}})


if __name__ == "__main__":
    unittest.main()
