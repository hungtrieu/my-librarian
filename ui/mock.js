// Backend giả để xem trước giao diện trong trình duyệt (không có window.__TAURI__).
// Chỉ dùng khi phát triển; app thật gọi lệnh Rust qua Tauri.
import { fold } from './ui-kit.js';

const now = Math.floor(Date.now() / 1000);
const listeners = {};
let nextId = 1;
const books = [];
const add = (o) => {
  const id = nextId++;
  const b = { id, authors: '', format: 'epub', size: 300000 + ((id * 7919) % 900000), category: null, series: null, volume: null, read_status: 'unread',
    progress: null, added_at: now - id * 3600, last_read_at: null, in_inbox: false, page_count: null, mtime: (now - id * 7200) * 1000, isbn: null, ...o };
  b.file_name ??= `${b.title}.${b.format}`;
  b.rel_path ??= [b.category, b.series, b.file_name].filter(Boolean).join('\\');
  b.path ??= 'E:\\books\\' + b.rel_path;
  books.push(b);
  return b;
};
const lit = [['Mắt Biếc', 'Nguyễn Nhật Ánh'], ['Tôi thấy hoa vàng trên cỏ xanh', 'Nguyễn Nhật Ánh'], ['Cho tôi xin một vé đi tuổi thơ', 'Nguyễn Nhật Ánh'], ['Số Đỏ', 'Vũ Trọng Phụng'],
  ['Truyện Kiều', 'Nguyễn Du'], ['Dế Mèn Phiêu Lưu Ký', 'Tô Hoài'], ['Nỗi buồn chiến tranh', 'Bảo Ninh'], ['Rừng Na Uy', 'Haruki Murakami'], ['Nhà Giả Kim', 'Paulo Coelho'],
  ['Trăm năm cô đơn', 'Gabriel García Márquez'], ['Bố già', 'Mario Puzo'], ['Hai số phận', 'Jeffrey Archer'], ['Chí Phèo', 'Nam Cao'], ['Vợ nhặt', 'Kim Lân']];
lit.forEach(([t, a], i) => add({ title: t, authors: a, category: 'Văn học', format: i % 4 === 3 ? 'pdf' : 'epub', read_status: i === 0 ? 'reading' : i === 3 ? 'read' : 'unread', progress: i === 0 ? 0.42 : null, last_read_at: i === 0 ? now - 600 : null }));
[['Steve Jobs', 'Walter Isaacson'], ['Đời viết văn của tôi', 'Nguyễn Hiến Lê'], ['Elon Musk', 'Walter Isaacson'], ['Becoming', 'Michelle Obama']]
  .forEach(([t, a]) => add({ title: t, authors: a, category: 'Tiểu sử - Hồi ký' }));
[['Clean Code', 'Robert C. Martin'], ['The Pragmatic Programmer', 'Andrew Hunt'], ['Refactoring', 'Martin Fowler'], ['Design Patterns', 'Erich Gamma'], ['Rust in Action', 'Tim McNamara']]
  .forEach(([t, a], i) => add({ title: t, authors: a, category: 'Công nghệ', format: i % 2 ? 'azw3' : 'pdf', read_status: i === 1 ? 'reading' : 'unread', progress: i === 1 ? 0.15 : null, last_read_at: i === 1 ? now - 9000 : null }));
