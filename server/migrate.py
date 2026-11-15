"""Copy explicitly selected Firestore teams into the desktop host's SQLite DB."""
import argparse
import json
import re
import shutil
import sqlite3
import subprocess
from contextlib import closing
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.parse import quote
from urllib.request import HTTPRedirectHandler, Request, build_opener

from . import db

PROJECT = "saigajoa"
CODE = re.compile("[A-HJ-NP-Z2-9]{6}")
AUTO_ID = re.compile("[A-Za-z0-9_-]{1,128}")
TEST_NAMES = {"사이 점검 팀", "AI 문안 점검", "해산 점검 팀"}
PAGE_SIZE = 100


class MigrationError(Exception):
    pass


class NoRedirect(HTTPRedirectHandler):
    # An administrator's Google access token is only for the fixed Google endpoint.
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def gcloud_token():
    executable = shutil.which("gcloud")
    if not executable:
        raise MigrationError("Google Cloud CLI를 설치한 뒤 gcloud auth login 으로 로그인해 주세요.")
    try:
        result = subprocess.run([executable, "auth", "print-access-token", "--quiet"],
                                capture_output=True, timeout=60, check=False)
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise MigrationError("Google Cloud CLI 로그인을 확인해 주세요.") from exc
    if result.returncode or not result.stdout.strip():
        raise MigrationError("gcloud auth login 으로 원본 프로젝트 읽기 권한이 있는 계정에 로그인해 주세요.")
    return result.stdout.decode("ascii").strip()


def team_selector(raw):
    value = raw.strip()
    if value.startswith("id:"):
        value = value[3:]
    elif CODE.fullmatch(value.upper()):
        return "joinCodes/" + value.upper()
    if not AUTO_ID.fullmatch(value):
        raise MigrationError("팀 이름 대신 참여 코드 또는 팀 ID를 입력해 주세요.")
    return "teams/" + value


class Firestore:
    def __init__(self, project):
        self.root = f"projects/{project}/databases/(default)/documents"
        self.endpoint = "https://firestore.googleapis.com/v1/" + self.root
        self.opener = build_opener(NoRedirect)
        self.token = None

    def request(self, suffix, body):
        if not self.token:
            self.token = gcloud_token()
        url = self.endpoint + suffix
        req = Request(url, data=json.dumps(body).encode(),
                      headers={"Authorization": "Bearer " + self.token,
                               "Content-Type": "application/json"})
        try:
            with self.opener.open(req, timeout=60) as response:
                return json.load(response)
        except HTTPError as exc:
            detail = {
                400: "원본 읽기 시점이 만료됐거나 요청이 거부됐습니다. 다시 시도해 주세요.",
                401: "gcloud auth login 으로 다시 로그인해 주세요.",
                403: "원본 프로젝트에서 Cloud Datastore Viewer 이상의 데이터 읽기 권한을 확인해 주세요.",
                429: "Firestore 할당량을 초과했습니다. 할당량이 회복된 뒤 같은 명령을 다시 실행해 주세요.",
            }.get(exc.code, "Firestore에 연결해 다시 시도해 주세요.")
            raise MigrationError(f"Firestore {exc.code}. {detail}") from exc
        except (OSError, URLError, ValueError) as exc:
            raise MigrationError("Firestore 응답을 읽지 못했습니다. 네트워크를 확인해 주세요.") from exc

    def snapshot_time(self, path):
        result = self.request(":batchGet", {"documents": [self.root + "/" + path]})
        if not isinstance(result, list) or len(result) != 1 or not result[0].get("readTime"):
            raise MigrationError("Firestore에서 읽기 기준 시각을 받지 못했습니다.")
        return result[0]["readTime"]

    def document(self, path, read_time):
        name = self.root + "/" + path
        rows = self.request(":batchGet", {"documents": [name], "readTime": read_time})
        if not isinstance(rows, list) or len(rows) != 1:
            raise MigrationError("원본 문서 응답이 올바르지 않습니다.")
        if rows[0].get("missing") == name:
            return None
        doc = rows[0].get("found", {})
        if doc.get("name") != name:
            raise MigrationError("원본 문서 경로가 요청과 달라 복사를 중단했습니다.")
        return plain_fields(doc)

    def descendants(self, team_path, read_time):
        # Same descendant selector and name cursor as Firebase CLI's recursive
        # traversal. This also finds records whose immediate parent was deleted.
        prefix = self.root + "/" + team_path + "/"
        query = {"from": [{"allDescendants": True}],
                 "orderBy": [{"field": {"fieldPath": "__name__"}, "direction": "ASCENDING"}],
                 "limit": PAGE_SIZE}
        while True:
            page = self.request("/" + quote(team_path, safe="/") + ":runQuery",
                                {"structuredQuery": query, "readTime": read_time})
            if not isinstance(page, list):
                raise MigrationError("원본 팀의 하위 문서 목록을 읽지 못했습니다.")
            docs = [entry["document"] for entry in page if "document" in entry]
            for doc in docs:
                name = doc.get("name", "")
                if not name.startswith(prefix):
                    raise MigrationError("선택한 팀 밖의 문서가 반환돼 복사를 중단했습니다.")
                yield name[len(self.root) + 1:], plain_fields(doc)
            if len(docs) < PAGE_SIZE:
                return
            query["startAt"] = {"values": [{"referenceValue": docs[-1]["name"]}], "before": False}


