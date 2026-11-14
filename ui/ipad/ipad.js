// My Librarian cho iPad – giao diện thư viện (I1-04..I1-08). Nghiệp vụ ở Rust (ipad_lib.rs); file này chỉ hiển thị và gọi lệnh.
import { h, fmtBytes, debounce, genCover } from '../ui-kit.js';
import { openReader } from '../reader.js';

const TAURI = window.__TAURI__;
const api = TAURI ? { invoke: TAURI.core.invoke, listen: TAURI.event.listen } : await import('./mock.js');
const call = (cmd, args = {}) => api.invoke(cmd, args);
const bookBase = (b) => (TAURI ? `${/Android|Windows/.test(navigator.userAgent) ? 'http://book.localhost' : 'book://localhost'}/${b.id}/` : `/__book/${b.format}/`);
const $ = (s) => document.querySelector(s);
const pct = (f) => Math.round((f || 0) * 100) + '%';
// iPad, iPhone hay điện thoại Android (cùng một app). Điện thoại: chỉ sách chữ, thanh nút nhanh của trình đọc dồn về góc
// dưới trái (cầm tay trái). Android dùng lại bố cục iPhone: body có thêm lớp `iphone` bên cạnh `android`.
const DEV = await call('device_info').catch(() => ({ kind: 'ipad', name: 'iPad' }));
const PHONE = DEV.phone ?? DEV.kind === 'iphone';
const ANDROID = DEV.kind === 'android';
document.body.classList.add(DEV.kind);
if (PHONE) document.body.classList.add('iphone');
document.title = `My Librarian – ${DEV.name}`;
const SEARCH_ICON = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round"><circle cx="10.5" cy="10.5" r="6.5"/><path d="M15.5 15.5 21 21"/></svg>';

// Lưới bìa hay danh sách: lưu trong localStorage của WebView (như tuỳ chọn trình đọc) nên giữ nguyên lần mở app sau.
const LAYOUT_KEY = 'ipad.layout';
const loadLayout = () => { try { return localStorage.getItem(LAYOUT_KEY) === 'list' ? 'list' : 'grid'; } catch { return 'grid'; } };
const S = { sync: { st: null, check: null, prog: null, msg: null, busy: false }, q: '', sort: 'added', layout: loadLayout(), tab: 'reading', view: { name: 'library' }, items: [], reading: [], nwCount: 0, usage: null };
const SORTS = [['added', 'Mới thêm'], ['recent', 'Đọc gần đây'], ['title', 'Tên']];

function toast(msg) {
  const t = h('div', { class: 'toast' }, msg);
  $('#toasts').append(t);
  setTimeout(() => t.remove(), 3500);
}
const fail = (e) => toast(String(e?.message || e));

// ------------------------------------------------------------------ chạm / nhấn giữ
// WebView của iOS/iPadOS không phát `contextmenu` khi nhấn giữ bằng ngón tay (chỉ khi bấm chuột phải), nên tự đếm giờ
// nhấn giữ bằng pointer events. Android có phát `contextmenu` khi nhấn giữ: `held` chặn mở menu hai lần và chặn cú
// chạm (click) theo sau.
const HOLD_MS = 500;
function pressable(tap, hold) {
  let timer = 0, held = false, x = 0, y = 0;
  const cancel = () => { clearTimeout(timer); timer = 0; };
  const fire = () => { cancel(); if (held) return; held = true; try { navigator.vibrate?.(10); } catch { /* không hỗ trợ */ } hold(); };
  return {
    onpointerdown: (e) => { held = false; if (e.pointerType === 'mouse') return; x = e.clientX; y = e.clientY; cancel(); timer = setTimeout(fire, HOLD_MS); },
    onpointermove: (e) => { if (timer && Math.hypot(e.clientX - x, e.clientY - y) > 10) cancel(); },
    onpointerup: cancel,
    onpointercancel: cancel,
    oncontextmenu: (e) => { e.preventDefault(); if (!held) fire(); },
    onclick: (e) => { if (held) { e.preventDefault(); held = false; return; } tap(e); },
  };
}

