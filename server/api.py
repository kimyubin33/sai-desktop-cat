"""Firestore REST operations used by the Sai app, backed by the local SQLite database."""
import base64
import copy
import json
import math
import re
import secrets
import sqlite3
import threading
from contextlib import closing
from functools import lru_cache
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, unquote, urlsplit

from . import db, policy


class RequestError(Exception):
    def __init__(self, code, message):
        super().__init__(message)
        self.code = code


def require(condition, message="요청 형식을 확인해 주세요."):
    if not condition:
        raise RequestError(400, message)


def permitted(result):
    if not result:
        raise RequestError(403, "이 작업을 수행할 권한이 없어요.")


def identifier(part):
    require(isinstance(part, str) and part and part not in (".", "..")
            and not any(c in "/\\\x00" or ord(c) < 32 for c in part)
            and len(part.encode("utf-8")) <= 1500, "잘못된 문서 경로예요.")
    return part


def path_parts(path, url_encoded=False):
    require(isinstance(path, str) and len(path.encode()) <= 6144)
    parts = path.split("/")
    require(len(parts) <= 12)
    if url_encoded:
        require(not re.search("%(?![a-fA-F0-9]{2})", path))
        parts = [unquote(part, errors="strict") for part in parts]
    return [identifier(part) for part in parts]


def decode_value(value, depth=0):
    require(type(value) is dict and len(value) == 1 and depth <= 20)
    tag, scalar = next(iter(value.items()))
    if tag == "nullValue" and scalar is None:
        return None
    if tag == "booleanValue" and type(scalar) is bool:
        return scalar
    if tag == "integerValue" and (type(scalar) is int or
                                  isinstance(scalar, str) and re.fullmatch(r"-?[0-9]{1,19}", scalar)):
        number = int(scalar)
        require(-(2**63) <= number < 2**63)
        return number
    if tag == "doubleValue" and type(scalar) in (int, float) and math.isfinite(scalar):
        return float(scalar)
    if tag == "stringValue" and type(scalar) is str:
        return scalar
    if tag == "arrayValue" and type(scalar) is dict and scalar.keys() <= {"values"}:
        values = scalar.get("values", [])
        require(type(values) is list)
        return [decode_value(v, depth + 1) for v in values]
    if tag == "mapValue" and type(scalar) is dict and scalar.keys() <= {"fields"}:
        return decode_fields(scalar.get("fields", {}), depth + 1)
    raise RequestError(400, "지원하지 않는 필드 값이에요.")


def decode_fields(fields, depth=0):
    require(type(fields) is dict and depth <= 20)
    return {k: decode_value(v, depth) for k, v in fields.items()}


def encode_value(value):
    if value is None:
        return {"nullValue": None}
    if type(value) is bool:
        return {"booleanValue": value}
    if type(value) is int:
        return {"integerValue": str(value)}
    if type(value) is float:
        return {"doubleValue": value}
    if isinstance(value, str):
        return {"stringValue": value}
    if isinstance(value, list):
        return {"arrayValue": {"values": [encode_value(v) for v in value]}}
    return {"mapValue": {"fields": {k: encode_value(v) for k, v in value.items()}}}


def field_path(mask):
    require(isinstance(mask, str) and 0 < len(mask) <= 1500)
    parts = re.findall(r"(?:`[^`\\]+`|[A-Za-z_][A-Za-z_0-9]*)", mask)
    require(parts and ".".join(parts) == mask)
    return [part.strip("`") for part in parts]


def patched(original, patch, mask):
    require(type(mask) is list and mask)
    result = copy.deepcopy(original) if original is not None else {}
    for field in mask:
        keys = field_path(field)
        source = patch
        dest = result
        for key in keys[:-1]:
            require(type(source) is dict and type(source.get(key)) is dict)
            source = source[key]
            if not isinstance(dest.get(key), dict):
                dest[key] = {}
            dest = dest[key]
        require(keys[-1] in source)
        dest[keys[-1]] = source[keys[-1]]
    return result


def as_document(name_root, row):
    fields = json.loads(row["fields"])
    return {"name": name_root + row["path"], "fields": {k: encode_value(v) for k, v in fields.items()},
            "updateTime": db.timestamp(row["version"])}