def plain_fields(doc):
    from .api import RequestError, decode_fields
    try:
        return decode_fields(doc.get("fields", {}))
    except (RequestError, ValueError, TypeError, KeyError) as exc:
        raise MigrationError("이 버전에서 옮길 수 없는 Firestore 문서 형식입니다. 복사를 중단했습니다.") from exc


def previous_import(conn, selector):
    collection, key = selector.split("/")
    column = "code" if collection == "joinCodes" else "team"
    return conn.execute(f"SELECT team FROM imported_teams WHERE {column}=?", (key,)).fetchone()


def resolve_team(source, selector, stamp):
    value = source.document(selector, stamp)
    if value is None:
        raise MigrationError(f"{selector}에 해당하는 팀을 찾을 수 없습니다.")
    if value.get("closed") is True:
        return None
    if selector.startswith("joinCodes/"):
        team = value.get("teamId", "")
        if not isinstance(team, str) or not AUTO_ID.fullmatch(team):
            raise MigrationError("참여 코드의 팀 ID가 올바르지 않습니다.")
        path = "teams/" + team
        root = source.document(path, stamp)
        mapping = value
        code = selector.split("/")[1]
    else:
        path = selector
        root = value
        code = root.get("joinCode", "")
        if root.get("name") in TEST_NAMES:
            return None
        if not isinstance(code, str) or not CODE.fullmatch(code):
            raise MigrationError("팀의 참여 코드가 올바르지 않습니다.")
        mapping = source.document("joinCodes/" + code, stamp)
    if not root or not mapping:
        raise MigrationError("원본 팀 문서와 참여 코드가 완전하지 않아 복사를 중단했습니다.")
    if root.get("closed") or mapping.get("closed") or root.get("name") in TEST_NAMES:
        return None
    if root.get("joinCode") != code or mapping.get("teamId") != path.split("/")[1]:
        raise MigrationError("원본 팀과 참여 코드가 서로 다릅니다. 복사를 중단했습니다.")
    return path, root, code, mapping