// ------------------------------------------------------------------ thẻ sách
// Bìa thật (EPUB, MOBI/AZW, CBZ/CBR, FB2) trích từ file trên thiết bị qua book://<id>/cover; PDF/TXT hoặc sách không có
// bìa dùng bìa tự sinh, giống app Windows.
const COVER_FORMATS = new Set(['epub', 'mobi', 'azw', 'azw3', 'cbz', 'cbr', 'fb2']);
function cover(b, { id = b.id, format = b.format, title = b.title || b.series, authors = b.authors } = {}) {
  const gen = () => genCover(title, authors, format);
  if (!TAURI || id == null || !COVER_FORMATS.has(format)) return gen();
  const img = h('img', { class: 'cover', src: `${bookBase({ id })}cover`, alt: '', loading: 'lazy', decoding: 'async', draggable: 'false' });
  img.onerror = () => img.replaceWith(gen());
  return img;
}
const bar = (f) => (f > 0 && f < 1 ? h('div', { class: 'pbar', 'aria-hidden': 'true' }, h('i', { style: `width:${pct(f)}` })) : null);

function bookCard(b, { showVol = false } = {}) {
  const done = b.read_status === 'read';
  return h('button', { class: 'tile', ...pressable(() => read(b.id), () => bookSheet(b)), 'aria-label': b.title },
    h('div', { class: 'cv' }, cover(b), bar(b.progress), done ? h('span', { class: 'done', 'aria-label': 'Đã đọc' }, '✓') : null),
    h('div', { class: 'tt' }, showVol && b.volume != null ? `Vol ${b.volume}` : b.title),
    h('div', { class: 'sub' }, showVol ? (done ? 'Đã đọc' : b.progress ? pct(b.progress) : b.format.toUpperCase()) : b.authors || b.format.toUpperCase()));
}

function seriesTile(s) {
  return h('button', { class: 'tile series', ...pressable(() => go({ name: 'series', series: s.series, notWorth: S.tab === 'not_worth' }), () => seriesSheet(s)), 'aria-label': `Bộ ${s.series}` },
    h('div', { class: 'cv sstack' }, h('div', { class: 'under u2' }), h('div', { class: 'under u1' }), h('div', { class: 'cvtop' }, cover(s, { id: s.cover_book_id, format: s.cover_format, title: s.series, authors: `${s.volumes} tập` })),
      h('span', { class: 'vbadge' }, `${s.read}/${s.volumes}`)),
    h('div', { class: 'tt' }, s.series),
    h('div', { class: 'sub' }, s.read === s.volumes ? 'Đã đọc hết' : `${s.volumes} tập`));
}

// ------------------------------------------------------------------ chế độ danh sách: bìa nhỏ, tên, tác giả, định dạng, bộ, tiến độ
function progressCell(b) {
  if (b.read_status === 'read') return h('div', { class: 'lprog done-txt' }, '✓ Đã đọc');
  if (b.progress > 0) return h('div', { class: 'lprog' }, h('span', { class: 'lbar', 'aria-hidden': 'true' }, h('i', { style: `width:${pct(b.progress)}` })), h('span', {}, pct(b.progress)));
  return h('div', { class: 'lprog dim2' }, fmtBytes(b.size));
}

function bookRow(b, { showVol = false } = {}) {
  const where = b.series ? (b.volume != null ? `${b.series} · Vol ${b.volume}` : b.series) : null;
  return h('button', { class: 'lrow', ...pressable(() => read(b.id), () => bookSheet(b)), 'aria-label': b.title },
    h('div', { class: 'lcv' }, cover(b)),
    h('div', { class: 'lmain' },
      h('div', { class: 'ltt' }, showVol && b.volume != null ? `Vol ${b.volume}` : b.title),
      h('div', { class: 'lsub' }, [b.authors, showVol ? null : where].filter(Boolean).join(' · ') || '—'),
      h('div', { class: 'lmeta' }, h('span', { class: 'fmt' }, b.format.toUpperCase()), PHONE ? progressCell(b) : null)),
    PHONE ? null : progressCell(b));
}

