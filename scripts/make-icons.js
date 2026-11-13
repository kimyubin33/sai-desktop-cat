// 의존성 없이 PNG 아이콘을 만드는 스크립트.
// 사이의 얼굴(16x16 도트)을 그려 assets/icon.png(256px), assets/tray.png(32px)로 저장한다.
const fs = require('fs');
const path = require('path');
const zlib = require('zlib');

const PALETTE = {
  '.': null,
  o: [59, 47, 47, 255], // 외곽선
  f: [242, 166, 90, 255], // 털
  s: [214, 128, 58, 255], // 줄무늬
  w: [251, 227, 196, 255], // 밝은 털
  p: [247, 178, 183, 255], // 귀 안쪽·코
  e: [43, 43, 43, 255], // 눈
};

// 16x16 얼굴
const FACE = [
  '................',
  '..oo........oo..',
  '.ofpo......opfo.',
  '.offpo....opffo.',
  '.offfoooooofffo.',
  '.ofsffsffsffsfo.',
  'offffffffffffffo',
  'offeeffffffeefo.',
  'offeeffffffeeffo',
  'offffffpp ffffo.'.replace(' ', 'f'),
  'ofwwwwfoofwwwwfo',
  '.owwwwwwwwwwwwo.',
  '.owwwwwwwwwwwwo.',
  '..owwwwwwwwwwo..',
  '...oooooooooo...',
  '................',
].map((r) => r.padEnd(16, '.').slice(0, 16));

function crc32(buf) {
  let c;
  const table = [];
  for (let n = 0; n < 256; n++) {
    c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[n] = c >>> 0;
  }
  let crc = 0xffffffff;
  for (const b of buf) crc = table[(crc ^ b) & 0xff] ^ (crc >>> 8);
  return (crc ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const td = Buffer.concat([Buffer.from(type), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(td));
  return Buffer.concat([len, td, crc]);
}

function png(size) {
  const scale = size / 16;
  const raw = Buffer.alloc((size * 4 + 1) * size);
  for (let y = 0; y < size; y++) {
    raw[y * (size * 4 + 1)] = 0;
    for (let x = 0; x < size; x++) {
      const ch = FACE[Math.floor(y / scale)][Math.floor(x / scale)];
      const col = PALETTE[ch] || [0, 0, 0, 0];
      const i = y * (size * 4 + 1) + 1 + x * 4;
      raw[i] = col[0];
      raw[i + 1] = col[1];
      raw[i + 2] = col[2];
      raw[i + 3] = col[3];
    }
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8;
  ihdr[9] = 6;
  return Buffer.concat([
    Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
    chunk('IHDR', ihdr),
    chunk('IDAT', zlib.deflateSync(raw)),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

const out = path.join(__dirname, '..', 'assets');
fs.mkdirSync(out, { recursive: true });
fs.writeFileSync(path.join(out, 'icon.png'), png(256));
fs.writeFileSync(path.join(out, 'tray.png'), png(32));
console.log('assets/icon.png, assets/tray.png 생성 완료');