def migrate(destination, selectors, source):
    selectors = list(dict.fromkeys(team_selector(value) for value in selectors))
    if not selectors:
        raise MigrationError("옮길 팀의 참여 코드 또는 ID를 --team 으로 지정해 주세요.")
    destination = Path(destination).expanduser()
    destination.parent.mkdir(parents=True, exist_ok=True)
    db.initialize(destination)
    with closing(db.connect(destination)) as conn:
        conn.execute("""CREATE TABLE IF NOT EXISTS imported_teams(
                     team TEXT PRIMARY KEY, code TEXT UNIQUE NOT NULL,
                     source_time TEXT NOT NULL, documents INTEGER NOT NULL)""")
        conn.execute("PRAGMA temp_store=MEMORY")
        conn.execute("CREATE TEMP TABLE incoming(team TEXT, path TEXT PRIMARY KEY, fields TEXT NOT NULL)")
        messages = []
        pending = []
        for selector in selectors:
            if previous_import(conn, selector):
                messages.append(f"{selector}: 이미 이전한 팀입니다. 현재 자료를 유지했습니다.")
            else:
                pending.append(selector)
        if not pending:
            return messages
        stamp = source.snapshot_time(pending[0])
        teams = {}
        for selector in pending:
            resolved = resolve_team(source, selector, stamp)
            if not resolved:
                messages.append(f"{selector}: 해산한 팀 또는 자동 점검 팀이어서 건너뛰었습니다.")
                continue
            path, root, code, mapping = resolved
            if path in teams:
                continue
            team = path.split("/")[1]

            def stage(doc_path, fields):
                encoded = json.dumps(fields, allow_nan=False, ensure_ascii=False, separators=(",", ":"))
                conn.execute("INSERT INTO incoming VALUES (?,?,?)", (team, doc_path, encoded))

            stage(path, root)
            stage("joinCodes/" + code, mapping)
            member = False
            for doc_path, fields in source.descendants(path, stamp):
                if not doc_path.startswith(path + "/") or len(doc_path.split("/")) % 2:
                    raise MigrationError("팀 범위 밖이거나 구조가 잘못된 문서 경로입니다.")
                stage(doc_path, fields)
                member |= doc_path.startswith(path + "/members/") and len(doc_path.split("/")) == 4
            if not member:
                messages.append(f"{team}: 팀원이 없는 방이어서 건너뛰었습니다.")
                conn.execute("DELETE FROM incoming WHERE team=?", (team,))
                continue
            teams[path] = code

        # No write lock during network reads. Recheck collisions while holding
        # the write lock so a server request or second importer cannot be overwritten.
        conn.execute("BEGIN IMMEDIATE")
        try:
            for path, code in teams.items():
                team = path.split("/")[1]
                if previous_import(conn, path):
                    messages.append(f"{team}: 이미 이전한 팀입니다. 현재 자료를 유지했습니다.")
                    continue
                collision = conn.execute("""SELECT 1 FROM docs WHERE path IN (?,?)
                        OR (path>=? AND path<?) LIMIT 1""",
                        (path, "joinCodes/" + code, path + "/", path + "0")).fetchone()
                if collision or previous_import(conn, "joinCodes/" + code):
                    raise MigrationError(f"데스크톱 DB에 팀 {team} 또는 같은 참여 코드가 있어 복사를 중단했습니다.")
                rows = conn.execute("SELECT path,fields FROM incoming WHERE team=?", (team,))
                count = 0
                for doc_path, fields in rows:
                    db.put_document(conn, doc_path, json.loads(fields))
                    count += 1
                conn.execute("INSERT INTO imported_teams VALUES(?,?,?,?)", (team, code, stamp, count))
                messages.append(f"{team}: 팀 문서·참여 코드 포함 {count}개 복사 완료 (기준 {stamp})")
            conn.commit()
        except BaseException:
            conn.rollback()
            raise
        return messages


def add_arguments(parser, default_db=None):
    parser.formatter_class = argparse.RawDescriptionHelpFormatter
    parser.epilog = (
        "1. 데스크톱에 Google Cloud CLI를 설치하고 gcloud auth login 으로 로그인합니다.\n"
        "   계정에는 saigajoa의 Cloud Datastore Viewer 역할 등 문서 읽기 권한이 필요합니다.\n"
        "2. 팀원이 기존 서버에 새 글을 쓰지 않는 동안 --team 뒤에 참여 코드를 넣어 실행합니다.\n"
        "3. 복사 완료 후 팀원 모두 사이 설정에서 같은 데스크톱 서버에 연결합니다.\n"
        "   이미 복사한 팀은 다시 실행해도 덮어쓰지 않습니다. 원본 Firebase 자료는 유지됩니다."
    )
    parser.add_argument("--team", action="append", required=True, metavar="CODE_OR_ID",
                        help="복사할 팀의 6자리 참여 코드 또는 팀 ID. 여러 팀이면 반복 입력합니다.")
    parser.add_argument("--db", required=default_db is None, default=default_db, metavar="PATH",
                        help="사이 팀 서버에서 사용할 SQLite 파일")


def run(args):
    try:
        for message in migrate(args.db, args.team, Firestore(PROJECT)):
            print(message)
        return 0
    except (MigrationError, OSError, sqlite3.Error, ValueError) as exc:
        print(f"이전 중단. 이번 실행에서 팀 자료를 변경하지 않았습니다. {exc}")
        return 1


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    add_arguments(parser)
    raise SystemExit(run(parser.parse_args()))