function seriesRow(s) {
  const done = s.read === s.volumes;
  const prog = h('div', { class: 'lprog' + (done ? ' done-txt' : '') }, done ? '✓ Đã đọc hết' :
    s.read ? [h('span', { class: 'lbar', 'aria-hidden': 'true' }, h('i', { style: `width:${Math.round((100 * s.read) / s.volumes)}%` })), h('span', {}, `${s.read}/${s.volumes}`)] : `${s.volumes} tập`);
  return h('button', { class: 'lrow', ...pressable(() => go({ name: 'series', series: s.series, notWorth: S.tab === 'not_worth' }), () => seriesSheet(s)), 'aria-label': `Bộ ${s.series}` },
    h('div', { class: 'lcv sstack' }, h('div', { class: 'under u1' }), h('div', { class: 'cvtop' }, cover(s, { id: s.cover_book_id, format: s.cover_format, title: s.series, authors: '' }))),
    h('div', { class: 'lmain' },
      h('div', { class: 'ltt' }, s.series),
      h('div', { class: 'lsub' }, `Bộ · ${s.volumes} tập · ${fmtBytes(s.size)}`),
      PHONE ? h('div', { class: 'lmeta' }, prog) : null),
    PHONE ? h('span', { class: 'chev', 'aria-hidden': 'true' }, '›') : prog);
}

/** Lưới bìa hoặc danh sách cho một tập mục (sách lẻ + ô bộ truyện). */
function itemsView(items, opts) {
  if (S.layout === 'list') return h('div', { class: 'rows' }, items.map((i) => (i.kind === 'series' ? seriesRow(i) : bookRow(i, opts))));
  return h('div', { class: 'grid' }, items.map((i) => (i.kind === 'series' ? seriesTile(i) : bookCard(i, opts))));
}

/** Nút bật tắt lưới / danh sách (vùng chạm ≥ 44pt). */
function layoutToggle() {
  const opt = (mode, label, svg) => {
    const b = h('button', { class: S.layout === mode ? 'on' : '', 'aria-pressed': S.layout === mode ? 'true' : 'false', 'aria-label': label, title: label,
      onclick: () => { if (S.layout === mode) return; S.layout = mode; try { localStorage.setItem(LAYOUT_KEY, mode); } catch { /* bỏ qua */ } render(); } });
    b.innerHTML = svg;
    return b;
  };
  return h('div', { class: 'seg lay', role: 'group', 'aria-label': 'Kiểu hiển thị' },
    opt('grid', 'Lưới bìa', '<svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><rect x="3" y="3" width="8" height="8" rx="1.5"/><rect x="13" y="3" width="8" height="8" rx="1.5"/><rect x="3" y="13" width="8" height="8" rx="1.5"/><rect x="13" y="13" width="8" height="8" rx="1.5"/></svg>'),
    opt('list', 'Danh sách', '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" aria-hidden="true"><path d="M9 6h12M9 12h12M9 18h12"/><circle cx="4" cy="6" r="1.3" fill="currentColor"/><circle cx="4" cy="12" r="1.3" fill="currentColor"/><circle cx="4" cy="18" r="1.3" fill="currentColor"/></svg>'));
}

// ------------------------------------------------------------------ trình đọc
async function read(id) {
  await openReader(id, {
    call, toast, fail, bookBase, bookmarks: true, mobile: true, leftHand: PHONE, phone: PHONE,
    modal: async ({ title }) => confirm(title),
    onClose: () => refresh(),
    onDelete: async (b) => { await removeBooks([b.id], b.title); return true; },
  });
}

// ------------------------------------------------------------------ xoá / dung lượng (I1-07, I1-08)
async function removeBooks(ids, label) {
  if (!confirm(`Xoá “${label}” khỏi ${DEV.name}?\nVị trí đọc được giữ lại; gửi lại từ máy tính sẽ mở đúng chỗ cũ.`)) return;
  try {
    const r = await call('lib_remove', { ids });
    toast(`Đã xoá ${r.count} sách, giải phóng ${fmtBytes(r.freed_bytes)}.`);
  } catch (e) { fail(e); }
  closeSheet();
  if (S.view.name === 'series' && !(await call('lib_series', { series: S.view.series })).length) S.view = { name: 'library' };
  refresh().then(autoSync);
}

function sheet(title, ...children) {
  closeSheet();
  const root = $('#sheet-root');
  root.append(h('div', { class: 'scrim', onclick: closeSheet }), h('div', { class: 'sheet', role: 'dialog', 'aria-label': title },
    h('div', { class: 'grab' }), h('h2', {}, title), ...children));
}
const closeSheet = () => { $('#sheet-root').replaceChildren(); };
const action = (label, fn, cls = '') => h('button', { class: 'act ' + cls, onclick: fn }, label);

