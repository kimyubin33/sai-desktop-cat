"""Regressions for role, privacy and transaction boundaries on the desktop host."""
import unittest

from server.policy import can_list, can_read, can_write


TEAM = "teams/team1"
CODE = "joinCodes/BCDF56"
OWNER, MEMBER, PEER = "owner", "member", "peer"
ROOT = {"joinCode": "BCDF56", "name": "개발팀", "createdBy": OWNER}
PROJECT = f"{TEAM}/projects/app"


class RulesTest(unittest.TestCase):
    def setUp(self):
        self.docs = {
            TEAM: ROOT.copy(),
            CODE: {"teamId": "team1", "teamName": "개발팀"},
            PROJECT: {"title": "프로젝트", "createdBy": MEMBER},
        }
        for user in (OWNER, MEMBER, PEER):
            self.docs[f"{TEAM}/members/{user}"] = {"displayName": user, "nameKey": user, "joinCode": "BCDF56"}
            self.docs[f"{TEAM}/names/{user}"] = {"uid": user}

    def authorize(self, uid, writes):
        def get_after(path):
            return writes.get(path, self.docs.get(path))

        return all(can_write(uid, path, self.docs.get(path), doc, self.docs.get, get_after)
                   for path, doc in writes.items())

    def commit(self, uid, writes):
        self.assertTrue(self.authorize(uid, writes))
        for path, doc in writes.items():
            if doc is None:
                self.docs.pop(path, None)
            else:
                self.docs[path] = doc

    def test_join_and_rename_require_matching_name_in_same_transaction(self):
        who = "first-timer"
        member = f"{TEAM}/members/{who}"
        name = f"{TEAM}/names/new"
        entry = {"displayName": "새 팀원", "nameKey": "new", "joinCode": "BCDF56"}
        self.assertFalse(self.authorize(who, {member: entry}))
        self.assertFalse(self.authorize(who, {member: dict(entry, joinCode="unknown"), name: {"uid": who}}))
        self.assertFalse(self.authorize(who, {member: dict(entry, nameKey=f"../members/{who}"), name: {"uid": who}}))
        self.assertFalse(can_list(who, f"{TEAM}/members", self.docs.get))
        self.commit(who, {member: entry, name: {"uid": who}})
        self.assertTrue(can_list(who, f"{TEAM}/members", self.docs.get))
        self.assertFalse(self.authorize(PEER, {member: dict(entry, displayName="바꿈")}))
        self.assertFalse(self.authorize(who, {member: dict(entry, joinCode="other")}))
        self.commit(who, {member: dict(entry, displayName="동료", nameKey="new2"),
                          f"{TEAM}/names/new2": {"uid": who}})
        self.commit(who, {name: None, member: None})

    def test_only_recipient_sees_inbox_only_participants_read_private_body(self):
        signal = f"{TEAM}/signals/s1"
        body = signal + "/private/body"
        sender = signal + "/owner/sender"
        header = {"topic": "workload", "toMember": MEMBER, "week": "2026-W46", "held": False}
        message = {"toMember": MEMBER, "title": "시간", "text": "개인 문안"}
        writes = {signal: header, body: message, sender: {"uid": OWNER}}
        self.assertFalse(self.authorize(PEER, writes))
        self.assertFalse(self.authorize(OWNER, {**writes, body: dict(message, toMember=PEER)}))
        self.commit(OWNER, writes)
        self.assertTrue(can_list(MEMBER, f"{TEAM}/signals", self.docs.get, [("toMember", "EQUAL", MEMBER)]))
        self.assertFalse(can_list(PEER, f"{TEAM}/signals", self.docs.get, [("toMember", "EQUAL", MEMBER)]))
        self.assertFalse(can_list(MEMBER, f"{TEAM}/signals", self.docs.get))
        self.assertTrue(can_read(OWNER, body, self.docs.get))
        self.assertTrue(can_read(MEMBER, body, self.docs.get))
        self.assertFalse(can_read(PEER, body, self.docs.get))
        self.assertTrue(can_read(OWNER, sender, self.docs.get))
        self.assertFalse(can_read(MEMBER, sender, self.docs.get))
        self.assertFalse(can_list(OWNER, signal + "/private", self.docs.get))
        self.assertFalse(can_list(OWNER, signal + "/owner", self.docs.get))
        self.assertFalse(self.authorize(OWNER, {signal: None}))
        self.assertFalse(self.authorize(OWNER, {signal: dict(header, held=True)}))
        self.commit(MEMBER, {signal: dict(header, held=True)})
        deletes = {key: None for key in writes}
        self.assertFalse(self.authorize(MEMBER, deletes))
        self.commit(OWNER, deletes)

    def test_proposal_votes_and_withdrawal(self):
        proposal, share = f"{TEAM}/proposals/vote", f"{TEAM}/shares/vote"
        vote = {"topic": "ai_promise", "week": "2026-W46", "responses": {}}
        self.assertFalse(self.authorize(MEMBER, {share: {"by": PEER, "text": "..."}}))
        self.commit(MEMBER, {proposal: vote, share: {"by": MEMBER, "text": "생각을 알려 주세요."}})
        self.assertFalse(self.authorize(OWNER, {proposal: dict(vote, responses={MEMBER: "agree"})}))
        self.commit(PEER, {proposal: dict(vote, responses={PEER: "agree"})})
        self.assertFalse(self.authorize(MEMBER, {proposal: None}))
        self.assertFalse(self.authorize(PEER, {share: None, proposal: None}))
        self.commit(MEMBER, {share: None, proposal: None})

    def test_old_projects_and_admin_deletion(self):
        self.docs[PROJECT] = {"title": "구형 프로젝트"}
        self.assertFalse(self.authorize(MEMBER, {PROJECT: {"title": "구형 프로젝트", "createdBy": MEMBER}}))
        self.assertFalse(self.authorize(MEMBER, {PROJECT: dict(self.docs[PROJECT], deleting=True)}))
        admin = f"{TEAM}/admins/{OWNER}"
        cfg = f"{TEAM}/admin/config"
        key = f"{TEAM}/adminKeys/derived_key"
        self.assertFalse(can_list(OWNER, f"{TEAM}/adminKeys", self.docs.get))
        self.assertFalse(self.authorize(OWNER, {admin: {"key": "derived_key"}}))
        self.commit(OWNER, {cfg: {"adminId": "root", "salt": "salt", "by": OWNER},
                            key: {"v": 1}, admin: {"key": "derived_key"}})
        self.assertFalse(self.authorize(PEER, {cfg: {"adminId": "other", "salt": "", "by": PEER}}))
        self.assertFalse(self.authorize(PEER, {f"{TEAM}/adminKeys/new": {"v": 1}}))
        self.assertFalse(can_read(PEER, admin, self.docs.get))
        self.commit(OWNER, {PROJECT: dict(self.docs[PROJECT], deleting=True)})
        self.assertFalse(self.authorize(MEMBER, {f"{PROJECT}/tasks/new": {"title": "", "status": "todo"}}))
        self.commit(OWNER, {PROJECT: None})

    def test_task_and_file_ownership_and_brief_import_limits(self):
        task, file, brief = f"{PROJECT}/tasks/1", f"{PROJECT}/files/1", f"{PROJECT}/briefs/current"
        job = {"title": "일", "status": "todo", "assignee": PEER, "accepted": False}
        self.commit(MEMBER, {task: job})
        self.assertFalse(self.authorize(MEMBER, {task: dict(job, accepted=True)}))
        self.commit(PEER, {task: dict(job, accepted=True)})
        self.commit(MEMBER, {task: dict(job, accepted=True, title="변경")})
        self.assertFalse(self.authorize(OWNER, {task: dict(job, accepted=True, assignee=MEMBER)}))
        self.commit(PEER, {file: {"by": PEER, "name": "기획서", "url": "https://example.test/doc"}})
        self.assertFalse(self.authorize(MEMBER, {file: None}))
        doc = {"name": "기획.MD", "text": "가" * 16384, "updatedAt": 100, "by": PEER}
        self.commit(PEER, {brief: doc})
        self.assertFalse(self.authorize(PEER, {brief: dict(doc, text=doc["text"] + "가")}))
        self.assertFalse(self.authorize(PEER, {brief: dict(doc, name="기획.html")}))
        self.assertFalse(self.authorize(MEMBER, {brief: None}))
        self.commit(MEMBER, {PROJECT: dict(self.docs[PROJECT], deleting=True)})
        self.commit(MEMBER, {file: None, task: None, brief: None})
        self.commit(MEMBER, {PROJECT: None})
        self.assertFalse(self.authorize(PEER, {brief: doc}))

    def test_integration_keys_and_telegram_links(self):
        claude, telegram = f"{TEAM}/integrations/claude", f"{TEAM}/telegram/{MEMBER}"
        self.commit(OWNER, {claude: {"apiKey": "fixture", "sharedBy": OWNER}})
        self.assertFalse(self.authorize(MEMBER, {claude: {"apiKey": "spoof", "sharedBy": MEMBER}}))
        self.assertFalse(self.authorize(MEMBER, {claude: None}))
        self.assertFalse(self.authorize(OWNER, {telegram: {"chatId": 3, "teamAlerts": True}}))
        self.commit(MEMBER, {telegram: {"chatId": 3, "teamAlerts": True}})
        self.assertFalse(can_read("outsider", claude, self.docs.get))
        self.assertTrue(can_read(MEMBER, claude, self.docs.get))

    def test_only_owner_can_close_and_access_ends_even_for_owner(self):
        closure = {TEAM: {"closed": True}, CODE: {"closed": True}}
        self.assertFalse(self.authorize(OWNER, {TEAM: {"closed": True}}))
        self.assertFalse(self.authorize(OWNER, {CODE: {"closed": True}}))
        self.assertFalse(self.authorize(MEMBER, closure))
        self.assertFalse(self.authorize(OWNER, {TEAM: {"closed": 1}, CODE: {"closed": True}}))
        self.commit(OWNER, closure)
        self.assertTrue(can_read("outsider", CODE, self.docs.get))
        for user in (OWNER, MEMBER):
            self.assertFalse(can_read(user, TEAM, self.docs.get))
            self.assertFalse(can_read(user, f"{TEAM}/members/{user}", self.docs.get))
            self.assertFalse(can_list(user, f"{TEAM}/shares", self.docs.get))
            self.assertFalse(self.authorize(user, {f"{TEAM}/members/{user}": None}))
        self.assertFalse(self.authorize(OWNER, {TEAM: ROOT}))

    def test_unknown_paths_and_unauthenticated_requests(self):
        for path in ("joinCodes", "teams", f"{TEAM}/admin", f"{TEAM}/new", f"{TEAM}/projects/app/briefs"):
            self.assertFalse(can_list(OWNER, path, self.docs.get))
        for path in (f"{TEAM}/admin/other", f"{TEAM}/admins/{PEER}", f"{TEAM}/shares/x/more/y"):
            self.assertFalse(can_read(OWNER, path, self.docs.get))
            self.assertFalse(self.authorize(OWNER, {path: {"text": "foo"}}))
        self.assertFalse(can_read("", CODE, self.docs.get))
        self.assertFalse(can_list(None, f"{TEAM}/members", self.docs.get))
        self.assertFalse(self.authorize("outsider", {f"{TEAM}/shares/news": {"by": "outsider", "text": "hi"}}))


