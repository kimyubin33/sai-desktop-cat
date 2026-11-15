"""Verify Firebase ID tokens for the existing team accounts with Google's public keys."""
import json
import re
import ssl
import threading
import time
import urllib.request

import certifi
import jwt
from cryptography import x509
from cryptography.hazmat.primitives.asymmetric import rsa

CERTIFICATES = "https://www.googleapis.com/robot/v1/metadata/x509/securetoken@system.gserviceaccount.com"


def fetch_certificates():
    with urllib.request.urlopen(CERTIFICATES, timeout=8,
                                context=ssl.create_default_context(cafile=certifi.where())) as response:
        encoded = response.read(65537)
        if len(encoded) > 65536:
            raise ValueError("인증서 응답이 너무 큽니다.")
        certs = json.loads(encoded)
        age = re.search(r"(?:^|,)\s*max-age=(\d+)", response.headers.get("Cache-Control", ""))
        keys = {kid: x509.load_pem_x509_certificate(pem.encode("ascii")).public_key()
                for kid, pem in certs.items()}
        if not keys or any(not isinstance(key, rsa.RSAPublicKey) for key in keys.values()):
            raise ValueError("Firebase 서명 인증서를 확인하지 못했습니다.")
        return keys, int(age.group(1)) if age else 60


class FirebaseIdentity:
    def __init__(self, project, fetch=fetch_certificates):
        self.project = project
        self.fetch = fetch
        self.keys = {}
        self.valid_until = 0
        self.retry_after = 0
        self.service_unavailable = False
        self.lock = threading.Lock()

    def signing_key(self, kid):
        with self.lock:
            now = time.monotonic()
            if now < self.valid_until and kid in self.keys:
                return self.keys[kid]
            if now < self.retry_after:
                if self.service_unavailable:
                    raise OSError("Google 로그인 인증서 서버에 잠시 연결할 수 없습니다.")
                raise ValueError("서명 키를 확인할 수 없습니다.")
            # Limit upstream refreshes for unknown key IDs as well as outages.
            self.retry_after = now + 30
            try:
                keys, max_age = self.fetch()
            except (OSError, ValueError) as exc:
                self.service_unavailable = True
                raise OSError("Google 로그인 인증서를 일시적으로 가져올 수 없습니다.") from exc
            self.service_unavailable = False
            self.keys = keys
            self.valid_until = now + min(86400, max(0, max_age))
            if kid not in keys:
                raise ValueError("등록된 서명 키가 아닙니다.")
            return keys[kid]

    def __call__(self, token):
        if not token or len(token) > 8192:
            raise ValueError("로그인 토큰을 확인해 주세요.")
        header = jwt.get_unverified_header(token)
        kid = header.get("kid")
        if header.get("alg") != "RS256" or not isinstance(kid, str) or not 0 < len(kid) <= 128:
            raise ValueError("지원하지 않는 서명입니다.")
        claims = jwt.decode(token, self.signing_key(kid), algorithms=["RS256"],
                            audience=self.project, issuer="https://securetoken.google.com/" + self.project,
                            options={"strict_aud": True, "require": ["sub", "aud", "iss", "exp", "iat", "auth_time"]})
        uid = claims["sub"]
        if not isinstance(uid, str) or not 0 < len(uid) <= 128 or "/" in uid or any(ord(c) < 32 for c in uid):
            raise ValueError("팀 계정을 확인할 수 없습니다.")
        if any(type(claims[name]) is not int for name in ("auth_time", "iat", "exp")):
            raise ValueError("토큰 시간이 올바르지 않습니다.")
        if not 0 <= claims["auth_time"] <= claims["iat"] < claims["exp"]:
            raise ValueError("인증 시각을 확인할 수 없습니다.")
        return uid