function bookSheet(b) {
  sheet(b.title,
    h('p', { class: 'dim' }, [b.authors, b.format.toUpperCase(), fmtBytes(b.size)].filter(Boolean).join(' · ')),
    action('Đọc', () => { closeSheet(); read(b.id); }),
    b.read_status === 'read' ? null : action('Đánh dấu đã đọc', async () => { await call('set_read_status', { ids: [b.id], status: 'read' }); closeSheet(); refresh(); }),
    b.not_worth ? action('Đáng đọc – đưa lại vào thư viện', () => markNotWorth([b.id], false)) : action('Không đáng đọc', () => markNotWorth([b.id], true)),
    action(b.in_place ? 'Gỡ khỏi thư viện' : `Xoá khỏi ${DEV.name}`, () => removeBooks([b.id], b.title), 'danger'));
}

async function seriesSheet(s) {
  const readIds = await call('lib_read_ids', { series: s.series });
  const nw = S.tab === 'not_worth';
  const vols = (await call('lib_series', { series: s.series })).filter((b) => !!b.not_worth === nw).map((b) => b.id);
  sheet(s.series, h('p', { class: 'dim' }, `${s.volumes} tập · ${s.read} đã đọc · ${fmtBytes(s.size)}`),
    s.continue_book_id ? action('Đọc tiếp', () => { closeSheet(); read(s.continue_book_id); }) : null,
    vols.length ? action(nw ? 'Đưa cả bộ lại vào thư viện' : 'Cả bộ không đáng đọc', () => markNotWorth(vols, !nw)) : null,
    readIds.length ? action(`Xoá ${readIds.length} tập đã đọc`, () => removeBooks(readIds, `${readIds.length} tập đã đọc của ${s.series}`), 'danger') : null);
}

// ------------------------------------------------------------------ "không đáng đọc"
/** Ẩn sách khỏi thư viện (chuyển sang tab "Không đáng đọc") hoặc đưa lại; báo ngay cho máy tính nếu đang thấy máy. */
async function markNotWorth(ids, on) {
  try {
    await call('lib_set_not_worth', { ids, on });
    const what = ids.length > 1 ? `${ids.length} sách` : 'sách';
    toast(on ? `Đã chuyển ${what} sang “Không đáng đọc”.` : `Đã đưa ${what} lại vào thư viện.`);
  } catch (e) { fail(e); }
  closeSheet();
  if (S.view.name === 'series' && !(await call('lib_series', { series: S.view.series })).some((b) => !!b.not_worth === !!S.view.notWorth)) S.view = { name: 'library' };
  await refresh();
  pushMarks();
}

/** Đồng bộ cờ "không đáng đọc" với máy tính (im lặng; máy tính tắt thì để lần sau). */
let marksBusy = false, marksAt = 0;
async function pushMarks() {
  if (marksBusy) return;
  marksBusy = true; marksAt = Date.now();
  try { if ((await call('sync_marks')) > 0) await refresh(); } catch { /* máy tính không bật là chuyện bình thường */ }
  marksBusy = false;
}

async function storageSheet() {
  const u = await call('lib_usage');
  const readIds = await call('lib_read_ids', { series: null });
  sheet('Dung lượng',
    u.low_space ? h('p', { class: 'alert' }, `${DEV.name} chỉ còn ${fmtBytes(u.free_bytes)} trống. Xoá bớt sách đã đọc để lấy lại chỗ.`) : null,
    h('dl', { class: 'usage' }, h('dt', {}, `Sách trên ${DEV.name}`), h('dd', {}, `${u.books} cuốn`),
      h('dt', {}, 'Đang chiếm'), h('dd', {}, fmtBytes(u.app_bytes)),
      h('dt', {}, `${DEV.name} còn trống`), h('dd', {}, fmtBytes(u.free_bytes)),
      h('dt', {}, 'Đã đọc xong'), h('dd', {}, `${u.read_count} cuốn · ${fmtBytes(u.read_bytes)}`)),
    u.read_count ? action(`Xoá ${u.read_count} sách đã đọc (lấy lại ${fmtBytes(u.read_bytes)})`, () => removeBooks(readIds, `${u.read_count} sách đã đọc`), 'danger') : null,
    action('Đóng', closeSheet));
}

// ------------------------------------------------------------------ màn hình
function go(view) { S.view = view; refresh(); scrollTo(0, 0); }

