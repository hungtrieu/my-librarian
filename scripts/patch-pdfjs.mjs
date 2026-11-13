// Vá pdf.js sau khi nâng cấp (xem ui/vendor/foliate-js/VERSION.txt): node scripts/patch-pdfjs.mjs ui/vendor/foliate-js/vendor/pdfjs/pdf.worker.mjs
// Vá pdf.worker.mjs: ChunkedStream lưu theo khúc (thưa) thay vì new Uint8Array(cả file).
import fs from 'node:fs';
const file = process.argv[2];
let s = fs.readFileSync(file, 'utf8');
const rep = (from, to) => {
  if (s.includes(to)) return; // đã vá (chạy lại không sao)
  const n = s.split(from).length - 1;
  if (n !== 1) throw new Error(`expected 1 match, got ${n}: ${from.slice(0, 80)}`);
  s = s.replace(from, to);
};

const SPARSE = `// [47 Librarian] Bộ nhớ thưa cho ChunkedStream: chỉ giữ các khúc đã tải (tối đa SparseBytes.MAX_BYTES,
// bỏ khúc dùng lâu nhất khi vượt), để mở được PDF lớn hơn giới hạn ArrayBuffer (~2 GB) mà không tốn RAM.
class SparseBytes {
  static MAX_BYTES = 384 * 1024 * 1024;
  constructor(length, chunkSize) {
    this.length = this.byteLength = length;
    this.chunkSize = chunkSize;
    this.chunks = new Map();
    this.tick = 0;
    this.lastBegin = 0;
    this.lastEnd = 0;
    this.lastArr = null;
    this.onEvict = null;
  }
  get maxChunks() {
    return Math.max(8, Math.floor(SparseBytes.MAX_BYTES / this.chunkSize));
  }
  chunkArr(i) {
    const c = this.chunks.get(i);
    if (!c) return null;
    c.t = ++this.tick;
    return c.a;
  }
  get(pos) {
    if (pos >= this.lastBegin && pos < this.lastEnd) return this.lastArr[pos - this.lastBegin];
    const i = Math.floor(pos / this.chunkSize);
    const a = this.chunkArr(i);
    if (!a) return 0;
    this.lastBegin = i * this.chunkSize;
    this.lastEnd = this.lastBegin + a.length;
    this.lastArr = a;
    return a[pos - this.lastBegin];
  }
  subarray(begin, end) {
    if (end === undefined || end > this.length) end = this.length;
    if (begin < 0) begin = 0;
    if (end <= begin) return new Uint8Array(0);
    const cs = this.chunkSize;
    const first = Math.floor(begin / cs), last = Math.floor((end - 1) / cs);
    if (first === last) {
      const a = this.chunkArr(first);
      const off = first * cs;
      return a ? a.subarray(begin - off, end - off) : new Uint8Array(end - begin);
    }
    const out = new Uint8Array(end - begin);
    for (let i = first; i <= last; i++) {
      const a = this.chunkArr(i);
      if (!a) continue;
      const off = i * cs;
      const from = Math.max(begin, off), to = Math.min(end, off + a.length);
      out.set(a.subarray(from - off, to - off), from - begin);
    }
    return out;
  }
  set(data, begin) {
    const cs = this.chunkSize;
    const keep = new Set();
    for (let p = 0; p < data.length; p += cs) {
      const i = Math.floor((begin + p) / cs);
      this.chunks.set(i, { a: data.slice(p, Math.min(p + cs, data.length)), t: ++this.tick });
      keep.add(i);
    }
    this.lastArr = null;
    this.lastBegin = this.lastEnd = 0;
    const max = this.maxChunks;
    if (this.chunks.size <= max) return;
    const old = [...this.chunks].filter(([i]) => !keep.has(i)).sort((x, y) => x[1].t - y[1].t);
    for (const [i] of old.slice(0, this.chunks.size - max)) {
      this.chunks.delete(i);
      this.onEvict?.(i);
    }
  }
}
class ChunkedStream extends Stream {`;

rep('class ChunkedStream extends Stream {', SPARSE);

rep(`  constructor(length, chunkSize, manager) {
    super(new Uint8Array(length), 0, length, null);
    this.chunkSize = chunkSize;`,
`  constructor(length, chunkSize, manager) {
    super(new Uint8Array(0), 0, length, null);
    this.bytes = new SparseBytes(length, chunkSize);
    this.bytes.onEvict = chunk => {
      this._loadedChunks.delete(chunk);
      this._lastSuccessfulEnsureByteChunk = -1;
    };
    this.chunkSize = chunkSize;`);