class Documents:
    def __init__(self, conn, uid, project, restrict_creation):
        self.restrict_creation = restrict_creation
        self.conn = conn
        self.uid = uid
        self.root = f"projects/{project}/databases/(default)/documents/"
        # Each request has its own transaction. Read authorization and responses see one snapshot.
        self.row = lru_cache(maxsize=2048)(lambda path: db.get_row(conn, path))
        self.get = lru_cache(maxsize=2048)(self._fields)

    def _fields(self, path):
        row = self.row(path)
        return json.loads(row["fields"]) if row is not None else None

    def read(self, path):
        permitted(policy.can_read(self.uid, path, self.get))
        row = self.row(path)
        if row is None:
            raise RequestError(404, "문서를 찾을 수 없어요.")
        return as_document(self.root, row)

    def list(self, path, query):
        permitted(policy.can_list(self.uid, path, self.get, []))
        require(query.keys() <= {"pageSize", "pageToken"})
        size = int(query.get("pageSize", ["100"])[0])
        require(1 <= size <= 100)
        after = ""
        if "pageToken" in query:
            token = query["pageToken"][0]
            require(isinstance(token, str) and len(token) < 8200)
            cursor = base64.b64decode(token, altchars=b"-_", validate=True).decode()
            parent, sep, leaf = cursor.rpartition("/")
            require(sep and parent == path)
            after = identifier(leaf)
        result = {"documents": []}
        stored_chars = 0
        last_path = ""
        for row in db.collection_rows(self.conn, path, after, size + 1):
            if last_path and (len(result["documents"]) >= size or stored_chars + len(row["fields"]) > 1024 * 1024):
                result["nextPageToken"] = base64.urlsafe_b64encode(last_path.encode()).decode()
                break
            result["documents"].append(as_document(self.root, row))
            stored_chars += len(row["fields"])
            last_path = row["path"]
        return result

    def query(self, parent, body):
        require(type(body) is dict and body.keys() == {"structuredQuery"})
        query = body["structuredQuery"]
        require(type(query) is dict and query.keys() <= {"from", "where", "orderBy", "limit"})
        sources = query.get("from")
        require(type(sources) is list and len(sources) == 1 and type(sources[0]) is dict
                and sources[0].keys() == {"collectionId"})
        coll = identifier(sources[0]["collectionId"])
        require(coll in ("shares", "signals"))
        path = f"{parent}/{coll}"
        filters = []
        if "where" in query:
            where = query["where"]
            require(type(where) is dict and where.keys() == {"fieldFilter"})
            filt = where["fieldFilter"]
            require(type(filt) is dict and filt.keys() == {"field", "op", "value"})
            field = "toMember" if coll == "signals" else "at"
            operator = "EQUAL" if coll == "signals" else "GREATER_THAN_OR_EQUAL"
            require(filt["field"] == {"fieldPath": field} and filt["op"] == operator)
            value = decode_value(filt["value"])
            require(type(value) is (str if coll == "signals" else int))
            filters = [(field, operator, value)]
        permitted(policy.can_list(self.uid, path, self.get, filters))
        latest = "orderBy" in query
        if latest:
            require(coll == "shares" and query["orderBy"] == [
                {"field": {"fieldPath": "at"}, "direction": "DESCENDING"}])
        limit = query.get("limit")
        require(limit is None or type(limit) is int and 1 <= limit <= 1000)
        return [{"document": as_document(self.root, row)}
                for row in db.collection_rows(self.conn, path, filters=filters, limit=limit, latest=latest)]

    def transact(self, writes):
        require(type(writes) is list and 0 < len(writes) <= 500)
        prospective = {}
        conditions = []
        for write in writes:
            require(type(write) is dict)
            deleting = "delete" in write
            allowed = {"delete", "currentDocument"} if deleting else {"update", "currentDocument", "updateMask"}
            require(write.keys() <= allowed)
            if deleting:
                name = write["delete"]
            else:
                update = write.get("update")
                require(type(update) is dict and update.keys() <= {"name", "fields"})
                name = update.get("name")
            require(isinstance(name, str) and name.startswith(self.root))
            parts = path_parts(name[len(self.root):])
            require(len(parts) % 2 == 0)
            path = "/".join(parts)
            require(path not in prospective)
            condition = write.get("currentDocument", {})
            require(type(condition) is dict and (not condition or
                    condition.keys() == {"exists"} and type(condition["exists"]) is bool or
                    condition.keys() == {"updateTime"} and type(condition["updateTime"]) is str))
            conditions.append((path, condition))
            candidate = None
            if not deleting:
                candidate = decode_fields(update.get("fields", {}))
                if "updateMask" in write:
                    mask = write["updateMask"]
                    require(type(mask) is dict and mask.keys() == {"fieldPaths"})
                    candidate = patched(self.get(path), candidate, mask["fieldPaths"])
                require(len(json.dumps(candidate, ensure_ascii=False).encode("utf-8")) <= 1048576)
            prospective[path] = candidate

        def get_after(path):
            return prospective[path] if path in prospective else self.get(path)

        creating_team = any(path.startswith("teams/") and path.count("/") == 1
                            and candidate is not None and self.get(path) is None
                            for path, candidate in prospective.items())
        if creating_team:
            # The root, join code, creator membership and name registration must all commit together.
            if self.restrict_creation:
                permitted(db.can_create_team(self.conn, self.uid))
            permitted(policy.can_bootstrap(self.uid, prospective, self.get))
        else:
            for path, candidate in prospective.items():
                permitted(policy.can_write(self.uid, path, self.get(path), candidate, self.get, get_after))
        for path, condition in conditions:
            row = self.row(path)
            if "exists" in condition and condition["exists"] != (row is not None):
                raise RequestError(409, "문서 상태가 변경됐어요.")
            if "updateTime" in condition and (row is None or condition["updateTime"] != db.timestamp(row["version"])):
                raise RequestError(409, "다른 팀원이 문서를 변경했어요.")
        results = []
        for path, value in prospective.items():
            if value is None:
                self.conn.execute("DELETE FROM docs WHERE path=?", (path,))
                results.append({})
            else:
                version = db.put_document(self.conn, path, value)
                results.append({"updateTime": db.timestamp(version)})
        return {"writeResults": results}

    def route(self, method, suffix, query, body):
        if suffix == ":commit":
            require(method == "POST" and not query and type(body) is dict and body.keys() == {"writes"})
            return self.transact(body["writes"])
        require(suffix.startswith("/"))
        if method == "POST" and suffix.endswith(":runQuery"):
            parts = path_parts(suffix[1:-len(":runQuery")], True)
            require(len(parts) % 2 == 0 and not query)
            return self.query("/".join(parts), body)
        parts = path_parts(suffix[1:], True)
        path = "/".join(parts)
        collection = len(parts) % 2 == 1
        if method == "GET":
            if collection:
                return self.list(path, query)
            require(not query)
            return self.read(path)
        if method == "POST" and collection:
            require(query.keys() <= {"documentId"})
            path += "/" + identifier(query.get("documentId", [secrets.token_urlsafe(15)])[0])
            condition = {"exists": False}
        elif method == "PATCH" and not collection:
            require(query.keys() <= {"currentDocument.exists", "updateMask.fieldPaths"})
            condition = {}
            if "currentDocument.exists" in query:
                require(query["currentDocument.exists"] == ["true"])
                condition = {"exists": True}
        elif method == "DELETE" and not collection:
            require(not query)
            self.transact([{"delete": self.root + path}])
            return {}
        else:
            raise RequestError(405, "지원하지 않는 작업이에요.")
        require(type(body) is dict and body.keys() <= {"fields"})
        write = {"update": {"name": self.root + path, "fields": body.get("fields", {})},
                 "currentDocument": condition}
        if "updateMask.fieldPaths" in query:
            write["updateMask"] = {"fieldPaths": query["updateMask.fieldPaths"]}
        self.transact([write])
        # A create may deliberately only grant access to the submitting team member later.
        return as_document(self.root, db.get_row(self.conn, path))