async function refresh() {
  try {
    if (S.view.name === 'sync') {
      S.sync.st = await call('sync_state');
    } else if (S.view.name === 'series') {
      S.vols = (await call('lib_series', { series: S.view.series })).filter((b) => !!b.not_worth === !!S.view.notWorth);
    } else {
      [S.items, S.reading, S.nwCount] = await Promise.all([call('lib_list', { q: S.q, sort: S.sort, notWorth: S.tab === 'not_worth' }),
        S.q ? [] : call('lib_continue', { limit: 12 }), call('lib_not_worth_count').catch(() => 0)]);
      if (S.tab === 'not_worth' && !S.nwCount && !S.q) { S.tab = 'all'; return refresh(); }
    }
    S.usage = await call('lib_usage');
    if (S.view.name === 'sync' && S.sync.st.paired && !S.sync.check && !S.sync.busy) checkPc();
  } catch (e) { fail(e); }
  render();
}

/** Tiêu đề trang nằm trong nội dung (cuộn theo), chừa vùng giờ/pin phía trên. */
function pageTitle() {
  return h('h1', { class: 'ptitle' }, S.view.name === 'sync' ? 'Đồng bộ' : S.view.name === 'series' ? S.view.series : 'Thư viện');
}

/** Thanh công cụ cố định ở đáy màn hình, nằm trên thanh Home – không bị giờ/Wi-Fi/pin che. */
function toolbar() {
  const back = S.view.name === 'series' || S.view.name === 'sync';
  const input = h('input', { type: 'search', class: 'search', placeholder: PHONE ? 'Tìm tên, tác giả' : 'Tìm tên, tác giả, bộ truyện', value: S.q, 'aria-label': 'Tìm sách',
    oninput: debounce((e) => { S.q = e.target.value; refresh(); }, 150) });
  const glass = h('span', { class: 'glass', 'aria-hidden': 'true' });
  glass.innerHTML = SEARCH_ICON;
  const search = h('label', { class: 'searchbox' }, glass, input);
  const tool = (icon, label, onclick, dot, on = false) => h('button', { class: 'icon' + (on ? ' on' : ''), onclick, 'aria-label': label },
    dot, h('span', { class: 'ic', 'aria-hidden': 'true' }, icon), h('span', { class: 'lb' }, label));
  return h('nav', { class: 'top' },
    back ? h('button', { class: 'back', onclick: () => go({ name: 'library' }) }, h('span', { class: 'ic', 'aria-hidden': 'true' }, '‹'), h('span', { class: 'lb' }, 'Thư viện')) : h('span'),
    back ? h('span') : search,
    h('div', { class: 'tools' },
      tool('⇅', 'Đồng bộ', () => go({ name: 'sync' }), S.sync.prog ? h('span', { class: 'dot busy' }) : null, S.view.name === 'sync'),
      tool('▤', 'Dung lượng', storageSheet, S.usage?.low_space ? h('span', { class: 'dot' }) : null)));
}

function sortBar() {
  return h('div', { class: 'seg', role: 'tablist' }, SORTS.map(([k, l]) =>
    h('button', { role: 'tab', 'aria-selected': S.sort === k ? 'true' : 'false', class: S.sort === k ? 'on' : '', onclick: () => { S.sort = k; refresh(); } }, l)));
}

/** Tab đang hiện: Đang đọc (khi có sách đang đọc dở), Tất cả, Không đáng đọc (khi có sách đã đánh dấu). */
function curTab() {
  if (S.tab === 'not_worth') return 'not_worth';
  return !S.q && S.tab === 'reading' && S.reading.length ? 'reading' : 'all';
}

function tabBar() {
  const tabs = [S.reading.length ? ['reading', `Đang đọc (${S.reading.length})`] : null, ['all', 'Tất cả'], S.nwCount ? ['not_worth', `Không đáng đọc (${S.nwCount})`] : null].filter(Boolean);
  if (tabs.length === 1) return h('h2', {}, 'Tất cả');
  const cur = curTab();
  // Tab "Không đáng đọc" lấy danh sách riêng → tải lại; hai tab kia dùng dữ liệu đã có.
  const pick = (k) => { if (cur === k) return; const reload = k === 'not_worth' || cur === 'not_worth'; S.tab = k; scrollTo(0, 0); if (reload) refresh(); else render(); };
  return h('div', { class: 'seg tabs', role: 'tablist', 'aria-label': 'Nhóm sách' }, tabs.map(([k, l]) =>
    h('button', { role: 'tab', 'aria-selected': cur === k ? 'true' : 'false', class: cur === k ? 'on' : '', onclick: () => pick(k) }, l)));
}

