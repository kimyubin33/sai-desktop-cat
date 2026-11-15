import tempfile
import time
import unittest
from pathlib import Path

import jwt
from cryptography.hazmat.primitives.asymmetric import rsa

from server.auth import FirebaseIdentity
from server.cli import backup_database
from server import db


class IdentityTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.signer = rsa.generate_private_key(public_exponent=65537, key_size=2048)

    def setUp(self):
        self.fetches = 0
        def fetch():
            self.fetches += 1
            return {"key-1": self.signer.public_key()}, 3600
        self.verifier = FirebaseIdentity("saigajoa", fetch)
        now = int(time.time()) - 1
        self.claims = {"aud": "saigajoa", "iss": "https://securetoken.google.com/saigajoa",
                       "sub": "member-A", "auth_time": now - 8, "iat": now, "exp": now + 900}

    def token(self, changes=None, kid="key-1"):
        return jwt.encode({**self.claims, **(changes or {})}, self.signer,
                          algorithm="RS256", headers={"kid": kid})

    def test_valid_signed_uid_and_certificate_cache(self):
        self.assertEqual(self.verifier(self.token()), "member-A")
        self.assertEqual(self.verifier(self.token({"sub": "member-B"})), "member-B")
        self.assertEqual(self.fetches, 1)

    def test_signature_must_match_the_selected_public_key(self):
        other_key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
        signed = jwt.encode(self.claims, other_key, algorithm="RS256", headers={"kid": "key-1"})
        with self.assertRaises(jwt.InvalidSignatureError):
            self.verifier(signed)

    def test_wrong_audience_issuer_expiry_auth_time_or_subject(self):
        for change in ({"aud": "another"}, {"aud": ["saigajoa"]}, {"iss": "https://example.org"},
                       {"exp": int(time.time()) - 60}, {"sub": ""}, {"sub": "a/b"},
                       {"auth_time": int(time.time()) + 600}):
            with self.subTest(change=change), self.assertRaises((ValueError, jwt.PyJWTError)):
                self.verifier(self.token(change))

    def test_certificate_outage_is_retryable(self):
        def unavailable():
            raise OSError("offline")
        verifier = FirebaseIdentity("saigajoa", unavailable)
        for _ in range(2):
            with self.assertRaises(OSError):
                verifier(self.token())

    def test_invalid_key_or_algorithm_rejected(self):
        with self.assertRaises(ValueError):
            self.verifier(self.token(kid="unknown"))
        # Expensive certificate refreshes are bounded even when unknown IDs arrive.
        with self.assertRaises(ValueError):
            self.verifier(self.token(kid="other"))
        self.assertEqual(self.fetches, 1)
        unsigned = jwt.encode(self.claims, key=None, algorithm="none")
        with self.assertRaises(ValueError):
            self.verifier(unsigned)


class BackupTests(unittest.TestCase):
    def test_wal_snapshot_and_no_overwrite(self):
        with tempfile.TemporaryDirectory() as tmp:
            source, target = Path(tmp, "db.sqlite3"), Path(tmp, "copy.sqlite3")
            db.initialize(source)
            conn = db.connect(source)
            try:
                db.put_document(conn, "teams/sample", {"name": "test"})
                backup_database(source, target)
                with self.assertRaises(FileExistsError):
                    backup_database(source, target)
                snapshot = db.connect(target)
                try:
                    self.assertIsNotNone(db.get_row(snapshot, "teams/sample"))
                finally:
                    snapshot.close()
            finally:
                conn.close()