MAX_BODY = 10 * 1024 * 1024
ERROR_NAMES = {400: "INVALID_ARGUMENT", 401: "UNAUTHENTICATED", 403: "PERMISSION_DENIED",
               404: "NOT_FOUND", 405: "METHOD_NOT_ALLOWED", 409: "FAILED_PRECONDITION",
               413: "RESOURCE_EXHAUSTED", 500: "INTERNAL", 503: "UNAVAILABLE"}


class TeamHandler(BaseHTTPRequestHandler):
    timeout = 15
    server_version = "SaiTeam/1"
    sys_version = ""

    def log_message(self, format, *args):
        # URL paths may include join codes and derived administrator keys.
        pass

    def send_json(self, status, data):
        payload = json.dumps(data, ensure_ascii=False, allow_nan=False, separators=(",", ":")).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(payload)))
        self.send_header("Cache-Control", "no-store")
        self.send_header("X-Content-Type-Options", "nosniff")
        self.end_headers()
        self.wfile.write(payload)

    def handle_method(self):
        try:
            data = self.dispatch()
            self.send_json(200, data)
        except (BrokenPipeError, ConnectionResetError, TimeoutError):
            self.close_connection = True
        except RequestError as exc:
            self.send_json(exc.code, {"error": {"code": exc.code, "status": ERROR_NAMES[exc.code], "message": str(exc)}})
        except (ValueError, TypeError, KeyError, UnicodeError, OverflowError, RecursionError):
            self.send_json(400, {"error": {"code": 400, "status": "INVALID_ARGUMENT", "message": "잘못된 요청이에요."}})
        except sqlite3.OperationalError:
            self.send_json(503, {"error": {"code": 503, "status": "UNAVAILABLE", "message": "서버 데이터베이스를 사용할 수 없어요."}})
        except Exception:
            self.send_json(500, {"error": {"code": 500, "status": "INTERNAL", "message": "서버에서 요청을 처리할 수 없어요."}})

    do_GET = do_POST = do_PATCH = do_DELETE = handle_method

    def dispatch(self):
        target = urlsplit(self.path)
        require(not target.scheme and not target.netloc and not target.fragment)
        if self.command == "GET" and target.path == "/health" and not target.query:
            return self.server.health
        base = f"/v1/projects/{self.server.project}/databases/(default)/documents"
        if not target.path.startswith(base) or target.path[len(base):][:1] not in ("/", ":"):
            raise RequestError(404, "팀 서버의 API 경로가 아니에요.")
        if self.headers.get("X-Sai-Server-Id", self.server.health["serverId"]) != self.server.health["serverId"]:
            raise RequestError(409, "설정에서 팀 서버를 다시 확인해 주세요.")

        auth = self.headers.get_all("Authorization") or []
        if len(auth) != 1 or not auth[0].startswith("Bearer ") or not (7 < len(auth[0]) < 16384):
            raise RequestError(401, "팀 계정으로 로그인해 주세요.")
        try:
            uid = self.server.verify_token(auth[0][7:])
        except OSError:
            raise RequestError(503, "팀 서버에서 Google 인증 키를 확인할 수 없어요. 잠시 뒤 다시 시도해 주세요.") from None
        except Exception:
            raise RequestError(401, "유효한 팀 계정 로그인이 필요해요.") from None
        if not isinstance(uid, str) or not uid:
            raise RequestError(401, "유효한 팀 계정 로그인이 필요해요.")
        identifier(uid)

        require(self.headers.get("Transfer-Encoding") is None)
        lengths = self.headers.get_all("Content-Length") or []
        require(len(lengths) <= 1)
        length = int(lengths[0]) if lengths else 0
        require(length >= 0)
        if length > MAX_BODY:
            raise RequestError(413, "요청이 너무 커요.")
        body = None
        if self.command in ("POST", "PATCH"):
            require(length > 0 and self.headers.get_content_type() == "application/json")
            body = json.loads(self.rfile.read(length).decode("utf-8"))
        else:
            require(length == 0)
        args = parse_qs(target.query, keep_blank_values=True, strict_parsing=True, max_num_fields=128,
                        errors="strict")
        require(all(k == "updateMask.fieldPaths" or len(v) == 1 for k, v in args.items()))

        suffix = target.path[len(base):]
        reading = self.command == "GET" or self.command == "POST" and suffix.endswith(":runQuery")
        with closing(db.connect(self.server.db_path)) as conn, conn:
            conn.execute("BEGIN" if reading else "BEGIN IMMEDIATE")
            docs = Documents(conn, uid, self.server.project, self.server.restrict_creation)
            return docs.route(self.command, suffix, args, body)


class TeamServer(ThreadingHTTPServer):
    def __init__(self, *args):
        self.workers = threading.BoundedSemaphore(32)
        super().__init__(*args)

    def process_request(self, request, client_address):
        if not self.workers.acquire(blocking=False):
            try:
                request.sendall(b"HTTP/1.0 503 Service Unavailable\r\nConnection: close\r\nContent-Length: 0\r\n\r\n")
            finally:
                self.shutdown_request(request)
            return
        try:
            super().process_request(request, client_address)
        except BaseException:
            self.workers.release()
            raise

    def process_request_thread(self, request, client_address):
        try:
            super().process_request_thread(request, client_address)
        finally:
            self.workers.release()


def create_server(address, db_path, verify_token, project="saigajoa", *, restrict_creation=True):
    """Build the host; verify_token must return the provider-verified Firebase UID."""
    identifier(project)
    server_id = db.initialize(db_path)
    server = TeamServer(address, TeamHandler)
    server.db_path = db_path
    server.project = project
    server.restrict_creation = restrict_creation
    server.verify_token = verify_token
    server.health = {"service": "sai-team-server", "protocol": 1, "projectId": project, "serverId": server_id}
    return server