class ImportScopeTest(unittest.TestCase):
    """Use migrated records and the same request/SQLite layer as the public service."""

    def test_migrated_room_owner_and_page_boundaries(self):
        import tempfile
        from contextlib import closing
        from pathlib import Path
        from server import db
        from server.api import Documents, RequestError

        with tempfile.TemporaryDirectory() as directory:
            filename = Path(directory) / "rooms.db"
            db.initialize(filename)
            with closing(db.connect(filename)) as conn, conn:
                conn.execute("BEGIN IMMEDIATE")
                for team, admin in (("mine", OWNER), ("other", PEER)):
                    root = "teams/" + team
                    db.put_document(conn, root, {"createdBy": admin, "name": team, "joinCode": team.upper()})
                    db.put_document(conn, f"{root}/members/{admin}", {"displayName": admin})
                    for key in ("하나", "둘", "셋"):
                        db.put_document(conn, f"{root}/shares/{key}", {"by": admin, "text": team})
                db.put_document(conn, "teams/mine/members/member", {"displayName": MEMBER})
                db.put_document(conn, "teams/closed", {"closed": True})
                db.put_document(conn, f"teams/closed/members/{OWNER}", {"displayName": OWNER})

                owner = Documents(conn, OWNER, "saigajoa", restrict_creation=True)
                member = Documents(conn, MEMBER, "saigajoa", restrict_creation=True)
                self.assertTrue(db.can_create_team(conn, OWNER))
                self.assertFalse(db.can_create_team(conn, MEMBER))
                with self.assertRaises(RequestError) as caught:
                    member.route("PATCH", "/teams/mine", {},
                                 {"fields": {"createdBy": {"stringValue": MEMBER}}})
                self.assertEqual(caught.exception.code, 403)
                self.assertEqual(owner.read("teams/mine")["fields"]["createdBy"]["stringValue"], OWNER)
                with self.assertRaises(RequestError) as caught:
                    member.route("POST", "/teams", {"documentId": ["new"]}, {"fields": {
                        "createdBy": {"stringValue": MEMBER},
                        "joinCode": {"stringValue": "ABCDEF"},
                        "name": {"stringValue": "새 팀"},
                    }})
                self.assertEqual(caught.exception.code, 403)

                first = owner.list("teams/mine/shares", {"pageSize": ["2"]})
                second = owner.list("teams/mine/shares", {"pageSize": ["2"], "pageToken": [first["nextPageToken"]]})
                names = [doc["name"].split("/")[-1] for doc in first["documents"] + second["documents"]]
                self.assertEqual(names, sorted(["하나", "둘", "셋"]))
                self.assertNotIn("nextPageToken", second)
                for other in ("teams/other/shares", "teams/closed/shares"):
                    with self.assertRaises(RequestError) as caught:
                        owner.list(other, {"pageToken": [first["nextPageToken"]]})
                    self.assertEqual(caught.exception.code, 403)


if __name__ == "__main__":
    unittest.main()