// Đọc từng byte qua SparseBytes.get thay vì chỉ số mảng.
rep(`    if (pos >= this.progressiveDataLength) {
      this.ensureByte(pos);
    }
    return this.bytes[this.pos++];`,
`    if (pos >= this.progressiveDataLength) {
      this.ensureByte(pos);
    }
    return this.bytes.get(this.pos++);`);

// Một lần đọc cần nhiều hơn bộ nhớ cho phép (vd. quét lại toàn file khi bảng xref hỏng): báo lỗi thay vì tải-bỏ mãi.
rep(`    const endChunk = Math.min(Math.floor((end - 1) / this.chunkSize) + 1, this.numChunks);
    for (let chunk = beginChunk; chunk < endChunk; ++chunk) {
      if (!this._loadedChunks.has(chunk)) {
        throw new MissingDataException(begin, end);`,
`    const endChunk = Math.min(Math.floor((end - 1) / this.chunkSize) + 1, this.numChunks);
    for (let chunk = beginChunk; chunk < endChunk; ++chunk) {
      if (!this._loadedChunks.has(chunk)) {
        if (end - begin > SparseBytes.MAX_BYTES) {
          throw new Error("PDF cần đọc một khối quá lớn (" + Math.round((end - begin) / 1048576) + " MB) – có thể file bị hỏng bảng xref.");
        }
        throw new MissingDataException(begin, end);`);

// Băm ToUnicode: luồng gốc có thể là ChunkedStream (không có .buffer thật).
rep(`new Uint8Array(stream.bytes.buffer, stream.start, stream.end - stream.start);`,
    `(stream.bytes instanceof Uint8Array ? new Uint8Array(stream.bytes.buffer, stream.start, stream.end - stream.start) : stream.bytes.subarray(stream.start, stream.end));`);

// Cây trang phẳng (vd. 2101 trang trong một mảng Kids, mỗi trang cách nhau ~1 MB): pdf.js vốn tải lần lượt
// mọi trang đứng trước để tìm trang i (mở sách = đọc gần hết file). Khi Kids.length === Count thì mỗi
// phần tử là một trang – nhảy thẳng tới kids[i]; nếu hoá ra không phải trang thì đi đường cũ.
rep(`      let kids = currentNode.getRaw("Kids");
      if (kids instanceof Ref) {
        kids = await xref.fetchAsync(kids);
      }
      if (!Array.isArray(kids)) {`,
`      let kids = currentNode.getRaw("Kids");
      if (kids instanceof Ref) {
        kids = await xref.fetchAsync(kids);
      }
      if (Array.isArray(kids) && Number.isInteger(count) && kids.length === count && pageIndex - currentPageIndex < kids.length) {
        const kidRef = kids[pageIndex - currentPageIndex];
        if (kidRef instanceof Ref && !visitedNodes.has(kidRef)) {
          const kid = await (pageDictCache.get(kidRef) || xref.fetchAsync(kidRef));
          if (kid instanceof Dict && (isName(kid.getRaw("Type"), "Page") || !kid.has("Kids"))) {
            if (!pageKidsCountCache.has(kidRef)) {
              pageKidsCountCache.put(kidRef, 1);
            }
            if (!pageIndexCache.has(kidRef)) {
              pageIndexCache.put(kidRef, pageIndex);
            }
            return [kid, kidRef];
          }
          // Nút Pages bọc đúng một trang (Count 1): đi thẳng xuống nút đó.
          if (kid instanceof Dict && kid.getRaw("Count") === 1) {
            visitedNodes.put(kidRef);
            currentPageIndex = pageIndex;
            nodesToVisit.length = 0;
            nodesToVisit.push(kid);
            continue;
          }
        }
      }
      if (!Array.isArray(kids)) {`);

// Nút gốc có hàng nghìn kid: pdf.js tải trước TẤT CẢ kid (mỗi kid một khúc 1 MB ở chỗ khác nhau → đọc cả file).
// Chỉ tải trước khi nút gốc ít kid.
rep(`        if (currentNode === this.toplevelPagesDict && lastKid instanceof Ref && !pageDictCache.has(lastKid)) {`,
    `        if (currentNode === this.toplevelPagesDict && kids.length <= 64 && lastKid instanceof Ref && !pageDictCache.has(lastKid)) {`);

fs.writeFileSync(file, s);
console.log('patched', file);