function empty() {
  return h('div', { class: 'blank' }, h('div', { class: 'big', 'aria-hidden': 'true' }, '📚'),
    h('h2', {}, S.q ? 'Không tìm thấy sách nào' : `Chưa có sách trên ${DEV.name}`),
    S.q ? h('p', {}, `Không có kết quả cho “${S.q}”.`) : h('p', {}, 'Trên máy tính Windows mở My Librarian → iPad / iPhone, ghép máy này, chọn sách gửi sang, rồi quét mã QR ở đây để kết nối.'),
    S.q ? null : h('button', { class: 'cta', onclick: () => go({ name: 'sync' }) }, 'Kết nối với máy tính'));
}

function render() {
  const app = $('#app');
  const keep = document.activeElement?.classList?.contains('search');
  const sel = keep ? [document.activeElement.selectionStart, document.activeElement.selectionEnd] : null;
  let body;
  if (S.view.name === 'sync') {
    body = syncView();
  } else if (S.view.name === 'series') {
    const next = S.vols.find((b) => b.read_status === 'reading') || S.vols.find((b) => b.read_status !== 'read');
    body = h('main', {},
      h('div', { class: 'series-head' }, next ? h('button', { class: 'cta', onclick: () => read(next.id) }, (next.read_status === 'reading' ? 'Đọc tiếp · ' : 'Bắt đầu · ') + `Vol ${next.volume ?? ''}`.trim()) : h('span', { class: 'dim' }, 'Đã đọc hết bộ này'),
        h('span', { class: 'dim' }, `${S.vols.length} tập · ${S.vols.filter((b) => b.read_status === 'read').length} đã đọc`), h('span', { class: 'grow' }), layoutToggle()),
      itemsView(S.vols, { showVol: true }));
  } else if (!S.items.length && !S.nwCount) {
    body = h('main', {}, empty());
  } else {
    // Đang đọc là một tab cạnh Tất cả (không còn dải bìa riêng): gọn hơn và theo đúng kiểu lưới / danh sách đang chọn.
    const tab = curTab();
    const list = tab === 'reading' ? S.reading : S.items;
    const none = h('p', { class: 'dim' }, S.q ? `Không có kết quả cho “${S.q}”.` : 'Mọi sách trên máy này đều đã bị đánh dấu không đáng đọc.');
    body = h('main', {},
      h('section', {}, h('div', { class: 'sec-head' }, S.q && tab !== 'not_worth' ? h('h2', {}, 'Kết quả') : tabBar(), h('div', { class: 'sec-tools' }, tab !== 'reading' ? sortBar() : null, layoutToggle())),
        tab === 'not_worth' ? h('p', { class: 'dim nw-hint' }, 'Không hiện trong thư viện. Nhấn giữ một cuốn → “Đáng đọc” để đưa lại.') : null,
        list.length ? itemsView(list) : none));
  }
  body.prepend(pageTitle());
  app.replaceChildren(body, toolbar());
  if (keep) { const el = $('.search'); el.focus(); el.setSelectionRange(...sel); }
}

// ------------------------------------------------------------------ đồng bộ với máy tính (I4)
const SY = S.sync;
const barEl = (a, b) => h('div', { class: 'pbar big', 'aria-hidden': 'true' }, h('i', { style: `width:${b ? Math.min(100, Math.round((a / b) * 100)) : 0}%` }));
let wake = null;
const keepAwake = async (on) => { try { if (on) wake = await navigator.wakeLock?.request('screen'); else { await wake?.release(); wake = null; } } catch { /* không bắt buộc */ } };

async function checkPc() {
  SY.busy = true; SY.check = null; render();
  try { SY.check = await call('sync_check'); } catch (e) { SY.check = { reachable: false, error: String(e?.message || e), items: [] }; }
  SY.busy = false;
  render();
}

async function startSync() {
  SY.msg = null; SY.prog = { done: 0, total: SY.check?.items.length || 0, bytes_done: 0, bytes_total: SY.check?.need_bytes || 0, title: '' };
  try { await call('sync_run'); keepAwake(true); } catch (e) { SY.prog = null; fail(e); }
  render();
}

