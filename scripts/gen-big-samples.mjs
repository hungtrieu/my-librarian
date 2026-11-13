// Tạo file mẫu dung lượng lớn để thử trình đọc với truyện tranh nặng (đọc lười qua HTTP Range):
//   node scripts/gen-big-samples.mjs [thư mục ra] [MB PDF] [MB MOBI]
// Mặc định: target/reader-samples-big, PDF ~2200 MB (ảnh RGB thô ~10 MB/trang), MOBI ~600 MB (PNG ~1.5 MB/trang).
// Xem bằng: node scripts/serve-ui.mjs 5174 target/reader-samples-big
import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';

const out = process.argv[2] || 'target/reader-samples-big';
const pdfMB = +(process.argv[3] || 2200);
const mobiMB = +(process.argv[4] || 600);
fs.mkdirSync(out, { recursive: true });

// ---------------------------------------------------------------- PDF: mỗi trang một ảnh RGB không nén
function writePdf(file, targetMB) {
  const W = 1800, H = 1850, imgLen = W * H * 3;
  const pages = Math.max(1, Math.round((targetMB * 1048576) / imgLen));
  const fd = fs.openSync(file, 'w');
  let pos = 0;
  const offsets = [];
  const put = (b) => { const buf = typeof b === 'string' ? Buffer.from(b, 'latin1') : b; fs.writeSync(fd, buf); pos += buf.length; };
  const obj = (n, body) => { offsets[n] = pos; put(`${n} 0 obj\n${body}\nendobj\n`); };
  put('%PDF-1.4\n%\xE2\xE3\xCF\xD3\n');
  // 1 Catalog, 2 Pages, rồi mỗi trang: Page, Contents, Image
  const pageObj = (i) => 3 + i * 3;
  obj(1, '<< /Type /Catalog /Pages 2 0 R >>');
  obj(2, `<< /Type /Pages /Count ${pages} /Kids [${Array.from({ length: pages }, (_, i) => `${pageObj(i)} 0 R`).join(' ')}] >>`);
  const row = Buffer.alloc(W * 3);
  for (let i = 0; i < pages; i++) {
    const p = pageObj(i), c = p + 1, im = p + 2;
    obj(p, `<< /Type /Page /Parent 2 0 R /MediaBox [0 0 ${W / 3} ${H / 3}] /Resources << /XObject << /Im0 ${im} 0 R >> /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >> /Contents ${c} 0 R >>`);
    const cs = `q ${W / 3} 0 0 ${H / 3} 0 0 cm /Im0 Do Q BT /F1 48 Tf 0 0 0 rg 40 40 Td (Trang ${i + 1}) Tj ET`;
    obj(c, `<< /Length ${cs.length} >>\nstream\n${cs}\nendstream`);
    offsets[im] = pos;
    put(`${im} 0 obj\n<< /Type /XObject /Subtype /Image /Width ${W} /Height ${H} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length ${imgLen} >>\nstream\n`);
    const hue = (i * 47) % 360;
    for (let y = 0; y < H; y += 1) {
      const band = Math.floor(y / 60) % 2;
      for (let x = 0; x < W; x++) {
        const k = x * 3, v = (x + y + i * 13) & 255;
        row[k] = band ? (hue + v) & 255 : 255 - v; row[k + 1] = (v * 2) & 255; row[k + 2] = band ? 200 : (hue & 255);
      }
      put(row);
    }
    put('\nendstream\nendobj\n');
    if (i % 20 === 19) process.stdout.write(`PDF ${i + 1}/${pages} trang, ${(pos / 1048576) | 0} MB\n`);
  }
  const n = 3 + pages * 3;
  const xref = pos;
  put(`xref\n0 ${n}\n0000000000 65535 f \n`);
  for (let k = 1; k < n; k++) put(`${String(offsets[k]).padStart(10, '0')} 00000 n \n`);
  put(`trailer\n<< /Size ${n} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`);
  fs.closeSync(fd);
  console.log(`PDF: ${file} – ${pages} trang, ${(pos / 1048576).toFixed(0)} MB`);
}

