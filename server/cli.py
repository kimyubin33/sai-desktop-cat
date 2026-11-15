"""Local operator commands for Sai Team Server. No management endpoints are exposed."""
import argparse
import os
import sqlite3
import sys
from contextlib import closing
from pathlib import Path

from . import db

PROJECT = "saigajoa"


def default_db():
    base = Path(os.environ.get("LOCALAPPDATA") or Path.home() / ".local" / "share")
    return base / "SaiTeamServer" / "team.sqlite3"


def backup_database(source, destination):
    source = Path(source).resolve(strict=True)
    destination = Path(destination).resolve()
    destination.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    os.close(fd)
    try:
        with closing(sqlite3.connect(source.as_uri() + "?mode=ro", uri=True)) as src, closing(sqlite3.connect(destination)) as out:
            src.backup(out)
            if out.execute("PRAGMA quick_check").fetchone()[0] != "ok":
                raise ValueError("백업 파일 무결성 점검에 실패했습니다.")
    except BaseException:
        destination.unlink(missing_ok=True)
        raise
    return destination


def main(argv=None):
    from . import migrate

    parser = argparse.ArgumentParser(description="사이 데스크톱 팀 서버")
    commands = parser.add_subparsers(dest="command")
    commands.add_parser("setup", help="Windows 시작 메뉴")
    serve = commands.add_parser("serve", help="이 컴퓨터에서 팀 서버 실행")
    serve.add_argument("--port", type=int, default=8765)
    backup = commands.add_parser("backup", help="실행 중인 데이터베이스의 일관된 백업 만들기")
    backup.add_argument("--output", type=Path, required=True)
    creator = commands.add_parser("allow-creator", help="이 서버에 새 팀을 만들 계정 지정")
    creator.add_argument("--uid", required=True)
    creator.add_argument("--revoke", action="store_true", help="명시적으로 부여한 개설 권한 회수")
    for cmd in (serve, backup, creator):
        cmd.add_argument("--db", type=Path, default=default_db())
    migrate.add_arguments(commands.add_parser("migrate", help="선택한 기존 팀을 가져오기"), default_db=default_db())
    args = parser.parse_args(argv)
    try:
        if args.command == "setup" or (not args.command and os.name == "nt"):
            from .windows_setup import run_setup
            return run_setup()
        if not args.command:
            parser.print_help()
            return 0
        if args.command == "backup":
            print("백업 완료:", backup_database(args.db, args.output))
            return 0
        if args.command == "migrate":
            return migrate.run(args)
        args.db.parent.mkdir(parents=True, exist_ok=True)
        if args.command == "allow-creator":
            uid = args.uid.strip()
            if not 0 < len(uid) <= 128 or "/" in uid or any(ord(c) < 32 for c in uid):
                raise ValueError("설정에서 확인한 팀 계정 ID를 입력해 주세요.")
            db.initialize(args.db)
            with closing(db.connect(args.db)) as conn:
                if args.revoke:
                    conn.execute("DELETE FROM host_creators WHERE uid=?", (uid,))
                else:
                    conn.execute("INSERT OR IGNORE INTO host_creators(uid) VALUES (?)", (uid,))
            print("새 팀 개설 권한 설정을 저장했습니다.")
            return 0
        if not 1 <= args.port <= 65535:
            raise ValueError("포트는 1~65535 범위에서 지정해 주세요.")
        from .api import create_server
        from .auth import FirebaseIdentity

        with create_server(("127.0.0.1", args.port), args.db, FirebaseIdentity(PROJECT), project=PROJECT) as http:
            print(f"사이 팀 서버 실행 중 · http://127.0.0.1:{args.port}\n저장 위치: {args.db}", flush=True)
            try:
                http.serve_forever()
            except KeyboardInterrupt:
                pass
        return 0
    except (OSError, sqlite3.Error, ValueError, RuntimeError) as exc:
        print(f"팀 서버: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