function onSyncDone(p) {
  keepAwake(false);
  SY.prog = null; SY.check = null;
  if (p.error) SY.msg = { bad: true, text: p.error };
  else {
    const r = p.report;
    SY.msg = { bad: r.failed.length > 0, text: r.cancelled ? `Đã dừng. Tải xong ${r.added} cuốn.` : `Đã tải ${r.added} cuốn (${fmtBytes(r.bytes)}).` + (r.failed.length ? ` ${r.failed.length} cuốn lỗi: ${r.failed.map((f) => f[0]).join(', ')}.` : '') + (r.drive_connected ? '' : ' Ổ sách trên máy tính chưa cắm.') };
  }
  refresh().then(autoSync);
}

async function scanQr() {
  try {
    if (!TAURI) return await pairWith({ payload: JSON.stringify({ v: 1, addrs: ['192.168.1.20'], port: 47470, code: '482913', library_id: 'mock' }) });
    await TAURI.core.invoke('plugin:barcode-scanner|request_permissions');
    const r = await TAURI.core.invoke('plugin:barcode-scanner|scan', { formats: ['QR_CODE'], windowed: false });
    await pairWith({ payload: r.content });
  } catch (e) { fail(e); }
}

async function pairWith(args) {
  SY.busy = true; render();
  const deviceName = $('.devname')?.value.trim() || DEV.name;
  try { await call('sync_pair', { ...args, deviceName }); toast('Đã ghép nối với máy tính.'); SY.check = null; }
  catch (e) { fail(e); }
  SY.busy = false;
  refresh().then(autoSync);
}

function syncView() {
  const st = SY.st;
  if (!st?.paired) {
    const addr = h('input', { class: 'field', placeholder: 'Địa chỉ IP, ví dụ 192.168.1.20', inputmode: 'decimal', 'aria-label': 'Địa chỉ IP máy tính' });
    const code = h('input', { class: 'field', placeholder: 'Mã 6 số', inputmode: 'numeric', maxlength: 6, 'aria-label': 'Mã ghép nối' });
    // Tên hiện trên máy tính (phân biệt khi có nhiều iPad/iPhone dùng chung thư viện).
    const devName = h('input', { class: 'field devname', placeholder: `Tên máy này, ví dụ “${DEV.name} của Na”`, value: S.devName ?? DEV.name, 'aria-label': 'Tên máy này', oninput: (e) => { S.devName = e.target.value; } });
    return h('main', { class: 'syncv' },
      h('h2', {}, 'Kết nối với máy tính'),
      h('label', { class: 'dim' }, 'Tên máy này (hiện trên máy tính)'), devName,
      h('p', { class: 'dim' }, 'Trên máy tính Windows mở My Librarian → iPad / iPhone → Bật kết nối, rồi quét mã QR hiện ở đó. Hai máy cần cùng mạng Wi-Fi.'),
      h('button', { class: 'cta', disabled: SY.busy, onclick: scanQr }, SY.busy ? 'Đang kết nối…' : 'Quét mã QR'),
      h('details', {}, h('summary', {}, 'Nhập tay'), addr, code,
        h('button', { class: 'act', onclick: () => pairWith({ addr: addr.value, code: code.value }) }, 'Ghép nối')));
  }
  const c = SY.check, p = SY.prog;
  let card;
  if (p) {
    card = h('div', { class: 'card2' }, h('b', {}, `Đang tải ${Math.min(p.done + 1, p.total)}/${p.total}`), h('p', { class: 'dim' }, p.title || '…'),
      barEl(p.bytes_done, p.bytes_total), h('p', { class: 'dim' }, `${fmtBytes(p.bytes_done)} / ${fmtBytes(p.bytes_total)}`),
      h('button', { class: 'act', onclick: () => call('sync_cancel') }, 'Dừng'));
  } else if (SY.busy || !c) {
    card = h('div', { class: 'card2' }, h('b', {}, `Đang tìm ${st.pc_name}…`));
  } else if (!c.reachable) {
    card = h('div', { class: 'card2' }, h('b', {}, `Không thấy ${st.pc_name}`), h('p', { class: 'dim' }, c.error || 'Kiểm tra máy tính đang bật, mở My Librarian và cùng mạng Wi-Fi.'),
      h('button', { class: 'act', onclick: checkPc }, 'Thử lại'));
  } else {
    const n = c.items.length;
    card = h('div', { class: 'card2' }, h('b', {}, n ? `${n} cuốn đang chờ (${fmtBytes(c.need_bytes)})` : 'Không có sách mới'),
      c.drive_connected ? null : h('p', { class: 'alert' }, 'Ổ sách trên máy tính chưa cắm – chưa tải được.'),
      n && !c.enough_space ? h('p', { class: 'alert' }, `${DEV.name} không đủ chỗ: còn trống ${fmtBytes(c.free_bytes)}, cần ${fmtBytes(c.need_bytes + 512 * 1024 * 1024)}. Hãy xoá bớt sách đã đọc.`) : null,
      n ? h('ul', { class: 'queue' }, c.items.slice(0, 12).map((i) => h('li', {}, h('span', {}, i.title), h('em', {}, fmtBytes(i.size)))), c.items.length > 12 ? h('li', { class: 'dim' }, `… và ${c.items.length - 12} cuốn nữa`) : null) : null,
      n ? h('button', { class: 'cta', disabled: !c.enough_space || !c.drive_connected, onclick: startSync }, `Tải ${n} cuốn`) : h('button', { class: 'act', onclick: checkPc }, 'Kiểm tra lại'),
      !c.enough_space ? h('button', { class: 'act', onclick: storageSheet }, 'Xem dung lượng') : null);
  }
  return h('main', { class: 'syncv' },
    SY.msg ? h('p', { class: SY.msg.bad ? 'alert' : 'okmsg' }, SY.msg.text) : null, card,
    h('label', { class: 'switch' }, h('span', {}, 'Tự đồng bộ khi mở app'),
      h('input', { type: 'checkbox', checked: st.auto_sync, onchange: async (e) => { await call('sync_set_auto', { on: e.target.checked }); st.auto_sync = e.target.checked; } })),
    h('button', { class: 'act danger', onclick: async () => { if (confirm(`Bỏ ghép nối với ${st.pc_name}? Sách đã tải vẫn giữ nguyên.`)) { await call('sync_unpair'); SY.check = null; refresh(); } } }, 'Bỏ ghép nối'));
}

