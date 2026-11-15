"""Run the app's Rust contracts against a temporary SQLite server: python -m server.tests.client_contract."""

import contextlib
import http.client
import json
import os
import secrets
import subprocess
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

from server.api import create_server


class LocalAccounts:
    """Issue opaque test-only tokens. Only this test process knows their identities."""

    def __init__(self):
        self.accounts = {}
        self.tokens = {}
        self.lock = threading.Lock()

    def issue(self, refresh=None):
        with self.lock:
            if refresh is None:
                refresh = secrets.token_urlsafe()
                self.accounts[refresh] = secrets.token_hex(14)
            uid = self.accounts[refresh]
            token = secrets.token_urlsafe()
            self.tokens[token] = uid
            return uid, token, refresh

    def verify(self, token):
        with self.lock:
            return self.tokens[token]


def issuer_handler(accounts):
    class Issuer(BaseHTTPRequestHandler):
        def do_POST(self):
            data = self.rfile.read(int(self.headers.get("Content-Length", 0)))
            path = urlsplit(self.path).path
            if path == "/identitytoolkit.googleapis.com/v1/accounts:signUp":
                uid, token, refresh = accounts.issue()
                result = {"localId": uid, "idToken": token, "refreshToken": refresh, "expiresIn": "3600"}
            elif path == "/securetoken.googleapis.com/v1/token":
                try:
                    refresh = parse_qs(data.decode())["refresh_token"][0]
                    uid, token, refresh = accounts.issue(refresh)
                except (KeyError, ValueError):
                    return self.send_error(401)
                result = {"user_id": uid, "id_token": token, "refresh_token": refresh, "expires_in": "3600"}
            else:
                return self.send_error(404)
            body = json.dumps(result).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *_):
            pass

    return Issuer


@contextlib.contextmanager
def listening(httpd):
    with httpd:
        thread = threading.Thread(target=httpd.serve_forever, daemon=True)
        thread.start()
        try:
            yield httpd.server_port
        finally:
            httpd.shutdown()
            thread.join()


def main():
    accounts = LocalAccounts()
    address = ("127.0.0.1", 0)
    with tempfile.TemporaryDirectory() as tmp:
        with listening(ThreadingHTTPServer(address, issuer_handler(accounts))) as auth_port:
            api = create_server(address, Path(tmp) / "test.sqlite3", accounts.verify, restrict_creation=False)
            with listening(api) as doc_port:
                with contextlib.closing(http.client.HTTPConnection("127.0.0.1", doc_port, timeout=10)) as conn:
                    conn.request("GET", "/health")
                    health = json.load(conn.getresponse())
                localhost_exceptions = "localhost,127.0.0.1,::1"
                env = {
                    **os.environ,
                    "no_proxy": localhost_exceptions,
                    "NO_PROXY": localhost_exceptions,
                    "SAI_FIREBASE_EMULATOR": "1",
                    "SAI_DESKTOP_CONTRACT_PORTS": f"{auth_port},{doc_port}",
                    "SAI_DESKTOP_CONTRACT_SERVER_ID": health["serverId"],
                }
                subprocess.run(
                    ["cargo", "test", "--locked", "-p", "sai-core", "--test", "firebase_real", "--",
                     "--ignored", "--nocapture", "--test-threads=1"],
                    env=env, cwd=Path(__file__).resolve().parents[2], check=True,
                )


if __name__ == "__main__":
    main()
