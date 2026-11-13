// API 키·로그인 토큰을 운영체제 보안 저장소(safeStorage)로 암호화해 설정 파일에 보관한다.
const { safeStorage } = require('electron');

function encryptSecret(value) {
  if (!value) return null;
  const s = typeof value === 'string' ? value : JSON.stringify(value);
  if (safeStorage.isEncryptionAvailable()) return 'enc:' + safeStorage.encryptString(s).toString('base64');
  return 'raw:' + Buffer.from(s).toString('base64');
}

function decryptSecret(stored) {
  if (!stored) return null;
  try {
    if (stored.startsWith('enc:')) return safeStorage.decryptString(Buffer.from(stored.slice(4), 'base64'));
    if (stored.startsWith('raw:')) return Buffer.from(stored.slice(4), 'base64').toString('utf8');
  } catch {
    return null;
  }
  return null;
}

function decryptJson(stored) {
  const s = decryptSecret(stored);
  if (!s) return null;
  try {
    return JSON.parse(s);
  } catch {
    return null;
  }
}

module.exports = { encryptSecret, decryptSecret, decryptJson };