api.listen?.('sync_progress', (e) => { SY.prog = e.payload; if (S.view.name === 'sync') render(); });
api.listen?.('sync_done', (e) => onSyncDone(e.payload));

/** I4-05: mở app thì tự đồng bộ nếu thấy máy tính và đủ chỗ. */
async function autoSync() {
  await pushMarks();
  try {
    const st = await call('sync_state');
    if (!st.paired || !st.auto_sync || st.running) return;
    const c = await call('sync_check');
    if (c.reachable && c.items.length && c.enough_space && c.drive_connected) { SY.check = c; await startSync(); toast(`Đang tải ${c.items.length} cuốn từ ${c.pc_name}…`); }
  } catch { /* im lặng: máy tính không bật là chuyện bình thường */ }
}

addEventListener('keydown', (e) => { if (e.key === 'Escape') closeSheet(); });
// App quay lại màn hình (cùng Wi-Fi với máy tính): lấy cờ "không đáng đọc" mới đổi trên Windows, tối đa 30 giây một lần.
document.addEventListener('visibilitychange', () => { if (document.visibilityState === 'visible' && Date.now() - marksAt > 30000) pushMarks(); });

// Android: nút/cử chỉ Quay lại đóng lớp trên cùng (bảng chọn → trình đọc → trang con) thay vì thoát app.
// WebView chỉ "quay lại" khi còn lịch sử, nên giữ đúng một mục lịch sử phụ khi có thứ để đóng; ở Thư viện thì thoát app.
if (ANDROID) {
  let pushed = false, skip = false;
  const sheetOpen = () => $('#sheet-root').children.length > 0;
  const closable = () => !!$('.reader') || sheetOpen() || S.view.name !== 'library';
  const track = () => {
    if (closable() && !pushed) { history.pushState({ layer: 1 }, ''); pushed = true; }
    else if (!closable() && pushed) { pushed = false; skip = true; history.back(); }
  };
  addEventListener('popstate', () => {
    if (skip) { skip = false; return; }
    pushed = false;
    if (sheetOpen()) closeSheet();
    // Trình đọc tự xử lý Esc: đóng bảng/mục lục đang mở trước, rồi mới về thư viện.
    else if ($('.reader')) document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }));
    else if (S.view.name !== 'library') go({ name: 'library' });
    track();
  });
  new MutationObserver(track).observe(document.body, { childList: true, subtree: true });
}
refresh().then(autoSync);
