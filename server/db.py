"""The desktop host's local team documents. Callers own the transaction."""
import json
import sqlite3
import time
import uuid
from datetime import datetime, timedelta, timezone


def connect(path):
    connection = sqlite3.connect(path, isolation_level=None, timeout=8)
    connection.row_factory = sqlite3.Row
    return connection


def initialize(path):
    db = connect(path)
    try:
        db.execute("PRAGMA journal_mode=WAL")
        db.executescript("""
            CREATE TABLE IF NOT EXISTS docs (
                path TEXT PRIMARY KEY,
                fields TEXT NOT NULL,
                version INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS version_clock (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                last INTEGER NOT NULL
            );
            INSERT OR IGNORE INTO version_clock VALUES(1, 0);
            CREATE TABLE IF NOT EXISTS host_creators (uid TEXT PRIMARY KEY);
            CREATE TABLE IF NOT EXISTS host_info (
                name TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
        """)
        db.execute("INSERT OR IGNORE INTO host_info VALUES('serverId',?)", (str(uuid.uuid4()),))
        return db.execute("SELECT value FROM host_info WHERE name='serverId'").fetchone()[0]
    finally:
        db.close()


def put_document(conn, path, fields):
    """Write a plain JSON dictionary inside the caller's write transaction."""
    # A delete followed by recreation must not revive an old updateTime.
    conn.execute("UPDATE version_clock SET last=max(last+1, ?) WHERE id=1",
                 (time.time_ns() // 1000,))
    version = conn.execute("SELECT last FROM version_clock WHERE id=1").fetchone()[0]
    encoded = json.dumps(fields, ensure_ascii=False, allow_nan=False, separators=(",", ":"))
    conn.execute("""INSERT INTO docs(path,fields,version) VALUES(?,?,?)
                    ON CONFLICT(path) DO UPDATE SET fields=excluded.fields, version=excluded.version""",
                 (path, encoded, version))
    return version


def timestamp(version):
    moment = datetime(1970, 1, 1, tzinfo=timezone.utc) + timedelta(microseconds=version)
    return moment.isoformat(timespec="microseconds").replace("+00:00", "Z")


def get_row(conn, path):
    return conn.execute("SELECT path,fields,version FROM docs WHERE path=?", (path,)).fetchone()


def collection_rows(conn, collection, after_id="", limit=None, filters=(), latest=False):
    # The bounded primary-key range works for Unicode IDs without LIKE wildcards.
    prefix = collection + "/"
    conditions = ["path > ?", "path < ?", "instr(substr(path, ?), '/') = 0"]
    params = [prefix + after_id, collection + "0", len(prefix) + 1]
    columns = {"toMember": "json_extract(fields, '$.toMember')", "at": "json_extract(fields, '$.at')"}
    operators = {"EQUAL": "=", "GREATER_THAN_OR_EQUAL": ">="}
    for field, operator, value in filters:
        conditions.append(f"{columns[field]} {operators[operator]} ?")
        params.append(value)
    order = "json_extract(fields, '$.at') DESC, path DESC" if latest else "path ASC"
    sql = f"SELECT path,fields,version FROM docs WHERE {' AND '.join(conditions)} ORDER BY {order}"
    if limit is not None:
        sql += " LIMIT ?"
        params.append(limit)
    return conn.execute(sql, params)


def can_create_team(conn, uid):
    if conn.execute("SELECT 1 FROM host_creators WHERE uid=?", (uid,)).fetchone():
        return True
    # Existing owners of migrated active teams can use the normal creation flow.
    return conn.execute("""SELECT 1 FROM docs WHERE path >= 'teams/' AND path < 'teams0'
                           AND instr(substr(path,7), '/') = 0
                           AND json_extract(fields, '$.createdBy')=?
                           AND coalesce(json_extract(fields, '$.closed'),0)=0 LIMIT 1""",
                        (uid,)).fetchone() is not None
