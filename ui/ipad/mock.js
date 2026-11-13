// Backend giả cho giao diện iPad (xem trước trong trình duyệt). Lệnh thật (Rust, crates/ipad) bám theo crates/core/src/ipad_lib.rs.
import { fold } from '../ui-kit.js';

const now = Math.floor(Date.now() / 1000);
const GB = 1024 ** 3;
let nextId = 1;
const books = [];
const add = (o) => {
  const id = nextId++;
  const b = { id, authors: '', series: null, volume: null, format: 'epub', size: 8e6 + ((id * 7919) % 90) * 1e6, sha256: 'x' + id, in_place: false, missing: false,
    added_at: now - id * 5400, read_status: 'unread', progress: null, locator: null, last_read_at: null, not_worth: false, ...o };
  b.file_name ??= `${b.title}.${b.format}`;
  books.push(b);
  return b;
};
[['Mắt Biếc', 'Nguyễn Nhật Ánh', 'reading', 0.42, 600], ['Số Đỏ', 'Vũ Trọng Phụng', 'read', 1, 99999], ['Truyện Kiều', 'Nguyễn Du'], ['Nhà Giả Kim', 'Paulo Coelho'],
  ['Rừng Na Uy', 'Haruki Murakami'], ['Clean Code', 'Robert C. Martin', 'reading', 0.15, 9000], ['Rust in Action', 'Tim McNamara'], ['Bố già', 'Mario Puzo', 'read', 1, 200000]]
  .forEach(([title, authors, st = 'unread', p = null, ago = 0], i) => add({ title, authors, format: i % 3 === 2 ? 'pdf' : 'epub', read_status: st, progress: p, last_read_at: p ? now - ago : null }));
for (const [s, n, fmt, readUpTo] of [['Doraemon', 12, 'cbz', 3], ['One Piece', 25, 'cbz', 0], ['7 seeds', 8, 'epub', 8]]) {
  for (let v = 1; v <= n; v++) {
    const st = v <= readUpTo ? 'read' : v === readUpTo + 1 && readUpTo ? 'reading' : 'unread';
    add({ title: `${s} - Vol ${v}`, authors: 'Nhiều tác giả', series: s, volume: v, format: fmt, size: 40e6, read_status: st, progress: st === 'read' ? 1 : st === 'reading' ? 0.3 : null,
      last_read_at: st === 'unread' ? null : now - (n - v) * 86400 - 3600 });
  }
}
const byId = (id) => books.find((b) => b.id === id);
const nat = new Intl.Collator('vi', { numeric: true }).compare;
const volSort = (a, b) => (a.volume != null && b.volume != null && a.volume !== b.volume ? a.volume - b.volume : nat(a.title, b.title));
const free = { bytes: 12 * GB };
const Q = new URLSearchParams(location.search);
const sync = { paired: Q.has('paired'), auto: true, running: false,
  items: [['Dune', 'Frank Herbert', 'epub', 6e6], ['Neuromancer', 'William Gibson', 'epub', 5e6], ['Berserk - Vol 1', 'Kentaro Miura', 'cbz', Q.has('full') ? 11e9 : 180e6]]
    .map(([title, authors, format, size], i) => ({ id: 100 + i, title, authors, format, size, series: null, volume: null, sha256: 'ab' })) };
const listeners = [];
const marks = [];
let nextMark = 1;
const DROID = Q.has('android');
const PHONE = Q.has('iphone') || DROID;
// iPhone chỉ nhận sách chữ: bỏ PDF/truyện tranh như lệnh thật (ipad_lib::is_text_format khi đồng bộ).
const TEXT = new Set(['epub', 'mobi', 'prc', 'azw', 'azw3', 'kf8', 'fb2', 'txt']);
if (PHONE) { for (let i = books.length - 1; i >= 0; i--) if (!TEXT.has(books[i].format)) books.splice(i, 1); sync.items = sync.items.filter((i) => TEXT.has(i.format)); }
const emit = (ev, payload) => listeners.filter((l) => l[0] === ev).forEach((l) => l[1]({ payload }));