for (const [s, n, fmt] of [['Doraemon', 45, 'cbz'], ['7 seeds', 35, 'mobi'], ['One Piece', 105, 'cbz'], ['Conan', 100, 'cbz'], ['Slam Dunk', 31, 'mobi']]) {
  for (let v = 1; v <= n; v++) add({ title: `${s} ${v}`, file_name: `${s} ${v}.${fmt}`, category: 'truyen tranh', series: s, volume: v, format: fmt, page_count: 180 + (v % 20), read_status: s === 'Doraemon' && v <= 12 ? 'read' : 'unread' });
}
for (let i = 1; i <= 160; i++) add({ title: `Sách mẫu số ${i}`, authors: `Tác giả ${i % 23}`, category: ['Văn học', 'Công nghệ', 'Tiểu sử - Hồi ký'][i % 3], format: ['epub', 'pdf', 'mobi', 'fb2', 'djvu'][i % 5] });
const inbox = [add({ title: 'Mắt Biếc', file_name: 'Mắt Biếc (1).epub', authors: 'Nguyễn Nhật Ánh', category: 'Chưa phân loại', in_inbox: true }),
  add({ title: 'Mat Biec', file_name: 'Mat Biec.pdf', category: 'Chưa phân loại', in_inbox: true, format: 'pdf', size: 15800000 }),
  add({ title: 'Ngày xưa có một chuyện tình', authors: 'Nguyễn Nhật Ánh', category: 'Chưa phân loại', in_inbox: true }),
  add({ title: '7 Seeds 36', file_name: '7 Seeds 36.mobi', category: 'Chưa phân loại', in_inbox: true, format: 'mobi' }),
  add({ title: 'Dragon Ball 1', file_name: 'Dragon Ball 1.cbz', category: 'Chưa phân loại', in_inbox: true, format: 'cbz' }),
  add({ title: 'Không rõ', file_name: 'Không rõ.txt', category: 'Chưa phân loại', in_inbox: true, format: 'txt' })];
add({ title: 'Sách hỏng', file_name: 'Sách hỏng.epub', category: 'Văn học', error: 'zip hỏng: invalid Zip archive' });
add({ title: 'Sách Kindle mua', authors: 'Amazon', category: 'Văn học', format: 'azw3', drm: true });
const byId = (id) => books.find((b) => b.id === id);
const present = () => books.filter((b) => !b.gone);
let config = { library_root: 'E:\\books', inbox_name: 'Chưa phân loại', extra_roots: [], exclude_names: ['$RECYCLE.BIN'], read_only: false, auto_clean_exact_in_background: false, auto_triage: true, classify_high: 75, classify_medium: 45, hash_threads: 3, scan_time: '03:00' };
const decided = new Set();
const TEXT = new Set(['epub', 'mobi', 'prc', 'azw', 'azw3', 'kf8', 'fb2', 'txt']);
// Hai thiết bị mẫu; ?nodevices=1 để xem lúc chưa ghép.
const ipad = { running: false, publicNet: true, devices: new URLSearchParams(location.search).has('nodevices') ? [] : [
  { id: 1, name: 'iPad Air', kind: 'ipad', created_at: now - 86400 * 9, last_seen: now - 600, free_bytes: 40e9, queue: [], has: [1, 2] },
  { id: 2, name: 'iPhone của Hưng', kind: 'iphone', created_at: now - 86400, last_seen: now - 7200, free_bytes: 9e9, queue: [], has: [2] }] };
const item = (id) => { const b = byId(id); return { id, title: b.title, authors: b.authors, series: b.series, volume: b.volume, format: b.format, size: b.size, sha256: id % 2 ? 'ab' : null }; };
const batches = [];
const report = () => ({ id: 7, kind: 'background', started_at: now - 3 * 3600, finished_at: now - 3 * 3600 + 80, status: 'done',
  stats: { scan: { added: 6, missing: 0, skipped_roots: [], cancelled: false }, total_books: present().length, total_bytes: present().reduce((s, b) => s + b.size, 0),
    exact_groups: exact().length, exact_files: exact().length, reclaimable_bytes: exact().reduce((s, g) => s + g.reclaimable, 0), suspect_groups: suspects().length, inbox_books: present().filter((b) => b.in_inbox).length, inbox_proposed: 3, missing_books: 0 } });
