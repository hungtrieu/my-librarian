// Xem trước giao diện trong trình duyệt với backend giả (ui/mock.js): node scripts/serve-ui.mjs [port] [thư mục mẫu]
// Trình đọc: /__book/<định dạng>/file (có HTTP Range), /pages, /page/<n> lấy từ thư mục mẫu
// READER_SAMPLES (mặc định target/reader-samples): sample.epub, sample.pdf, sample.mobi, sample.fb2,
// sample.txt và thư mục comic/ chứa ảnh trang.
import http from 'node:http';
import { readFile, readdir, stat, open } from 'node:fs/promises';
import { extname, join, normalize } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../ui/', import.meta.url));
const samples = process.argv[3] || process.env.READER_SAMPLES || fileURLToPath(new URL('../target/reader-samples/', import.meta.url));
const types = { '.html': 'text/html', '.js': 'text/javascript', '.mjs': 'text/javascript', '.css': 'text/css', '.png': 'image/png', '.jpg': 'image/jpeg',
  '.svg': 'image/svg+xml', '.json': 'application/json', '.bcmap': 'application/octet-stream', '.pfb': 'application/octet-stream', '.ttf': 'font/ttf' };
const port = +(process.env.PORT || process.argv[2] || 5173);
const natural = new Intl.Collator('vi', { numeric: true }).compare;

async function comicPages() {
  const dir = join(samples, 'comic');
  return (await readdir(dir)).filter((n) => /\.(png|jpe?g|webp|gif)$/i.test(n)).sort(natural).map((n) => join(dir, n));
}

async function serveBook(req, res, fmt, rest) {
  if (rest[0] === 'pages') return res.writeHead(200, { 'Content-Type': 'application/json' }).end(JSON.stringify({ count: (await comicPages()).length }));
  if (rest[0] === 'page') {
    const f = (await comicPages())[+rest[1]];
    if (!f) return res.writeHead(404).end('no page');
    return res.writeHead(200, { 'Content-Type': types[extname(f).toLowerCase()] || 'image/png' }).end(await readFile(f));
  }
  // sample.<fmt>.link: đường dẫn file thật thay cho file mẫu; sample.<fmt>.xref: đuôi nối thêm (bảng xref đã sửa).
  let file = join(samples, 'sample.' + fmt);
  try { file = (await readFile(file + '.link', 'utf8')).trim(); } catch { /* không có */ }
  let extra = Buffer.alloc(0);
  try { extra = await readFile(join(samples, 'sample.' + fmt + '.xref')); } catch { /* không có */ }
  const fileSize = (await stat(file)).size, size = fileSize + extra.length;
  const m = /^bytes=(\d*)-(\d*)$/.exec(req.headers.range || '');
  if (!m) return res.writeHead(200, { 'Content-Length': size, 'Accept-Ranges': 'bytes' }).end(Buffer.concat([await readFile(file), extra]));
  const start = m[1] ? +m[1] : size - +m[2];
  // Như MAX_RANGE phía Rust: một phản hồi Range không quá 32 MB.
  const end = Math.min(m[1] && m[2] ? Math.min(+m[2], size - 1) : size - 1, start + 32 * 1024 * 1024 - 1);
  const buf = Buffer.alloc(end - start + 1);
  const inFile = start < fileSize ? Math.min(end + 1, fileSize) - start : 0;
  if (inFile > 0) { const fh = await open(file); await fh.read(buf, 0, inFile, start); await fh.close(); }
  if (inFile < buf.length) extra.copy(buf, inFile, start + inFile - fileSize, start + buf.length - fileSize);
  res.writeHead(206, { 'Content-Range': `bytes ${start}-${end}/${size}`, 'Content-Length': buf.length, 'Accept-Ranges': 'bytes' }).end(buf);
}

http.createServer(async (req, res) => {
  const p = normalize(decodeURIComponent(new URL(req.url, 'http://x').pathname)).replace(/^[\\/]+/, '');
  const parts = p.split(/[\\/]/);
  if (parts[0] === '__book') {
    try { await serveBook(req, res, parts[1], parts.slice(2)); } catch (e) { res.writeHead(404).end(String(e.message)); }
    return;
  }
  const file = join(root, p || 'index.html');
  if (!file.startsWith(root)) { res.writeHead(403).end(); return; }
  try {
    const body = await readFile(file);
    res.writeHead(200, { 'Content-Type': (types[extname(file)] || 'application/octet-stream') + '; charset=utf-8', 'Cache-Control': 'no-store' }).end(body);
  } catch { res.writeHead(404).end('not found'); }
}).listen(port, () => console.log(`UI: http://127.0.0.1:${port}`));