function tile(series, notWorth = false) {
  const v = books.filter((b) => b.series === series && b.not_worth === notWorth).sort(volSort);
  const reading = v.find((b) => b.read_status === 'reading');
  const lastRead = v.filter((b) => b.read_status === 'read').pop();
  const next = reading || (lastRead ? v[v.indexOf(lastRead) + 1] : v[0]);
  return { kind: 'series', series, volumes: v.length, read: v.filter((b) => b.read_status === 'read').length, cover_book_id: v[0].id, continue_book_id: next?.id ?? null,
    size: v.reduce((s, b) => s + b.size, 0), last_read_at: Math.max(0, ...v.map((b) => b.last_read_at || 0)) || null, added_at: Math.max(...v.map((b) => b.added_at)) };
}

const handlers = {
  lib_list: ({ q = '', sort = 'added', notWorth = false }) => {
    const toks = fold(q).split(/\s+/).filter(Boolean);
    const hit = (b) => b.not_worth === notWorth && toks.every((t) => fold(`${b.title} ${b.authors} ${b.series || ''}`).includes(t));
    const items = [];
    const seen = new Set();
    for (const b of books.filter(hit)) {
      if (b.series && books.filter((x) => x.series === b.series && x.not_worth === notWorth).length >= 2) { if (!seen.has(b.series)) { seen.add(b.series); items.push(tile(b.series, notWorth)); } } else items.push({ kind: 'book', ...b });
    }
    const t = (i) => (sort === 'recent' ? i.last_read_at || 0 : sort === 'title' ? 0 : i.added_at);
    const name = (i) => i.title ?? i.series;
    items.sort(sort === 'title' ? (a, b) => nat(name(a), name(b)) : (a, b) => t(b) - t(a));
    return items;
  },
  lib_set_not_worth: ({ ids, on }) => ids.filter((id) => { const b = byId(id); if (!b || b.not_worth === on) return false; b.not_worth = on; return true; }).length,
  lib_not_worth_count: () => books.filter((b) => b.not_worth).length,
  // Giả: máy tính không đổi cờ nào (?paired=1 vẫn trả 0).
  sync_marks: () => (sync.paired && !Q.has('offline') ? 0 : null),
  lib_series: ({ series }) => books.filter((b) => b.series === series).sort(volSort),
  lib_continue: ({ limit = 10 }) => {
    const items = books.filter((b) => b.read_status === 'reading' && !b.not_worth).sort((a, b) => b.last_read_at - a.last_read_at);
    return items.slice(0, limit);
  },
  lib_remove: ({ ids }) => {
    let freed = 0;
    for (const id of ids) { const i = books.findIndex((b) => b.id === id); if (i >= 0) { freed += books[i].in_place ? 0 : books[i].size; books.splice(i, 1); } }
    free.bytes += freed;
    return { count: ids.length, freed_bytes: freed, files_to_delete: [] };
  },
  lib_read_ids: ({ series }) => books.filter((b) => b.read_status === 'read' && (series == null || b.series === series)).map((b) => b.id),
  lib_usage: () => {
    const read = books.filter((b) => b.read_status === 'read' && !b.in_place);
    return { books: books.length, app_bytes: books.reduce((s, b) => s + b.size, 0), read_bytes: read.reduce((s, b) => s + b.size, 0), read_count: read.length, free_bytes: free.bytes, low_space: free.bytes < 5 * GB };
  },
  set_read_status: ({ ids, status }) => { for (const id of ids) { const b = byId(id); if (b) { b.read_status = status; b.progress = status === 'read' ? 1 : null; } } },
  // Trình đọc dùng lại reader.js của app Windows.
  reader_info: ({ id }) => {
    const b = byId(id);
    const engine = { epub: 'foliate', mobi: 'foliate', fb2: 'foliate', cbz: 'comic', pdf: 'pdf', txt: 'text' }[b.format] || 'external';
    let prev = null, next = null, series_pos = null;
    if (b.series) {
      const v = books.filter((x) => x.series === b.series).sort(volSort), i = v.indexOf(b);
      const ref = (x) => (x ? { id: x.id, title: x.title, volume: x.volume } : null);
      prev = ref(v[i - 1]); next = ref(v[i + 1]); series_pos = [i + 1, v.length];
    }
    return { book: b, engine, locator: b.locator, progress: b.progress, prev, next, series_pos };
  },
  save_position: ({ id, locator, progress }) => {
    const b = byId(id);
    Object.assign(b, { locator, progress: progress >= 0.995 ? 1 : progress, last_read_at: Math.floor(Date.now() / 1000) });
    b.read_status = progress >= 0.995 || b.read_status === 'read' ? 'read' : 'reading';
    return b.read_status;
  },
  open_external: () => {},
  bookmark_list: ({ id }) => marks.filter((m) => m.book === id),
  bookmark_add: ({ id, locator, progress, label }) => { const m = { id: nextMark++, book: id, locator, progress, label, created_at: Math.floor(Date.now() / 1000) }; marks.push(m); return m; },
  bookmark_remove: ({ bookmark }) => { const i = marks.findIndex((m) => m.id === bookmark); if (i >= 0) marks.splice(i, 1); },
  // ?iphone=1 xem giao diện iPhone (chỉ sách chữ, nút nhanh bên trái).
  // ?android=1 xem bản điện thoại Android (bố cục iPhone + nút Quay lại của hệ thống).
  device_info: () => (DROID ? { kind: 'android', phone: true, name: 'Android', machine: 'mock' } : PHONE ? { kind: 'iphone', phone: true, name: 'iPhone', machine: 'mock' } : { kind: 'ipad', phone: false, name: 'iPad', machine: 'mock' }),
  // Đồng bộ (giả): ?paired=1 đã ghép sẵn, ?offline=1 không thấy máy tính, ?full=1 iPad đầy.
  sync_state: () => ({ paired: sync.paired, pc_name: sync.paired ? 'PC-NHA' : null, auto_sync: sync.auto, running: sync.running }),
  sync_pair: ({ payload, code }) => { if (payload === undefined && code !== '482913') throw new Error('Mã ghép sai hoặc đã hết hạn'); sync.paired = true; return handlers.sync_state(); },
  sync_unpair: () => { sync.paired = false; },
  sync_set_auto: ({ on }) => { sync.auto = on; },
  sync_check: () => {
    if (Q.has('offline')) return { reachable: false, error: 'Không kết nối được (timeout)', pc_name: 'PC-NHA', drive_connected: false, items: [], need_bytes: 0, free_bytes: free.bytes, enough_space: true };
    const need = sync.items.reduce((s, i) => s + i.size, 0);
    return { reachable: true, error: null, pc_name: 'PC-NHA', drive_connected: true, items: sync.items, need_bytes: need, free_bytes: free.bytes, enough_space: need + 512 * 1024 ** 2 <= free.bytes };
  },
  sync_run: () => {
    sync.running = true;
    const items = sync.items.slice(), total = items.reduce((s, i) => s + i.size, 0);
    let done = 0, bytes = 0, k = 0;
    const tick = () => {
      if (k >= items.length) { sync.running = false; sync.items = []; emit('sync_done', { report: { added: items.length, already: 0, failed: [], bytes: total, cancelled: false, drive_connected: true } }); return; }
      const it = items[k];
      bytes += it.size / 4;
      emit('sync_progress', { book_id: it.id, title: it.title, done, total: items.length, bytes_done: Math.min(bytes, total), bytes_total: total });
      if (bytes >= items.slice(0, k + 1).reduce((s, i) => s + i.size, 0)) { add({ title: it.title, authors: it.authors, format: it.format, size: it.size }); done++; k++; }
      setTimeout(tick, 250);
    };
    setTimeout(tick, 100);
  },
  sync_cancel: () => {},
  // ?empty=1 để xem màn hình trống.
};
if (new URLSearchParams(location.search).has('empty')) books.length = 0;
if (new URLSearchParams(location.search).has('low')) free.bytes = 2 * GB;

export async function invoke(cmd, args) {
  await new Promise((r) => setTimeout(r, 25));
  if (!handlers[cmd]) throw new Error('mock: thiếu lệnh ' + cmd);
  return structuredClone(handlers[cmd](args || {}) ?? null);
}
export async function listen(ev, cb) { listeners.push([ev, cb]); return () => {}; }