function exact() {
  const a = inbox[0], k = books[0];
  return a.gone || k.gone ? [] : [{ full_hash: 'ab12cd34ef56', size: k.size, keeper: { ...k, rel_path: k.rel_path }, keep_reason: 'đã nằm trong thư mục thể loại của thư viện', others: [{ ...a, size: k.size }], reclaimable: k.size }];
}
function suspects() {
  const m = [books[0], inbox[1]].filter((b) => !b.gone);
  return m.length === 2 && !decided.has('mb') ? [{ key: 'mb', reason: 'Cùng tên chuẩn hoá “mat biec”', members: m }] : [];
}
function query(q) {
  let r = present();
  if (q.q) {
    const toks = fold(q.q).split(/[^\p{L}\p{N}]+/u).filter(Boolean);
    r = r.filter((b) => { const words = fold(`${b.title} ${b.authors} ${b.file_name} ${b.series || ''}`).split(/[^\p{L}\p{N}]+/u); return toks.every((t) => words.some((w) => w.startsWith(t))); });
  }
  if (q.category != null) r = r.filter((b) => (q.category === '' ? !b.category : fold(b.category) === fold(q.category)));
  if (q.format) r = r.filter((b) => b.format === q.format);
  if (q.read_status) r = r.filter((b) => b.read_status === q.read_status);
  if (q.problems) r = r.filter((b) => b.error || b.drm);
  if (q.on_ipad) r = r.filter((b) => b.on_ipad);
  r = r.filter((b) => !!b.not_worth === !!q.not_worth);
  if (q.series) r = r.filter((b) => b.series === q.series).sort((a, b) => a.volume - b.volume);
  let items = r.map((b) => ({ kind: 'book', ...b }));
  if (q.group_series && !q.series) {
    const g = new Map();
    items = [];
    for (const b of r) { if (b.series) { const k = b.category + '|' + b.series; (g.get(k) || g.set(k, []).get(k)).push(b); } else if (!q.only_series) items.push({ kind: 'book', ...b }); }
    for (const vols of g.values()) {
      vols.sort((a, b) => a.volume - b.volume);
      const lastRead = vols.map((v) => v.read_status).lastIndexOf('read');
      items.push({ kind: 'series', category: vols[0].category, series: vols[0].series, volumes: vols.length, read: vols.filter((v) => v.read_status === 'read').length,
        cover_book_id: vols[0].id, continue_book_id: lastRead >= 0 ? vols[lastRead + 1]?.id : null, size: vols.reduce((s, v) => s + v.size, 0), authors: '', title: vols[0].series, added_at: vols[0].added_at, last_read_at: null });
    }
  }
  const t = (x) => x.title || x.series;
  const cmp = { added: (a, b) => b.added_at - a.added_at, relevance: (a, b) => b.added_at - a.added_at, title: (a, b) => t(a).localeCompare(t(b), 'vi', { numeric: true }),
    author: (a, b) => (a.authors || '').localeCompare(b.authors || '', 'vi'), recent_read: (a, b) => (b.last_read_at || 0) - (a.last_read_at || 0), size: (a, b) => b.size - a.size }[q.sort || 'added'];
  if (!q.series) items.sort(cmp);
  return { total: items.length, offset: q.offset || 0, items: items.slice(q.offset || 0, (q.offset || 0) + (q.limit || 200)) };
}
const cats = () => ['Công nghệ', 'Tiểu sử - Hồi ký', 'Văn học', 'truyen tranh'].map((name) => ({ name, books: present().filter((b) => b.category === name).length,
  series_dirs: name === 'truyen tranh' ? ['7 seeds', 'Conan', 'Doraemon', 'One Piece', 'Slam Dunk'] : [], path: 'E:\\books\\' + name }));
const trash = (ids) => { const t = ids.map(byId).filter(Boolean); t.forEach((b) => { b.gone = true; }); batches.unshift({ id: batches.length + 1, kind: 'trash', created_at: Math.floor(Date.now() / 1000), undone_at: null, done: t.length, failed: 0, bytes: t.reduce((s, b) => s + b.size, 0), ops: t.map((b) => ({ id: b.id, kind: 'trash', src: b.path, dst: null, status: 'done' })) });
  return { batch_id: batches.length, trashed: t.length, freed_bytes: t.reduce((s, b) => s + b.size, 0), failures: [], aborted: null }; };