// ---------------------------------------------------------------- PNG đơn giản (để làm trang truyện trong MOBI)
const crcTable = Array.from({ length: 256 }, (_, n) => { let c = n; for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1; return c >>> 0; });
const crc = (buf) => { let c = 0xffffffff; for (const b of buf) c = crcTable[(c ^ b) & 255] ^ (c >>> 8); return (c ^ 0xffffffff) >>> 0; };
function chunk(type, data) {
  const len = Buffer.alloc(4); len.writeUInt32BE(data.length);
  const td = Buffer.concat([Buffer.from(type, 'latin1'), data]);
  const c = Buffer.alloc(4); c.writeUInt32BE(crc(td));
  return Buffer.concat([len, td, c]);
}
/** PNG ~targetBytes: ảnh nhiễu (khó nén) có dải màu theo số trang. */
function png(i, targetBytes) {
  const W = 800, H = Math.max(200, Math.round(targetBytes / (W * 3)));
  const raw = Buffer.alloc((W * 3 + 1) * H);
  let seed = 12345 + i * 7919;
  for (let y = 0; y < H; y++) {
    const o = y * (W * 3 + 1);
    for (let x = 0; x < W; x++) {
      seed = (seed * 1103515245 + 12345) & 0x7fffffff;
      const k = o + 1 + x * 3, stripe = Math.floor(y / 80) % 2;
      raw[k] = stripe ? (i * 40) & 255 : seed & 255; raw[k + 1] = stripe ? 255 - ((i * 40) & 255) : (seed >> 8) & 255; raw[k + 2] = stripe ? 128 : (seed >> 16) & 255;
    }
  }
  const ihdr = Buffer.alloc(13); ihdr.writeUInt32BE(W, 0); ihdr.writeUInt32BE(H, 4); ihdr[8] = 8; ihdr[9] = 2;
  return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk('IHDR', ihdr), chunk('IDAT', zlib.deflateSync(raw, { level: 0 })), chunk('IEND', Buffer.alloc(0))]);
}

// ---------------------------------------------------------------- MOBI6: văn bản HTML trỏ tới ảnh qua recindex
function writeMobi(file, targetMB) {
  const imgBytes = 1.5 * 1048576;
  const pages = Math.max(1, Math.round((targetMB * 1048576) / imgBytes));
  const title = `Truyen lon ${pages} trang`;
  let html = '<html><head></head><body>';
  for (let i = 1; i <= pages; i++) html += `<p><img recindex="${String(i).padStart(5, '0')}"/></p><mbp:pagebreak/>`;
  html += '</body></html>';
  const body = Buffer.from(html, 'utf8');
  const textRecs = [];
  for (let p = 0; p < body.length; p += 4096) textRecs.push(body.subarray(p, p + 4096));
  const firstImage = 1 + textRecs.length;

  const MOBI_LEN = 232;
  const rec0 = Buffer.alloc(16 + MOBI_LEN);
  rec0.writeUInt16BE(1, 0); // không nén
  rec0.writeUInt32BE(body.length, 4);
  rec0.writeUInt16BE(textRecs.length, 8);
  rec0.writeUInt16BE(4096, 10);
  rec0.write('MOBI', 16, 'latin1');
  rec0.writeUInt32BE(MOBI_LEN, 20);
  rec0.writeUInt32BE(2, 24);
  rec0.writeUInt32BE(65001, 28);
  rec0.writeUInt32BE(rec0.length, 84);
  rec0.writeUInt32BE(title.length, 88);
  rec0.writeUInt32BE(firstImage, 108);
  const rec0full = Buffer.concat([rec0, Buffer.from(title + '\0\0', 'latin1')]);

  const numRecords = 1 + textRecs.length + pages;
  const head = Buffer.alloc(78);
  head.write('TruyenLon', 0, 'latin1');
  head.write('BOOKMOBI', 60, 'latin1');
  head.writeUInt16BE(numRecords, 76);
  // Một ảnh dùng lại cho mọi trang cho nhanh, nhưng đổi màu theo nhóm 8 trang để thấy trang thay đổi.
  const imgs = Array.from({ length: 8 }, (_, k) => png(k, imgBytes));
  const sizes = [rec0full.length, ...textRecs.map((r) => r.length), ...Array.from({ length: pages }, (_, i) => imgs[i % 8].length)];
  const table = Buffer.alloc(numRecords * 8 + 2);
  let off = 78 + table.length;
  sizes.forEach((s, i) => { table.writeUInt32BE(off, i * 8); table.writeUInt32BE(i * 2, i * 8 + 4); off += s; });
  const fd = fs.openSync(file, 'w');
  for (const b of [head, table, rec0full, ...textRecs]) fs.writeSync(fd, b);
  for (let i = 0; i < pages; i++) fs.writeSync(fd, imgs[i % 8]);
  fs.closeSync(fd);
  console.log(`MOBI: ${file} – ${pages} trang, ${(off / 1048576).toFixed(0)} MB`);
}

if (mobiMB > 0) writeMobi(path.join(out, 'sample.mobi'), mobiMB);
if (pdfMB > 0) writePdf(path.join(out, 'sample.pdf'), pdfMB);
