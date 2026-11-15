"""HTTP and SQLite guarantees that affect a desktop host during normal use."""

import contextlib
import http.client
import json
import tempfile
import unittest
from pathlib import Path

from server import db
from server.api import create_server
from server.tests.client_contract import LocalAccounts, listening


DOCUMENTS = "/v1/projects/saigajoa/databases/(default)/documents"


def fields(data):
    return {"fields": {name: {"stringValue": value} for name, value in data.items()}}


def request(port, method, path, token=None, payload=None, server_id=None):
    headers = {}
    if server_id:
        headers["X-Sai-Server-Id"] = server_id
    if token:
        headers["Authorization"] = "Bearer " + token
    if payload is not None:
        headers["Content-Type"] = "application/json"
    with contextlib.closing(http.client.HTTPConnection("127.0.0.1", port, timeout=10)) as conn:
        conn.request(method, path, json.dumps(payload) if payload is not None else None, headers)
        response = conn.getresponse()
        body = response.read()
        return response.status, json.loads(body) if body else None


class TeamHttpContract(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.path = Path(self.tmp.name) / "team.sqlite3"
        self.accounts = LocalAccounts()
        self.uid, self.token, _ = self.accounts.issue()
        db.initialize(self.path)

    def serve(self):
        return listening(create_server(("127.0.0.1", 0), self.path, self.accounts.verify))

    def seed_membership(self):
        with contextlib.closing(db.connect(self.path)) as conn:
            conn.execute("BEGIN")
            db.put_document(conn, "teams/t", {"createdBy": self.uid, "joinCode": "SAI123", "name": "우리 팀"})
            db.put_document(conn, f"teams/t/members/{self.uid}",
                            {"displayName": "작성자", "nameKey": "n-작성자", "joinCode": "SAI123"})
            conn.commit()

    def test_verified_author_and_persistent_history(self):
        self.seed_membership()
        with self.serve() as port:
            status, health = request(port, "GET", "/health")
            self.assertEqual(status, 200)
            self.assertEqual(health["service"], "sai-team-server")
            self.assertEqual(health["protocol"], 1)
            self.assertEqual(health["projectId"], "saigajoa")
            original_id = health["serverId"]
            self.assertTrue(original_id)
            self.assertEqual(request(port, "GET", DOCUMENTS + "/teams/t")[0], 401)
            self.assertEqual(request(port, "GET", DOCUMENTS + "/teams/t", self.uid)[0], 401)
            endpoint = DOCUMENTS + "/teams/t/shares?documentId=message"
            body = fields({"by": self.uid, "text": "서버를 다시 켜도 보이는 톡"})
            self.assertIn(request(port, "POST", endpoint, self.token, body)[0], (200, 201))
            duplicate = fields({"by": self.uid, "text": "중복 요청으로 교체하면 안 됨"})
            self.assertIn(request(port, "POST", endpoint, self.token, duplicate)[0], (403, 409))
        with self.serve() as port:
            self.assertEqual(request(port, "GET", "/health")[1]["serverId"], original_id)
            status, listing = request(port, "GET", DOCUMENTS + "/teams/t/shares", self.token, server_id=original_id)
            self.assertEqual(status, 200)
            self.assertEqual([row["fields"] for row in listing["documents"]], [body["fields"]])
            self.assertEqual(request(port, "GET", DOCUMENTS + "/teams/t/shares", self.token, server_id="other-host")[0], 409)

    def test_new_host_allows_only_configured_team_creators(self):
        other_uid, outside_token, _ = self.accounts.issue()
        root = "teams/first-team"
        code = "BCDF56"

        def bootstrap(who):
            docs = {
                root: {"createdBy": who, "joinCode": code, "name": "테스트 팀"},
                "joinCodes/" + code: {"teamId": "first-team", "teamName": "테스트 팀"},
                f"{root}/members/{who}": {"displayName": "개설자", "nameKey": "name", "joinCode": code},
                f"{root}/names/name": {"uid": who},
            }
            prefix = DOCUMENTS.removeprefix("/v1/").strip("/") + "/"
            return [{"update": {"name": prefix + path, **fields(data)}, "currentDocument": {"exists": False}}
                    for path, data in docs.items()]

        with self.serve() as port:
            endpoint = DOCUMENTS + ":commit"

            def post(token, writes):
                return request(port, "POST", endpoint, token, {"writes": writes})[0]

            valid = bootstrap(self.uid)
            self.assertEqual(post(self.token, valid), 403)
            with contextlib.closing(db.connect(self.path)) as conn:
                conn.execute("INSERT INTO host_creators(uid) VALUES (?)", (self.uid,))
            self.assertEqual(post(outside_token, bootstrap(other_uid)), 403)
            self.assertEqual(post(self.token, valid[:1]), 403)
            self.assertEqual(post(self.token, valid[:3]), 403)
            self.assertEqual(post(self.token, bootstrap(other_uid)), 403)
            with contextlib.closing(db.connect(self.path)) as conn:
                self.assertEqual(conn.execute("SELECT COUNT(*) FROM docs").fetchone()[0], 0)
            self.assertEqual(post(self.token, valid), 200)
            with contextlib.closing(db.connect(self.path)) as conn:
                self.assertEqual(conn.execute("SELECT COUNT(*) FROM docs").fetchone()[0], 4)
            self.assertEqual(request(port, "GET", DOCUMENTS + "/" + root, self.token)[0], 200)

    def test_batch_denial_rolls_back_earlier_document(self):
        self.seed_membership()
        team_prefix = "projects/saigajoa/databases/(default)/documents/teams/t/shares/"
        writes = [{"update": {"name": team_prefix + key, **fields({"by": author, "text": "회의 자료"})},
                   "currentDocument": {"exists": False}}
                  for key, author in (("ok", self.uid), ("other", "another-member"))]
        with self.serve() as port:
            status, _ = request(port, "POST", DOCUMENTS + ":commit", self.token, {"writes": writes})
            self.assertEqual(status, 403)
            status, listing = request(port, "GET", DOCUMENTS + "/teams/t/shares", self.token)
            self.assertEqual(status, 200)
            self.assertEqual(listing.get("documents", []), [])


if __name__ == "__main__":
    unittest.main()