const handlers = {
  overview: () => ({ config, library_ok: !!config.library_root, home: 'C:\\Users\\ban\\AppData\\Roaming\\my-librarian', scanning: false, categories: cats(), report: report(),
    banner: `Từ lần quét ngầm gần nhất: ${suspects().length} nhóm nghi trùng, ${exact().length} bản trùng chắc chắn, ${present().filter((b) => b.in_inbox).length} sách mới chờ phân loại.`,
    facets: { total: present().length, bytes: 0, inbox: present().filter((b) => b.in_inbox).length, missing: 0, problems: present().filter((b) => b.error || b.drm).length, on_ipad: present().filter((b) => b.on_ipad).length,
      not_worth: present().filter((b) => b.not_worth).length,
      formats: Object.entries(present().reduce((m, b) => ((m[b.format] = (m[b.format] || 0) + 1), m), {})).sort((a, b) => b[1] - a[1]),
      read_status: Object.entries(present().filter((b) => !b.not_worth).reduce((m, b) => ((m[b.read_status] = (m[b.read_status] || 0) + 1), m), {})) } }),
  query: ({ q }) => query(q),
  book: ({ id }) => byId(id),
  set_not_worth: ({ ids, on }) => ids.filter((id) => { const b = byId(id); if (!b || !!b.not_worth === on) return false; b.not_worth = on; return true; }).length,
  set_read_status: ({ ids, status }) => ids.forEach((id) => { const b = byId(id); b.read_status = status; b.last_read_at = Math.floor(Date.now() / 1000); }),
  exact_groups: exact,
  clean_exact: () => trash(exact().flatMap((g) => g.others.map((o) => o.id))),
  suspect_groups: suspects,
  decide: () => { decided.add('mb'); return 1; },
  trash_books: ({ ids }) => trash(ids),
  propose: () => ({ targets: cats().flatMap((c) => [{ label: c.name, path: c.path }, ...c.series_dirs.map((s) => ({ label: `${c.name} › ${s}`, path: c.path + '\\' + s }))]),
    proposals: inbox.filter((b) => !b.gone).map((b) => {
      const p = { book: b, target: null, target_label: null, confidence: 0, bucket: 'Manual', preselected: false, reason: 'Không đủ thông tin để đoán thể loại', suggestion: null };
      if (b.file_name.startsWith('7 Seeds')) Object.assign(p, { target: 'E:\\books\\truyen tranh\\7 seeds', target_label: 'truyen tranh\\7 seeds', confidence: 95, bucket: 'High', preselected: true, reason: 'Khớp bộ truyện “7 seeds”' });
      if (b.title.startsWith('Ngày xưa')) Object.assign(p, { target: 'E:\\books\\Văn học', target_label: 'Văn học', confidence: 91, bucket: 'High', preselected: true, reason: '3 sách cùng tác giả đang ở mục này' });
      if (b.title === 'Mat Biec') Object.assign(p, { target: 'E:\\books\\Văn học', target_label: 'Văn học', confidence: 62, bucket: 'Medium', reason: 'Từ khoá khớp: mat, biec' });
      if (b.file_name.startsWith('Dragon')) Object.assign(p, { reason: 'Truyện chưa có thư mục bộ', suggestion: 'Bộ mới “dragon ball” – tạo thư mục trong “truyen tranh”' });
      if (b.file_name.includes('(1)')) Object.assign(p, { reason: 'Trùng chắc chắn với sách đã có trong thư viện', suggestion: 'Dọn ở màn Trùng chắc chắn' });
      return p;
    }) }),
  apply_moves: ({ moves, kind }) => {
    const ops = moves.map((m) => { const b = byId(m.book_id); const from = b.path; const cat = m.target_dir.split('\\')[2]; const dst = m.target_dir + '\\' + b.file_name; Object.assign(b, { prev: { ...b }, category: cat, in_inbox: false, path: dst, rel_path: dst.split('\\').slice(2).join('\\') }); return { id: b.id, kind: 'move', src: from, dst: m.target_dir + '\\' + b.file_name, status: 'done', book_id: b.id }; });
    batches.unshift({ id: batches.length + 1, kind: kind || 'classify', created_at: Math.floor(Date.now() / 1000), undone_at: null, done: ops.length, failed: 0, bytes: 0, ops });
    return { batch_id: batches.length, moved: ops.map((o) => ({ book_id: o.book_id, from: o.src, to: o.dst })), failures: [] };
  },
  undo_batch: ({ id }) => { const b = batches.find((x) => x.id === id); b.undone_at = Math.floor(Date.now() / 1000); b.ops.forEach((o) => { const x = byId(o.book_id); if (x?.prev) Object.assign(x, x.prev, { prev: null }); o.status = 'undone'; }); return { batch_id: id, restored: b.ops.length, failures: [] }; },
  list_batches: () => batches,
  batch_ops: ({ id }) => batches.find((x) => x.id === id).ops,
  evaluate: () => ({ total: 186, correct: 161, high_total: 120, high_correct: 117, medium_total: 48, medium_correct: 38, manual: 18 }),
  report_history: () => [report()],
  save_config: (a) => { config = a.config; },
  pick_folder: () => 'E:\\books',
  set_library: ({ path }) => { config.library_root = path; },
  start_scan: () => {
    // `?stall` trên URL: giả lập một bước đứng yên tới khi bộ giám sát tự dừng quét (xem progress.rs).
    if (new URLSearchParams(location.search).has('stall')) {
      let idle = 0;
      emit('scan-progress', { phase: 'hash_full', done: 3, total: 10 });
      const t = setInterval(() => {
        idle += 6;
        const stalled = idle >= 60;
        emit('scan-health', { phase: 'hash_full', label: 'Băm đầy đủ', done: 3, total: 10, idle_secs: idle, limit_secs: 60, stalled });
        if (stalled) {
          clearInterval(t);
          setTimeout(() => emit('scan-done', { ok: false, message: 'Bước “Băm đầy đủ” không có tiến triển trong 60 giây (đang ở 3 / 10), mục đang xử lý: D:\\books\\Khác\\sach-lon.pdf. Đã dừng quét – phần đã làm được giữ lại.' }), 400);
        }
      }, 120);
      return;
    }
    let n = 0;
    const tick = setInterval(() => { n += 437; emit('scan-progress', n <= 1200 ? { phase: 'scan', done: n, total: 1200 } : n <= 2000 ? { phase: 'triage', done: n - 1200, total: 800 } : { phase: 'hash_quick', done: Math.min(600, n - 2000), total: 600 }); if (n > 2600) { clearInterval(tick); emit('scan-done', { ok: true, stats: report().stats }); } }, 250);
  },
  cancel_scan: () => {},
  reveal: () => {}, open_external: () => {}, open_recycle_bin: () => {},
  // Trình đọc: nội dung lấy từ file mẫu do scripts/serve-ui.mjs phục vụ ở /__book/<định dạng>/…
  reader_info: ({ id }) => {
    const b = byId(id);
    const engine = { epub: 'foliate', mobi: 'foliate', azw: 'foliate', azw3: 'foliate', fb2: 'foliate', cbz: 'comic', cbr: 'comic', pdf: 'pdf', txt: 'text' }[b.format] || 'external';
    let prev = null, next = null, series_pos = null;
    if (b.series) {
      const vols = present().filter((x) => x.series === b.series && x.category === b.category).sort((x, y) => x.volume - y.volume);
      const i = vols.indexOf(b);
      const ref = (v) => (v ? { id: v.id, title: v.title, volume: v.volume } : null);
      prev = ref(vols[i - 1]); next = ref(vols[i + 1]); series_pos = [i + 1, vols.length];
    }
    return { book: b, engine, locator: b.locator || null, progress: b.progress, prev, next, series_pos };
  },
  save_position: ({ id, locator, progress }) => {
    const b = byId(id);
    Object.assign(b, { locator, progress: progress >= 0.995 ? 1 : progress, last_read_at: Math.floor(Date.now() / 1000) });
    b.read_status = progress >= 0.995 || b.read_status === 'read' ? 'read' : 'reading';
    return b.read_status;
  },
  // Đồng bộ iPad (giả): hàng chờ trong bộ nhớ.
  // Mỗi thiết bị một hàng chờ (Map book_id → sent); `has` = sách đang có trên máy.
  ipad_state: () => ({ running: ipad.running, port: 47470, addrs: ['192.168.1.20'], on_device: [...new Set(ipad.devices.flatMap((d) => d.has))],
    devices: ipad.devices.map(({ queue, has, ...d }) => { const q = queue.map(item); return { ...d, queue: q, queue_bytes: q.reduce((s, i) => s + i.size, 0), books: has.length }; }) }),
  ipad_devices: () => ipad.devices.map(({ queue, has, ...d }) => d),
  // Mạng Công cộng (giả): bấm Sửa thì thành Riêng tư.
  ipad_net_check: () => ({ public: ipad.publicNet ? [{ alias: 'Wi-Fi', name: 'Nhà', index: 13 }] : [], rule: !ipad.publicNet }),
  ipad_net_fix: () => { ipad.publicNet = false; return { public: [], rule: true }; },
  ipad_device_books: ({ id }) => ipad.devices.find((d) => d.id === id).has.map(item),
  book_devices: ({ id }) => ipad.devices.filter((d) => d.has.includes(id)).map((d) => d.name),
  ipad_rename_device: ({ id, name }) => { if (!name.trim()) throw new Error('Tên thiết bị không được để trống'); ipad.devices.find((d) => d.id === id).name = name.trim(); return name.trim(); },
  ipad_queue_add: ({ ids, devices }) => {
    if (!devices?.length) throw new Error('Chưa chọn thiết bị nào');
    const r = { added: 0, books: 0, skipped_unsupported: 0, skipped_device: 0, skipped_missing: 0 };
    for (const id of ids) {
      const b = byId(id); if (!b) { r.skipped_missing++; continue; }
      if (['cbr', 'djvu'].includes(b.format)) { r.skipped_unsupported++; continue; }
      let any = false;
      for (const d of ipad.devices.filter((x) => devices.includes(x.id))) {
        if (d.kind === 'iphone' && !TEXT.has(b.format)) { r.skipped_device++; continue; }
        if (!d.queue.includes(id)) d.queue.push(id);
        r.added++; any = true;
      }
      if (any) r.books++;
    }
    return r;
  },
  ipad_queue_remove: ({ ids, device }) => { for (const d of ipad.devices) if (device == null || d.id === device) d.queue = d.queue.filter((x) => !ids.includes(x)); return ids.length; },
  ipad_remove_device: ({ id }) => { ipad.devices = ipad.devices.filter((d) => d.id !== id); },
  toggle_fullscreen: () => false,
  take_open_request: () => null,
  open_path: ({ path }) => { throw new Error('mock: không mở được ' + path); },
};

function emit(ev, payload) { (listeners[ev] || []).forEach((cb) => cb({ payload })); }

export async function invoke(cmd, args) {
  await new Promise((r) => setTimeout(r, 30));
  if (!handlers[cmd]) throw new Error('mock: thiếu lệnh ' + cmd);
  return structuredClone(handlers[cmd](args || {}) ?? null);
}

export async function listen(ev, cb) { (listeners[ev] ||= []).push(cb); return () => {}; }
