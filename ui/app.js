// 47 Librarian – giao diện (không framework). Nghiệp vụ ở Rust; file này chỉ hiển thị và gọi lệnh.
import { h, fold, highlight, fmtBytes, fmtDate, genCover, debounce } from './ui-kit.js';
import { openReader, readerOpen } from './reader.js';

const TAURI = window.__TAURI__;
const api = TAURI ? { invoke: TAURI.core.invoke, listen: TAURI.event.listen } : await import('./mock.js');
const call = (cmd, args = {}) => api.invoke(cmd, args);
const COVER_FORMATS = new Set(['epub', 'mobi', 'azw', 'azw3', 'cbz', 'cbr', 'fb2']);
const coverUrl = (b) => (TAURI && COVER_FORMATS.has(b.format) ? `http://cover.localhost/${b.id}?v=${b.mtime}` : null);
// Nội dung sách cho trình đọc (giao thức book://). Bản xem trước trên trình duyệt dùng file mẫu của serve-ui.
const bookBase = (b) => (TAURI ? `http://book.localhost/${b.id}/` : `/__book/${b.format}/`);

// ------------------------------------------------------------------ trạng thái
const store = (k, v) => { try { v === undefined ? localStorage.removeItem(k) : localStorage.setItem(k, JSON.stringify(v)); } catch { /* bỏ qua */ } };
const load = (k, d) => { try { const v = localStorage.getItem(k); return v == null ? d : JSON.parse(v); } catch { return d; } };

const S = {
  ov: null,
  view: { name: 'library', scope: 'all' },
  q: '',
  sort: load('sort', 'relevance'),
  layout: load('layout', 'grid') === 'list' ? 'list' : 'grid', // lưới bìa hay danh sách – nhớ giữa các lần mở app
  format: '',
  readStatus: '',
  sel: new Set(),
  anchor: null,
  scanning: false,
  progress: null,
  health: null,
  quitting: false,
};

const $ = (s) => document.querySelector(s);
const view = $('#view');

// ------------------------------------------------------------------ toast, modal, menu
function toast(msg, opts = {}) {
  const el = h('div', { class: 'toast' + (opts.error ? ' err' : '') }, h('span', {}, msg));
  if (opts.action) el.append(h('button', { class: 'btn link', onclick: () => { el.remove(); opts.action.run(); } }, opts.action.label));
  $('#toasts').append(el);
  setTimeout(() => el.remove(), opts.ms || (opts.action ? 9000 : 4000));
}
const fail = (e) => toast(String(e), { error: true, ms: 7000 });

function modal({ title, body, ok = 'Đồng ý', cancel = 'Huỷ', danger = false, content }) {
  return new Promise((resolve) => {
    const close = (v) => { back.remove(); document.removeEventListener('keydown', onKey, true); resolve(v); };
    const onKey = (e) => { if (e.key === 'Escape') { e.stopPropagation(); close(false); } if (e.key === 'Enter' && !content) { e.stopPropagation(); close(true); } };
    const okBtn = h('button', { class: 'btn ' + (danger ? 'danger solid' : 'primary'), onclick: () => close(true) }, ok);
    const back = h('div', { class: 'modal-back', onclick: (e) => e.target === back && close(false) },
      h('div', { class: 'modal', role: 'dialog', 'aria-modal': 'true', 'aria-label': title },
        h('h3', {}, title), body ? h('p', {}, body) : null, content || null,
        h('div', { class: 'actions' }, cancel ? h('button', { class: 'btn', onclick: () => close(false) }, cancel) : null, okBtn)));
    $('#modal-root').append(back);
    document.addEventListener('keydown', onKey, true);
    okBtn.focus();
  });
}

function showMenu(x, y, items) {
  const m = $('#menu');
  m.replaceChildren(...items.filter(Boolean).map((it) => it === '-' ? h('hr') :
    h('button', { class: it.danger ? 'danger' : '', role: 'menuitem', onclick: () => { hideMenu(); it.run(); } }, it.label)));
  m.hidden = false;
  const r = m.getBoundingClientRect();
  m.style.left = Math.min(x, innerWidth - r.width - 8) + 'px';
  m.style.top = Math.min(y, innerHeight - r.height - 8) + 'px';
}
const hideMenu = () => { $('#menu').hidden = true; };
addEventListener('click', hideMenu);
addEventListener('blur', hideMenu);

// ------------------------------------------------------------------ tổng quan
async function refreshOverview() {
  S.ov = await call('overview');
  S.devices = await call('ipad_devices').catch(() => []);
  if (S.ov.notice) modal({ title: 'Đã khôi phục dữ liệu thư viện', body: S.ov.notice, cancel: null, ok: 'Đã hiểu' });
  S.scanning = S.scanning || S.ov.scanning;
  renderNav();
  renderScanBox();
  renderBanner();
}
const stats = () => S.ov?.report?.stats || null;

function navItem(label, ico, target, count, badge, drop) {
  const v = S.view;
  const cur = v.name === target.name && (target.scope ? v.scope === target.scope : true) && (target.category ? v.category === target.category : true);
  // `drop`: thư mục thể loại nhận sách kéo thả từ lưới/danh sách.
  return h('button', { class: 'nav-item', 'aria-current': cur ? 'page' : null, 'data-drop': drop ? drop.name : null, onclick: () => go(target) },
    h('span', { class: 'ico', 'aria-hidden': 'true' }, ico), h('span', { class: 'label' }, label),
    badge ? h('span', { class: 'badge' }, badge) : count != null ? h('span', { class: 'count' }, count.toLocaleString('vi-VN')) : null);
}

function renderNav() {
  const ov = S.ov, st = stats();
  const nav = [h('div', { class: 'nav-section' }, 'Thư viện'),
    navItem('Tất cả sách', '▦', { name: 'library', scope: 'all' }, ov.facets.total - (ov.facets.not_worth || 0)),
    navItem('Đang đọc', '◔', { name: 'library', scope: 'reading' }, ov.facets.read_status.find((x) => x[0] === 'reading')?.[1] || 0),
    navItem('Bộ truyện', '❏', { name: 'library', scope: 'series' }),
    navItem(ov.config.inbox_name, '⇣', { name: 'library', scope: 'inbox' }, null, ov.facets.inbox || null),
    navItem('Không đáng đọc', '⊘', { name: 'library', scope: 'not_worth' }, ov.facets.not_worth || 0)];
  if (ov.categories.length) {
    nav.push(h('div', { class: 'nav-section' }, 'Bộ sưu tập'));
    const inboxKey = fold(ov.config.inbox_name);
    for (const c of ov.categories) if (fold(c.name) !== inboxKey) nav.push(navItem(c.name, '▤', { name: 'library', scope: 'category', category: c.name }, c.books, null, c));
  }
  nav.push(h('div', { class: 'nav-section' }, 'Dọn dẹp'),
    navItem('Trùng chắc chắn', '⧉', { name: 'dupes' }, null, st?.exact_files || null),
    navItem('Nghi trùng', '≈', { name: 'suspects' }, null, st?.suspect_groups || null),
    navItem('Phân loại', '⇢', { name: 'classify' }, null, st?.inbox_books || null),
    ov.facets.on_ipad ? navItem('Có trên iPad/iPhone', '▭', { name: 'library', scope: 'ipad' }, ov.facets.on_ipad) : null,
    ov.facets.problems ? navItem('Không đọc được', '⚠', { name: 'library', scope: 'problems' }, ov.facets.problems) : null,
    h('div', { class: 'nav-section' }, 'Khác'),
    navItem('iPad / iPhone', '▭', { name: 'ipad' }),
    navItem('Hoạt động', '↺', { name: 'activity' }),
    navItem('Cài đặt', '⚙', { name: 'settings' }));
  $('#nav').replaceChildren(...nav.filter(Boolean)); // replaceChildren biến null thành chữ "null"
}

function renderScanBox() {
  const rb = $('#refresh-btn');
  if (rb) { rb.disabled = S.scanning; rb.textContent = S.scanning ? 'Đang làm mới…' : '⟳ Làm mới'; }
  const box = $('#scan-box');
  if (!S.ov?.library_ok) { box.replaceChildren(); return; }
  if (S.quitting) {
    box.replaceChildren(h('div', { class: 'txt' }, 'Đang dừng quét để đóng app…'), h('div', { class: 'progress' }, h('i')));
    return;
  }
  if (S.scanning) {
    const p = S.progress, { text, pct } = progressText(p);
    box.replaceChildren(h('div', { class: 'txt', title: 'Phần đã xong được lưu ngay – tắt máy giữa chừng thì lần sau chỉ chạy phần còn lại.' }, text),
      h('div', { class: 'progress' }, pct == null ? h('i') : h('i', { class: 'det', style: `width:${pct}%` })),
      ...[healthLine(S.health)].filter(Boolean), // replaceChildren biến null thành chữ "null"
      h('button', { class: 'btn small', onclick: () => call('cancel_scan') }, 'Huỷ'));
    return;
  }
  const r = S.ov.report;
  box.replaceChildren(
    h('div', { class: 'txt', style: 'margin-bottom:6px' }, r ? `Quét gần nhất: ${fmtDate(r.started_at)}` : 'Chưa quét lần nào'),
    h('button', { class: 'btn small', onclick: startScan }, '⟳ Quét thư viện'));
}

// Giám sát treo từ lõi (mỗi giây): { label, idle_secs, limit_secs, stalled }. Đứng yên ≥ 5 giây thì hiện đồng hồ;
// quá giới hạn thì lõi tự dừng quét và báo lỗi.
function healthLine(hl) {
  if (!hl) return null;
  if (hl.stalled) return h('div', { class: 'txt health err' }, `Bước “${hl.label}” đứng yên quá ${hl.limit_secs} giây – đang dừng quét…`);
  if (hl.idle_secs < 5) return null;
  return h('div', { class: 'txt health' + (hl.idle_secs >= hl.limit_secs / 2 ? ' warn' : '') },
    `${hl.label}: ${hl.idle_secs} giây chưa có tiến triển (tự dừng sau ${hl.limit_secs} giây)`);
}

// Tiến trình từ lõi: { phase, done, total } (total = 0: chưa biết tổng → thanh chạy vô định).
function progressText(p) {
  const n = (x) => (x || 0).toLocaleString('vi-VN');
  const of = (unit) => p.total ? `${n(p.done)} / ${n(p.total)} ${unit}` : `${n(p.done)} ${unit}`;
  const pct = p?.total ? Math.min(100, Math.round((p.done / p.total) * 100)) : null;
  switch (p?.phase) {
    case 'triage_read': return { text: `Đang đọc hộp thư đến ${of('thư mục')}`, pct };
    case 'triage': return { text: p.total ? `Đang phân loại ${of('sách')}` : 'Đang liệt kê hộp thư đến…', pct };
    case 'triage_clean': return { text: `Đang dọn thư mục rỗng trong hộp thư đến ${of('mục')}`, pct };
    case 'hash_quick': return { text: `Đang băm nhanh ${of('file')}`, pct };
    case 'hash_full': return { text: `Đang băm đầy đủ ${of('file')}`, pct };
    case 'analyze': return { text: `Đang phân tích trùng & thể loại (${p.done || 0} / ${p.total || 4} phần)`, pct };
    case 'count': return { text: `Đang liệt kê file… ${n(p.done)} file`, pct: null };
    case 'recover': return { text: `Đang nhận lại sách đã chuyển chỗ ${of('sách')}`, pct };
    case 'scan': return { text: !p.total && !p.done ? 'Đang liệt kê file…' : `Đang quét… ${of('file')}`, pct };
    default: return { text: 'Đang quét…', pct: null };
  }
}

function renderBanner() {
  const b = $('#banner'), r = S.ov?.report, st = stats();
  const hidden = !S.ov?.banner || !st || load('bannerSeen', 0) === r.id;
  b.hidden = hidden;
  if (hidden) return;
  const acts = [];
  if (st.suspect_groups) acts.push(h('button', { class: 'btn small', onclick: () => go({ name: 'suspects' }) }, 'Duyệt nghi trùng'));
  if (st.exact_files) acts.push(h('button', { class: 'btn small', onclick: () => go({ name: 'dupes' }) }, 'Dọn trùng'));
  if (st.inbox_books) acts.push(h('button', { class: 'btn small', onclick: () => go({ name: 'classify' }) }, 'Phân loại'));
  b.replaceChildren(h('span', { class: 'msg' }, S.ov.banner), ...acts,
    h('button', { class: 'btn link', 'aria-label': 'Ẩn thông báo', onclick: () => { store('bannerSeen', r.id); renderBanner(); } }, '✕'));
}

async function startScan() {
  try {
    S.scanning = true; S.progress = null; S.health = null; renderScanBox();
    await call('start_scan');
  } catch (e) { S.scanning = false; renderScanBox(); fail(e); }
}

api.listen('scan-progress', (e) => { S.scanning = true; S.progress = e.payload; renderScanBox(); });
// Bấm đóng cửa sổ khi đang quét: hỏi lại; đồng ý thì dừng quét (lưu phần đã làm) rồi app tự thoát.
let closeAsked = false;
api.listen('close-requested', async () => {
  if (closeAsked || S.quitting) return;
  closeAsked = true;
  const ok = await modal({
    title: 'Đang quét thư viện',
    body: 'Quét chưa xong. Đóng app bây giờ sẽ dừng quét – phần đã quét được giữ lại, lần sau quét tiếp phần còn lại. Ổ chậm có thể cần vài giây để dừng hẳn.',
    ok: 'Dừng quét và đóng', cancel: 'Tiếp tục quét', danger: true,
  });
  closeAsked = false;
  if (!ok) return;
  S.quitting = true; renderScanBox();
  call('quit_app').catch(fail);
});

api.listen('scan-health', (e) => { if (!S.scanning) return; S.health = e.payload; renderScanBox(); });

api.listen('scan-done', async (e) => {
  const stalled = S.health?.stalled;
  S.scanning = false; S.progress = null; S.health = null;
  const p = e.payload;
  // Treo: hiện hộp thoại (không chỉ toast) để người dùng thấy rõ bước / file nào và chép lại được.
  if (!p.ok && stalled) modal({ title: 'Đã dừng quét vì bị treo', body: p.message, cancel: null, ok: 'Đã hiểu' });
  else if (!p.ok) fail('Quét lỗi: ' + p.message);
  else if (p.stats?.scan?.cancelled) toast('Đã huỷ quét – lần sau sẽ quét tiếp.');
  else toast(`Quét xong: ${p.stats.scan.added} sách mới, ${p.stats.scan.missing} sách mất.`);
  store('bannerSeen', 0);
  await refreshOverview();
  render();
});

// ------------------------------------------------------------------ điều hướng
function go(v) {
  S.view = v;
  S.sel.clear();
  S.anchor = null;
  if (!v.keepQuery) S.q = '';
  renderNav();
  renderBanner();
  render();
}

function render() {
  hideMenu();
  view.scrollTop = 0;
  grid = null;
  const v = S.view.name;
  ({ library: renderLibrary, series: renderLibrary, dupes: renderDupes, suspects: renderSuspects, classify: renderClassify,
    activity: renderActivity, settings: renderSettings, ipad: renderIpad })[v]();
}

function toolbar(title, sub, ...tools) {
  $('#toolbar').replaceChildren(h('h1', {}, title, sub ? h('span', { class: 'sub' }, sub) : null), h('div', { class: 'tools' }, ...tools));
}

// ------------------------------------------------------------------ lưới ảo hoá (E3-05)
let grid = null;
const PAGE = 120;

class Grid {
  constructor(host, query) {
    this.host = host; this.query = query; this.pages = new Map(); this.total = null; this.cards = new Map();
    this.wrap = h('div', { class: 'grid-wrap' + (S.layout === 'list' ? ' as-list' : ''), role: 'grid', 'aria-label': 'Sách' });
    if (S.layout === 'list') host.append(h('div', { class: 'list-head', 'aria-hidden': 'true' },
      h('span'), h('span', {}, 'Tên sách'), h('span', {}, 'Tác giả'), h('span', {}, 'Định dạng'), h('span', {}, 'Thư mục'), h('span', {}, 'Tiến độ')));
    host.append(this.wrap);
    this.onScroll = () => this.paint();
    view.addEventListener('scroll', this.onScroll, { passive: true });
    this.ro = new ResizeObserver(() => { this.layout(); this.paint(true); });
    this.ro.observe(view);
  }
  destroy() { view.removeEventListener('scroll', this.onScroll); this.ro.disconnect(); }
  async start() { await this.fetch(0); this.layout(); this.paint(true); return this.total; }
  async fetch(page) {
    if (this.pages.has(page)) return this.pages.get(page);
    const p = call('query', { q: { ...this.query, offset: page * PAGE, limit: PAGE } }).then((r) => { this.total = r.total; this.pages.set(page, r.items); return r.items; });
    this.pages.set(page, p);
    return p;
  }
  item(i) { const p = this.pages.get(Math.floor(i / PAGE)); return Array.isArray(p) ? p[i % PAGE] : undefined; }
  layout() {
    const w = this.wrap.clientWidth || view.clientWidth - 56;
    if (S.layout === 'list') {
      // Chế độ danh sách: một cột, mỗi dòng cao cố định – vẫn ảo hoá như lưới.
      this.cols = 1; this.cw = w; this.ch = 52; this.gap = 0;
    } else {
      const gap = 14, min = innerWidth < 860 ? 128 : 150;
      this.cols = Math.max(2, Math.floor((w + gap) / (min + gap)));
      this.cw = (w - gap * (this.cols - 1)) / this.cols; this.ch = this.cw * 1.5 + 76; this.gap = gap;
    }
    const gap = this.gap;
    const rows = Math.ceil((this.total || 0) / this.cols);
    this.wrap.style.height = Math.max(0, rows * (this.ch + gap)) + 'px';
  }
  paint(force) {
    if (this.total == null) return;
    const top = view.scrollTop - this.wrap.offsetTop, vh = view.clientHeight;
    const r0 = Math.max(0, Math.floor(top / (this.ch + this.gap)) - 2), r1 = Math.ceil((top + vh) / (this.ch + this.gap)) + 2;
    const i0 = r0 * this.cols, i1 = Math.min(this.total, r1 * this.cols);
    if (!force && i0 === this.i0 && i1 === this.i1) return;
    this.i0 = i0; this.i1 = i1;
    const need = new Set();
    for (let p = Math.floor(i0 / PAGE); p <= Math.floor(Math.max(i0, i1 - 1) / PAGE); p++) if (!Array.isArray(this.pages.get(p))) need.add(p);
    need.forEach((p) => this.fetch(p).then(() => this.paint(true)));
    for (const [i, el] of this.cards) if (i < i0 || i >= i1 || force) { el.remove(); this.cards.delete(i); }
    for (let i = i0; i < i1; i++) {
      if (this.cards.has(i)) continue;
      const it = this.item(i); if (!it) continue;
      const el = card(it, i);
      const row = Math.floor(i / this.cols), col = i % this.cols;
      Object.assign(el.style, { left: col * (this.cw + this.gap) + 'px', top: row * (this.ch + this.gap) + 'px', width: this.cw + 'px', height: this.ch + 'px' });
      this.wrap.append(el); this.cards.set(i, el);
    }
  }
  refreshSelection() { for (const [i, el] of this.cards) el.classList.toggle('selected', S.sel.has(key(this.item(i)))); }
  loadedItems() { const out = []; for (let i = 0; i < (this.total || 0); i++) { const it = this.item(i); if (it) out.push(it); } return out; }
}

const key = (it) => (it.kind === 'series' ? `s:${it.category ?? ''}|${it.series}` : `b:${it.id}`);

function coverFor(b, title, authors) {
  const url = coverUrl(b);
  const gen = () => genCover(title ?? b.title, authors ?? b.authors, b.format);
  if (!url) return gen();
  const img = h('img', { class: 'cover', src: url, alt: '', loading: 'lazy', decoding: 'async', draggable: 'false' });
  img.onerror = () => img.replaceWith(gen());
  return img;
}

/** Một dòng của chế độ danh sách: bìa nhỏ, tên, tác giả, định dạng, thư mục, tiến độ đọc. */
function listRow(it) {
  if (it.kind === 'series') {
    const fake = { id: it.cover_book_id, format: 'cbz', mtime: 0, title: it.series, authors: it.authors };
    return [h('div', { class: 'thumb' }, coverFor(fake, it.series, it.authors)),
      h('div', { class: 'c-title' }, h('span', { class: 't' }, highlight(it.series, S.q)), h('span', { class: 'tag' }, `${it.volumes} tập`)),
      h('div', { class: 'c-author' }, highlight(it.authors || '—', S.q)),
      h('div', { class: 'c-fmt' }, 'Bộ'),
      h('div', { class: 'c-dir' }, it.category || '—'),
      h('div', { class: 'c-prog' }, it.read ? [h('span', { class: 'bar' }, h('i', { style: `width:${Math.round((100 * it.read) / it.volumes)}%` })), h('span', {}, `${it.read}/${it.volumes}`)] : h('span', { class: 'muted' }, '—'))];
  }
  const pct = it.read_status === 'read' ? 100 : Math.round((it.progress || 0) * 100);
  const dir = it.rel_path ? it.rel_path.split(/[\\/]/).slice(0, -1).join(' › ') : it.category || '';
  return [h('div', { class: 'thumb' }, coverFor(it)),
    h('div', { class: 'c-title' }, h('span', { class: 't' }, highlight(it.title, S.q)), it.on_ipad ? h('span', { class: 'tag', title: 'Có trên iPad/iPhone' }, '▭') : null,
      it.error || it.drm ? h('span', { class: 'tag bad', title: it.error || 'File có DRM – không đọc được trong app' }, it.drm ? 'DRM' : 'Lỗi') : null),
    h('div', { class: 'c-author' }, highlight(it.authors || '—', S.q)),
    h('div', { class: 'c-fmt' }, it.format.toUpperCase()),
    h('div', { class: 'c-dir', title: dir }, dir || '—'),
    h('div', { class: 'c-prog' }, it.read_status === 'unread' ? h('span', { class: 'muted' }, fmtBytes(it.size)) :
      [h('span', { class: 'bar' }, h('i', { style: `width:${pct}%` })), h('span', {}, it.read_status === 'read' ? 'Đã đọc' : pct + '%')])];
}

function card(it, i) {
  const sel = S.sel.has(key(it));
  if (S.layout === 'list') {
    const el = h('div', { class: 'card row' + (sel ? ' selected' : ''), role: 'row', 'aria-selected': sel ? 'true' : 'false', title: it.kind === 'book' ? it.rel_path : it.series, 'data-i': i }, ...listRow(it));
    return wireCard(el, it, i);
  }
  let cover, title, author, state;
  if (it.kind === 'series') {
    const fake = { id: it.cover_book_id, format: 'cbz', mtime: 0, title: it.series, authors: it.authors };
    cover = h('div', { class: 'stack' }, coverFor(fake, it.series, it.authors), h('span', { class: 'vol-badge' }, `${it.volumes} vol`));
    title = it.series; author = it.authors || it.category || '';
    state = h('div', { class: 'state' }, it.read ? `Đã đọc ${it.read}/${it.volumes}` : `${it.volumes} tập`);
  } else {
    cover = h('div', { style: 'position:relative;width:100%;max-height:100%;display:flex;align-items:flex-end' }, coverFor(it), it.on_ipad ? h('span', { class: 'vol-badge', title: 'Có trên iPad' }, '▭ iPad') : null);
    title = it.series && S.view.name === 'series' && it.volume != null ? `${it.title}` : it.title;
    author = it.authors || '—';
    const pct = it.read_status === 'read' ? 100 : Math.round((it.progress || 0) * 100);
    state = h('div', { class: 'state' },
      it.error || it.drm ? h('span', { class: 'problem', title: it.error || 'File có DRM – không đọc được trong app' }, it.drm ? 'Có DRM' : 'Không đọc được: ' + it.error) :
      it.read_status === 'unread' ? h('span', {}, it.format.toUpperCase() + ' · ' + fmtBytes(it.size)) :
        [h('span', { class: 'bar' }, h('i', { style: `width:${pct}%` })), h('span', {}, it.read_status === 'read' ? 'Đã đọc' : pct + '%')]);
  }
  const el = h('div', { class: 'card' + (sel ? ' selected' : ''), role: 'gridcell', 'aria-selected': sel ? 'true' : 'false', title: it.kind === 'book' ? it.rel_path : it.series, 'data-i': i },
    h('div', { class: 'cover-box' }, cover),
    h('div', { class: 'meta' }, h('div', { class: 'title' }, highlight(title, S.q)), h('div', { class: 'author' }, highlight(author, S.q)), state));
  return wireCard(el, it, i);
}

function wireCard(el, it, i) {
  el.addEventListener('click', (e) => select(it, i, e));
  el.addEventListener('dblclick', () => openItem(it));
  el.addEventListener('contextmenu', (e) => { e.preventDefault(); if (!S.sel.has(key(it))) select(it, i, {}); itemMenu(e.clientX, e.clientY, it); });
  el.addEventListener('pointerdown', (e) => dragArm(e, it, i));
  return el;
}

// ------------------------------------------------------------------ kéo thả sách vào thư mục thể loại ở sidebar
// Dùng sự kiện pointer thay cho HTML5 drag & drop: WebView2 của Tauri nuốt sự kiện kéo thả khi cửa sổ nhận thả file.
let drag = null;
const DRAG_START = 6; // px – di chuyển ít hơn vẫn tính là click

function dragArm(e, it, i) {
  // Ctrl/Shift+kéo không bắt đầu kéo để Ctrl/Shift+click chọn nhiều vẫn như cũ.
  if (e.button !== 0 || e.ctrlKey || e.metaKey || e.shiftKey || !S.ov?.categories.length) return;
  drag = { it, i, x: e.clientX, y: e.clientY, on: false, target: null };
}

function dragBegin() {
  // Kéo một mục chưa chọn: chọn riêng mục đó (như Explorer); kéo mục đã chọn: kéo cả vùng chọn.
  if (!S.sel.has(key(drag.it))) { S.sel.clear(); S.sel.add(key(drag.it)); S.anchor = drag.i; grid?.refreshSelection(); }
  const items = grid ? grid.loadedItems().filter((x) => S.sel.has(key(x))) : [drag.it];
  const books = items.reduce((s, x) => s + (x.kind === 'series' ? x.volumes : 1), 0) + (S.sel.size - items.length);
  const it = drag.it;
  const cv = it.kind === 'series' ? coverFor({ id: it.cover_book_id, format: 'cbz', mtime: 0 }, it.series, it.authors) : coverFor(it);
  drag.ghost = h('div', { class: 'drag-ghost', 'aria-hidden': 'true' },
    h('div', { class: 'thumb' }, cv),
    h('div', { class: 'txt' }, h('b', {}, books > 1 ? `${books.toLocaleString('vi-VN')} quyển` : it.title), h('span', {}, 'Thả vào một thư mục ở sidebar')),
    books > 1 ? h('span', { class: 'n' }, books.toLocaleString('vi-VN')) : null);
  document.body.append(drag.ghost);
  document.body.classList.add('dragging');
  drag.on = true;
  hideMenu();
}

function dragTarget(t) {
  if (drag.target === t) return;
  drag.target?.classList.remove('drop-hover');
  drag.target = t;
  t?.classList.add('drop-hover');
  drag.ghost.querySelector('.txt span').textContent = t ? `Chuyển vào “${t.dataset.drop}”` : 'Thả vào một thư mục ở sidebar';
}

const swallowClick = (e) => { e.stopPropagation(); e.preventDefault(); };

function dragEnd(drop) {
  const d = drag;
  drag = null;
  if (!d?.on) return;
  d.ghost.remove();
  d.target?.classList.remove('drop-hover');
  document.body.classList.remove('dragging');
  clearInterval(d.scroll);
  // Chặn click trình duyệt sinh ra ngay sau khi nhả chuột (kẻo mất vùng chọn), chỉ trong lượt sự kiện này.
  if (drop) {
    addEventListener('click', swallowClick, true);
    setTimeout(() => removeEventListener('click', swallowClick, true), 0);
  }
  if (drop && d.target) moveToCategory(d.target.dataset.drop);
}

addEventListener('pointermove', (e) => {
  if (!drag) return;
  if (!drag.on) {
    if (Math.hypot(e.clientX - drag.x, e.clientY - drag.y) < DRAG_START) return;
    dragBegin();
  }
  drag.ghost.style.transform = `translate(${e.clientX + 14}px, ${e.clientY + 12}px)`;
  dragTarget(document.elementFromPoint(e.clientX, e.clientY)?.closest('.nav-item[data-drop]') || null);
  // Tự cuộn sidebar khi kéo sát mép trên/dưới (danh sách thể loại dài).
  const nav = $('#nav'), r = nav.getBoundingClientRect();
  const inX = e.clientX < r.right;
  const dir = inX && e.clientY < r.top + 28 ? -1 : inX && e.clientY > r.bottom - 28 ? 1 : 0;
  if (dir !== drag.dir) {
    clearInterval(drag.scroll);
    drag.dir = dir;
    if (dir) drag.scroll = setInterval(() => { nav.scrollTop += dir * 12; }, 30);
  }
});
addEventListener('pointerup', () => dragEnd(true));
addEventListener('pointercancel', () => dragEnd(false));
addEventListener('keydown', (e) => { if (e.key === 'Escape' && drag?.on) { e.stopPropagation(); dragEnd(false); } }, true);
addEventListener('blur', () => dragEnd(false));

/** Thả vùng chọn lên thư mục thể loại `name`: di chuyển file bằng apply_moves (kind manual – hoàn tác được ở toast và màn Hoạt động). */
async function moveToCategory(name) {
  const cat = S.ov.categories.find((c) => c.name === name);
  if (!cat) return;
  if (S.ov.config.read_only) return fail('Đang bật chế độ chỉ đọc – tắt trong Cài đặt để di chuyển sách.');
  let books;
  try { books = (await selectedBooks()).filter((b) => b.id); } catch (e) { return fail(e); }
  const k = fold(cat.name);
  const todo = books.filter((b) => fold(b.category || '') !== k);
  const already = books.length - todo.length;
  if (!todo.length) return toast(books.length === 1 ? `“${books[0].title}” đã ở trong “${cat.name}”.` : `${books.length} quyển đã ở trong “${cat.name}”.`);
  // Bộ truyện: thư mục đích đã có thư mục con cùng tên bộ thì cho vào đó, không thì vào thẳng thư mục thể loại.
  const dirFor = (b) => {
    const s = b.series && cat.series_dirs.find((x) => fold(x) === fold(b.series));
    return s ? cat.path + '\\' + s : cat.path;
  };
  try {
    const r = await call('apply_moves', { moves: todo.map((b) => ({ book_id: b.id, target_dir: dirFor(b) })), kind: 'manual' });
    if (r.failures.length) fail(`${r.failures.length} quyển không di chuyển được: ${r.failures[0].message}`);
    if (r.moved.length) toast(`Đã chuyển ${r.moved.length} quyển vào “${cat.name}”` + (already ? ` · ${already} quyển đã ở đó sẵn` : '') + '.',
      { action: { label: 'Hoàn tác', run: () => undo(r.batch_id) } });
  } catch (e) { fail(e); }
  S.sel.clear();
  await refreshOverview().catch(fail);
  await refreshKeepScroll();
}

/** Vẽ lại màn thư viện sau khi sách đổi chỗ / đọc xong mà không nhảy lên đầu trang. */
async function refreshKeepScroll() {
  const top = view.scrollTop, name = S.view.name;
  if (name === 'library' || name === 'series') {
    await renderLibrary();
    view.scrollTop = top;
    grid?.paint(true);
  } else render();
}

function select(it, i, e) {
  const k = key(it);
  if (e.shiftKey && S.anchor != null && grid) {
    const [a, b] = [Math.min(S.anchor, i), Math.max(S.anchor, i)];
    if (!e.ctrlKey) S.sel.clear();
    for (let j = a; j <= b; j++) { const x = grid.item(j); if (x) S.sel.add(key(x)); }
  } else if (e.ctrlKey || e.metaKey) {
    S.sel.has(k) ? S.sel.delete(k) : S.sel.add(k); S.anchor = i;
  } else { S.sel.clear(); S.sel.add(k); S.anchor = i; }
  grid?.refreshSelection();
}

async function openItem(it) {
  if (it.kind === 'series') return go({ name: 'series', category: it.category, series: it.series, notWorth: inNotWorth() });
  return read(it.id);
}

// ------------------------------------------------------------------ trình đọc (M5)
const readerCtx = {
  call, toast, fail, modal, bookBase,
  onClose: () => refreshAfterReading(),
  onDelete: async (b) => {
    const ok = await modal({ title: `Đưa “${b.title}” vào Thùng rác?`, body: `${fmtBytes(b.size)}. Có thể khôi phục bằng Restore trong Thùng rác Windows.`, ok: 'Xoá', danger: true });
    if (!ok) return false;
    try {
      const r = await call('trash_books', { ids: [b.id] });
      reportTrash(r);
      return r.trashed > 0;
    } catch (e) { fail(e); return false; }
  },
};

const read = (id) => openReader(id, readerCtx);

/** Đóng trình đọc: cập nhật tiến độ trên lưới nhưng giữ nguyên chỗ đang cuộn. */
async function refreshAfterReading() {
  await refreshOverview().catch(fail);
  await refreshKeepScroll();
}

/** Mở file được bấm đúp trong Explorer (liên kết file – E10-01). File ngoài thư viện vẫn đọc được. */
async function openPath(path) {
  try { read(await call('open_path', { path })); } catch (e) { fail(e); }
}
api.listen('open-file', (e) => openPath(e.payload));

async function openExternal(id) {
  try { await call('open_external', { id }); } catch (e) { fail(e); }
}

function itemMenu(x, y, it) {
  const n = S.sel.size;
  if (it.kind === 'series') {
    return showMenu(x, y, [
      { label: 'Mở bộ', run: () => openItem(it) },
      it.continue_book_id ? { label: 'Đọc tiếp', run: () => read(it.continue_book_id) } : null,
      ...sendItems(),
      notWorthItem(n, true),
      '-', { label: n > 1 ? `Xoá ${n} mục vào Thùng rác…` : 'Xoá cả bộ vào Thùng rác…', danger: true, run: deleteSelection },
    ].filter(Boolean));
  }
  showMenu(x, y, [
    { label: 'Đọc', run: () => openItem(it) },
    { label: 'Mở bằng ứng dụng mặc định', run: () => openExternal(it.id) },
    { label: 'Hiện trong thư mục', run: () => call('reveal', { id: it.id }).catch(fail) },
    { label: 'Sao chép đường dẫn', run: () => navigator.clipboard.writeText(it.path).then(() => toast('Đã sao chép đường dẫn')) },
    '-',
    ...sendItems(n),
    it.on_ipad ? { label: 'Có trên thiết bị nào?', run: () => showBookDevices(it) } : null,
    { label: 'Đánh dấu đã đọc', run: () => markRead('read') },
    { label: 'Đánh dấu chưa đọc', run: () => markRead('unread') },
    notWorthItem(n, false),
    { label: n > 1 ? `Chuyển ${n} sách vào thư mục…` : 'Chuyển vào thư mục…', run: moveSelection },
    '-', { label: n > 1 ? `Xoá ${n} sách vào Thùng rác…` : 'Xoá vào Thùng rác…', danger: true, run: deleteSelection },
  ].filter(Boolean));
}

/** Mục menu "Gửi sang <thiết bị>": một mục cho mỗi iPad/iPhone đã ghép, thêm "tất cả" khi có từ hai máy. */
function sendItems(n = 1) {
  const devs = S.devices || [];
  const what = n > 1 ? `Gửi ${n} sách sang` : 'Gửi sang';
  if (!devs.length) return [{ label: `${what} iPad / iPhone…`, run: () => toast('Chưa ghép thiết bị nào – mở mục iPad / iPhone để ghép trước.') }];
  const items = devs.map((d) => ({ label: `${what} ${d.name}`, run: () => sendTo([d.id]) }));
  if (devs.length > 1) items.push({ label: `${what} tất cả ${devs.length} thiết bị`, run: () => sendTo(devs.map((d) => d.id)) });
  return items;
}

async function showBookDevices(it) {
  try {
    const names = await call('book_devices', { id: it.id });
    toast(names.length ? `“${it.title}” có trên: ${names.join(', ')}` : `“${it.title}” chưa có trên thiết bị nào`);
  } catch (e) { fail(e); }
}

/// Đổi mục đã chọn (kể cả ô bộ truyện) thành danh sách sách cụ thể.
async function selectedBooks() {
  const out = [];
  for (const k of S.sel) {
    if (k.startsWith('b:')) {
      const it = grid?.loadedItems().find((x) => key(x) === k);
      if (it) out.push(it); else { const b = await call('book', { id: +k.slice(2) }); if (b) out.push(b); }
    } else {
      const [cat, series] = k.slice(2).split('|');
      const r = await call('query', { q: { series, category: cat || null, not_worth: inNotWorth(), limit: 100000 } });
      out.push(...r.items);
    }
  }
  return out;
}

/// Thêm mục đã chọn vào hàng chờ của các thiết bị `devices` (I3-03). Mỗi thiết bị một hàng chờ riêng.
/// CBR/DJVU bị bỏ qua; iPhone chỉ nhận sách chữ (PDF/truyện tranh bị bỏ qua cho iPhone).
async function sendTo(devices) {
  try {
    const ids = (await selectedBooks()).map((b) => b.id);
    if (!ids.length) return;
    const r = await call('ipad_queue_add', { ids, devices });
    const names = (S.devices || []).filter((d) => devices.includes(d.id)).map((d) => d.name).join(', ');
    const skip = [];
    if (r.skipped_unsupported) skip.push(`${r.skipped_unsupported} cuốn CBR/DJVU không đọc được`);
    if (r.skipped_device) skip.push(`${r.skipped_device} lượt PDF/truyện tranh không gửi sang iPhone`);
    if (r.skipped_missing) skip.push(`${r.skipped_missing} cuốn không còn file`);
    toast((r.books ? `Đã xếp ${r.books} sách vào hàng chờ của ${names}` : 'Không thêm được sách nào') + (skip.length ? ` · bỏ qua ${skip.join(', ')}` : ''));
  } catch (e) { fail(e); }
}

/** Đang xem mục "Không đáng đọc" (hoặc một bộ mở từ mục đó). */
const inNotWorth = () => S.view.scope === 'not_worth' || !!S.view.notWorth;

/** Mục menu đánh dấu "không đáng đọc" / đưa lại vào thư viện, theo mục đang xem. */
function notWorthItem(n, series) {
  if (inNotWorth()) return { label: n > 1 ? `Đưa ${n} mục lại vào thư viện` : series ? 'Đưa cả bộ lại vào thư viện' : 'Đáng đọc – đưa lại vào thư viện', run: () => markNotWorth(false) };
  return { label: n > 1 ? `Đánh dấu ${n} mục không đáng đọc` : series ? 'Đánh dấu cả bộ không đáng đọc' : 'Đánh dấu không đáng đọc', run: () => markNotWorth(true) };
}

/** Ẩn sách khỏi thư viện (chuyển sang mục "Không đáng đọc") hoặc đưa lại. Thiết bị di động nhận theo khi đồng bộ. */
async function markNotWorth(on, ids = null) {
  try {
    ids ??= (await selectedBooks()).map((b) => b.id);
    if (!ids.length) return;
    await call('set_not_worth', { ids, on });
    S.sel.clear();
    await refreshOverview();
    await refreshKeepScroll();
    const what = ids.length > 1 ? `${ids.length} sách` : 'sách';
    toast(on ? `Đã chuyển ${what} sang “Không đáng đọc”.` : `Đã đưa ${what} lại vào thư viện.`, { action: { label: 'Hoàn tác', run: () => markNotWorth(!on, ids) } });
  } catch (e) { fail(e); }
}

async function markRead(status) {
  const ids = (await selectedBooks()).map((b) => b.id);
  await call('set_read_status', { ids, status }).catch(fail);
  render();
}

async function deleteSelection() {
  const books = await selectedBooks();
  if (!books.length) return;
  const total = books.reduce((s, b) => s + b.size, 0);
  const ok = await modal({ title: `Đưa ${books.length} sách vào Thùng rác?`, body: `Tổng ${fmtBytes(total)}. Có thể khôi phục bằng Restore trong Thùng rác Windows.`, ok: 'Xoá', danger: true });
  if (!ok) return;
  try { reportTrash(await call('trash_books', { ids: books.map((b) => b.id) })); } catch (e) { fail(e); }
  S.sel.clear();
  await refreshOverview();
  render();
}

function reportTrash(r) {
  if (r.aborted) fail(r.aborted);
  if (r.failures.length) modal({ title: `${r.failures.length} file không xoá được`, cancel: null, ok: 'Đóng', content: h('div', { style: 'max-height:240px;overflow:auto' }, ...r.failures.map((f) => h('div', { class: 'path', style: 'margin-bottom:6px' }, f.path, h('br'), h('span', { class: 'muted' }, f.message)))) });
  if (r.trashed) toast(`Đã đưa ${r.trashed} file vào Thùng rác (${fmtBytes(r.freed_bytes)}).`, { action: { label: 'Mở Thùng rác', run: () => call('open_recycle_bin') } });
}

async function moveSelection() {
  const books = (await selectedBooks()).filter((b) => b.id);
  const { targets } = await call('propose');
  const sel = h('select', { class: 'pill', style: 'width:100%' }, ...targets.map((t) => h('option', { value: t.path }, t.label)));
  const ok = await modal({ title: `Chuyển ${books.length} sách vào…`, ok: 'Chuyển', content: h('div', { style: 'margin-bottom:16px' }, sel) });
  if (!ok) return;
  try {
    const r = await call('apply_moves', { moves: books.map((b) => ({ book_id: b.id, target_dir: sel.value })), kind: 'manual' });
    reportMoves(r);
  } catch (e) { fail(e); }
  await refreshOverview();
  render();
}

function reportMoves(r) {
  if (r.failures.length) fail(`${r.failures.length} file không di chuyển được: ${r.failures[0].message}`);
  if (r.moved.length) toast(`Đã di chuyển ${r.moved.length} file.`, { action: { label: 'Hoàn tác', run: () => undo(r.batch_id) } });
}

async function undo(id) {
  try {
    const u = await call('undo_batch', { id });
    toast(`Đã trả ${u.restored} file về chỗ cũ` + (u.failures.length ? `, ${u.failures.length} file không hoàn tác được` : '.'), { error: !!u.failures.length });
  } catch (e) { fail(e); }
  await refreshOverview();
  render();
}

// ------------------------------------------------------------------ màn thư viện
function libraryQuery() {
  const v = S.view, base = { q: S.q || null, sort: S.sort, format: S.format || null, read_status: S.readStatus || null };
  if (v.name === 'series') return { ...base, series: v.series, category: v.category ?? '', sort: 'relevance', not_worth: !!v.notWorth };
  switch (v.scope) {
    case 'reading': return { ...base, read_status: 'reading', sort: S.q ? S.sort : 'recent_read' };
    case 'series': return { ...base, group_series: true, only_series: true };
    case 'inbox': return { ...base, category: S.ov.config.inbox_name };
    case 'problems': return { ...base, problems: true };
    case 'ipad': return { ...base, on_ipad: true };
    case 'not_worth': return { ...base, not_worth: true, group_series: true };
    case 'category': return { ...base, category: v.category, group_series: true };
    default: return { ...base, group_series: true };
  }
}

function libraryTitle() {
  const v = S.view;
  if (v.name === 'series') return v.series;
  return { all: 'Thư viện', reading: 'Đang đọc', series: 'Bộ truyện', inbox: S.ov.config.inbox_name, category: v.category, problems: 'Không đọc được', ipad: 'Có trên iPad', not_worth: 'Không đáng đọc' }[v.scope];
}

async function renderLibrary() {
  if (!S.ov.library_ok) return renderWelcome();
  const search = h('input', { type: 'search', placeholder: 'Tìm tên sách, tác giả, tên file…', value: S.q, 'aria-label': 'Tìm kiếm' });
  search.addEventListener('input', debounce(() => { S.q = search.value.trim(); reloadGrid(); }, 160));
  const sortSel = h('select', { class: 'pill', 'aria-label': 'Sắp xếp', onchange: (e) => { S.sort = e.target.value; store('sort', S.sort); reloadGrid(); } },
    ...[['relevance', 'Phù hợp / mới thêm'], ['added', 'Mới thêm'], ['title', 'Tên'], ['author', 'Tác giả'], ['recent_read', 'Đọc gần đây'], ['size', 'Dung lượng']]
      .map(([v, l]) => h('option', { value: v, selected: S.sort === v }, l)));
  const fmtSel = h('select', { class: 'pill', 'aria-label': 'Định dạng', onchange: (e) => { S.format = e.target.value; reloadGrid(); } },
    h('option', { value: '' }, 'Mọi định dạng'), ...S.ov.facets.formats.map(([f, n]) => h('option', { value: f, selected: S.format === f }, `${f.toUpperCase()} (${n})`)));
  const rsSel = h('select', { class: 'pill', 'aria-label': 'Trạng thái đọc', onchange: (e) => { S.readStatus = e.target.value; reloadGrid(); } },
    ...[['', 'Mọi trạng thái'], ['unread', 'Chưa đọc'], ['reading', 'Đang đọc'], ['read', 'Đã đọc']].map(([v, l]) => h('option', { value: v, selected: S.readStatus === v }, l)));
  const tools = [h('div', { class: 'search' }, search), sortSel, fmtSel];
  if (S.view.scope !== 'reading') tools.push(rsSel);
  tools.push(layoutToggle(), h('button', { class: 'btn', id: 'refresh-btn', title: 'Quét lại thư viện, chỉ đọc file mới hoặc đã đổi (F5)', disabled: S.scanning, onclick: startScan }, S.scanning ? 'Đang làm mới…' : '⟳ Làm mới'));
  toolbar(libraryTitle(), '', ...tools);
  const host = h('div', { class: 'pad' });
  if (S.view.name === 'series') {
    host.append(h('div', { style: 'display:flex;gap:8px;margin:2px 0 14px' },
      h('button', { class: 'btn small', onclick: () => go({ name: 'library', scope: S.view.notWorth ? 'not_worth' : 'series' }) }, S.view.notWorth ? '‹ Không đáng đọc' : '‹ Bộ truyện'),
      h('span', { class: 'muted small', style: 'align-self:center' }, S.view.category || '')));
  }
  view.replaceChildren(host);
  await reloadGrid(host);
  if (S.view.name === 'series' && grid) {
    // "Đọc tiếp": vol đang đọc dở, hoặc vol ngay sau vol đã đọc cuối cùng.
    const vols = (await Promise.all([...Array(Math.ceil((grid.total || 0) / PAGE)).keys()].map((p) => grid.fetch(p)))).flat();
    const last = vols.map((b) => b.read_status).lastIndexOf('read');
    const next = vols.find((b) => b.read_status === 'reading') || (last >= 0 ? vols[last + 1] : vols[0]);
    if (next) host.firstChild.append(h('button', { class: 'btn small primary', onclick: () => openItem(next) }, last >= 0 || next.read_status === 'reading' ? `Đọc tiếp: ${next.title}` : `Bắt đầu đọc: ${next.title}`));
  }
}

/** Nút bật tắt lưới bìa / danh sách. Lựa chọn lưu trong localStorage nên giữ nguyên lần mở app sau. */
function layoutToggle() {
  const opt = (mode, ico, label) => h('button', { class: S.layout === mode ? 'on' : '', 'aria-pressed': S.layout === mode ? 'true' : 'false', title: label + ' (G)', 'aria-label': label,
    onclick: () => setLayout(mode) }, ico);
  return h('div', { class: 'seg-toggle', role: 'group', 'aria-label': 'Kiểu hiển thị' }, opt('grid', '▦', 'Lưới bìa sách'), opt('list', '☰', 'Danh sách'));
}

async function setLayout(mode) {
  if (S.layout === mode || !grid) return;
  S.layout = mode;
  store('layout', mode);
  $('#toolbar .seg-toggle')?.replaceWith(layoutToggle());
  const sel = new Set(S.sel), anchor = S.anchor;
  await reloadGrid();
  sel.forEach((k) => S.sel.add(k)); S.anchor = anchor;
  grid?.refreshSelection();
}

async function reloadGrid(host) {
  host = host || view.querySelector('.pad');
  if (!host) return;
  grid?.destroy();
  host.querySelector('.grid-wrap')?.remove();
  host.querySelector('.list-head')?.remove();
  host.querySelector('.empty')?.remove();
  S.sel.clear();
  const g = new Grid(host, libraryQuery());
  grid = g;
  try {
    const total = await g.start();
    $('#toolbar .sub')?.remove();
    $('#toolbar h1').append(h('span', { class: 'sub' }, `${total.toLocaleString('vi-VN')} mục`));
    if (!total) host.append(h('div', { class: 'empty' }, h('div', {}, h('h2', {}, S.q ? 'Không tìm thấy' : 'Chưa có sách'),
      h('p', {}, S.q ? `Không có sách nào khớp “${S.q}” trong mục này.` : S.view.scope === 'not_worth' ? 'Chuột phải một cuốn trong thư viện → “Đánh dấu không đáng đọc” để ẩn nó khỏi thư viện và đưa vào đây.' : 'Mục này đang trống.'),
      S.q && !(S.view.name === 'library' && S.view.scope === 'all') ? h('button', { class: 'btn', onclick: () => go({ name: 'library', scope: 'all', keepQuery: true }) }, 'Tìm trong toàn thư viện') : null)));
  } catch (e) { fail(e); }
}

function renderWelcome() {
  toolbar('Chào mừng', '');
  view.replaceChildren(h('div', { class: 'empty' }, h('div', {},
    h('h2', {}, 'Chọn thư mục thư viện'),
    h('p', {}, 'Chọn thư mục “books” trên ổ của bạn. Mỗi thư mục con cấp 1 là một thể loại; sách mới đặt trong “Chưa phân loại”. App chỉ đọc cho tới khi bạn bấm xoá hoặc áp dụng phân loại.'),
    h('button', { class: 'btn primary', onclick: chooseLibrary }, 'Chọn thư mục…'))));
}

async function chooseLibrary() {
  try {
    const p = await call('pick_folder');
    if (!p) return;
    await call('set_library', { path: p });
    await refreshOverview();
    render();
    startScan();
  } catch (e) { fail(e); }
}

// ------------------------------------------------------------------ trùng chắc chắn (E4-03)
async function renderDupes() {
  toolbar('Trùng chắc chắn', 'giống nhau từng byte (SHA-256)');
  view.replaceChildren(h('div', { class: 'pad muted' }, 'Đang tải…'));
  let groups;
  try { groups = await call('exact_groups'); } catch (e) { return fail(e); }
  if ((stats()?.exact_files || 0) !== groups.reduce((n, g) => n + g.others.length, 0)) refreshOverview().catch(fail);
  const keep = new Map(), include = new Set(groups.map((g) => g.full_hash));
  if (!groups.length) return view.replaceChildren(h('div', { class: 'empty' }, h('div', {}, h('h2', {}, 'Không có bản trùng'), h('p', {}, 'Chạy “Quét thư viện” để cập nhật.'))));
  const bar = h('div', { class: 'sticky-bar' });
  const list = h('div', { class: 'pad' });
  const paintBar = () => {
    const sel = groups.filter((g) => include.has(g.full_hash));
    const files = sel.reduce((s, g) => s + g.others.length, 0), bytes = sel.reduce((s, g) => s + g.reclaimable, 0);
    bar.replaceChildren(h('span', { class: 'grow' }, `${files} bản thừa trong ${sel.length} nhóm · thu hồi ${fmtBytes(bytes)}`),
      h('button', { class: 'btn danger solid', disabled: !files, onclick: () => clean(sel) }, `Dọn ${files} bản vào Thùng rác`));
  };
  const clean = async (sel) => {
    const files = sel.reduce((s, g) => s + g.others.length, 0);
    if (!(await modal({ title: `Dọn ${files} bản trùng?`, body: 'Bản giữ lại được kiểm tra còn nguyên trước khi xoá. Các bản thừa vào Thùng rác Windows.', ok: 'Dọn', danger: true }))) return;
    try {
      reportTrash(await call('clean_exact', { hashes: sel.map((g) => g.full_hash), keep: Object.fromEntries([...keep].filter(([h]) => sel.some((g) => g.full_hash === h))) }));
    } catch (e) { fail(e); }
    await refreshOverview();
    render();
  };
  for (const g of groups) {
    const members = [g.keeper, ...g.others];
    const body = h('div');
    const paint = () => {
      const kid = keep.get(g.full_hash) ?? g.keeper.id;
      body.replaceChildren(...members.map((b) => {
        const isKeep = b.id === kid;
        return h('label', { class: 'member ' + (isKeep ? 'keep' : 'drop') },
          h('input', { type: 'radio', name: 'k' + g.full_hash, checked: isKeep, onchange: () => { keep.set(g.full_hash, b.id); paint(); } }),
          h('div', { class: 'thumb' }, coverFor(b)),
          h('div', { class: 'info' }, h('div', { class: 'name' }, b.file_name), h('div', { class: 'path' }, b.rel_path)),
          isKeep ? h('span', { class: 'tag ok' }, b.id === g.keeper.id ? 'Giữ · ' + g.keep_reason : 'Giữ (bạn chọn)') : h('span', { class: 'tag bad' }, 'Bỏ'));
      }));
    };
    paint();
    const cb = h('input', { type: 'checkbox', checked: true, 'aria-label': 'Chọn nhóm', onchange: (e) => { e.target.checked ? include.add(g.full_hash) : include.delete(g.full_hash); paintBar(); } });
    list.append(h('div', { class: 'group' },
      h('div', { class: 'group-head' }, cb, h('div', { class: 'grow' }, h('b', {}, g.keeper.title), h('span', { class: 'muted small' }, `  ·  ${members.length} bản · ${fmtBytes(g.size)} mỗi bản`)),
        h('button', { class: 'btn small', onclick: () => clean([g]) }, 'Dọn nhóm này')), body));
  }
  paintBar();
  view.replaceChildren(list, bar);
}

// ------------------------------------------------------------------ nghi trùng (E4-06, E4-07)
let suspectKeys = null;
async function renderSuspects() {
  toolbar('Nghi trùng', 'cùng tên/ISBN, nội dung khác – cần bạn duyệt');
  view.replaceChildren(h('div', { class: 'pad muted' }, 'Đang tải…'));
  let groups;
  try { groups = await call('suspect_groups'); } catch (e) { return fail(e); }
  if ((stats()?.suspect_groups || 0) !== groups.length) refreshOverview().catch(fail);
  let i = 0;
  const host = h('div', { class: 'pad' });
  view.replaceChildren(host);
  const decide = async (decision) => {
    const g = groups[i];
    try { await call('decide', { ids: g.members.map((b) => b.id), decision }); } catch (e) { return fail(e); }
    groups.splice(i, 1); i = Math.min(i, groups.length - 1); paint(); refreshOverview().catch(fail);
    toast(decision === 'keep_both' ? 'Đã ghi “Giữ tất cả”.' : 'Đã ghi “Không trùng”.');
  };
  const keepOnly = async (b) => {
    const g = groups[i], drop = g.members.filter((x) => x.id !== b.id);
    if (!(await modal({ title: `Giữ “${b.file_name}”?`, body: `${drop.length} bản còn lại (${fmtBytes(drop.reduce((s, x) => s + x.size, 0))}) sẽ vào Thùng rác.`, ok: 'Giữ bản này', danger: true }))) return;
    try { reportTrash(await call('trash_books', { ids: drop.map((x) => x.id) })); } catch (e) { return fail(e); }
    groups.splice(i, 1); i = Math.min(i, groups.length - 1); paint(); refreshOverview();
  };
  const paint = () => {
    if (!groups.length) {
      host.replaceChildren(h('div', { class: 'empty' }, h('div', {}, h('h2', {}, 'Đã duyệt hết'), h('p', {}, 'Không còn nhóm nghi trùng nào.'))));
      refreshOverview();
      return;
    }
    const g = groups[i];
    host.replaceChildren(
      h('div', { class: 'section-title' }, `Nhóm ${i + 1}/${groups.length}`, h('span', { class: 'tag warn' }, g.reason)),
      h('div', { class: 'compare' }, ...g.members.map((b, k) => h('div', { class: 'cmp-card' },
        h('div', { class: 'cv' }, coverFor(b)),
        h('div', {}, h('b', {}, b.title), h('div', { class: 'muted small' }, b.authors || '—')),
        h('dl', {}, ...[['Định dạng', b.format.toUpperCase()], ['Dung lượng', fmtBytes(b.size)], ['Số trang', b.page_count ?? '—'], ['ISBN', b.isbn || '—'],
          ['Thư mục', b.rel_path], ['Sửa lúc', fmtDate(Math.floor(b.mtime / 1000))]].flatMap(([a, v]) => [h('dt', {}, a), h('dd', {}, String(v))])),
        h('div', { style: 'display:flex;gap:6px;flex-wrap:wrap' },
          h('button', { class: 'btn small primary', onclick: () => keepOnly(b) }, `Giữ bản này`, k < 9 ? h('span', { class: 'kbd', style: 'margin-left:6px;color:#fff;border-color:#fff8' }, String(k + 1)) : null),
          h('button', { class: 'btn small', title: 'Mở trong trình đọc để so sánh', onclick: () => read(b.id) }, 'Mở thử'))))),
      h('div', { style: 'display:flex;gap:8px;margin-top:18px;flex-wrap:wrap;align-items:center' },
        h('button', { class: 'btn', disabled: i === 0, onclick: () => { i--; paint(); } }, '← Trước'),
        h('button', { class: 'btn', disabled: i >= groups.length - 1, onclick: () => { i++; paint(); } }, 'Sau →'),
        h('span', { style: 'flex:1' }),
        h('button', { class: 'btn', onclick: () => decide('not_duplicate') }, 'Không trùng ', h('span', { class: 'kbd' }, 'N')),
        h('button', { class: 'btn', onclick: () => decide('keep_both') }, 'Giữ tất cả ', h('span', { class: 'kbd' }, 'K'))));
  };
  paint();
  suspectKeys = (e) => {
    if (S.view.name !== 'suspects' || !groups.length || $('#modal-root').children.length || e.target.tagName === 'INPUT') return;
    if (e.key === 'ArrowRight' && i < groups.length - 1) { i++; paint(); }
    else if (e.key === 'ArrowLeft' && i > 0) { i--; paint(); }
    else if (e.key.toLowerCase() === 'n') decide('not_duplicate');
    else if (e.key.toLowerCase() === 'k') decide('keep_both');
    else if (/^[1-9]$/.test(e.key) && groups[i].members[+e.key - 1]) keepOnly(groups[i].members[+e.key - 1]);
  };
}

// ------------------------------------------------------------------ phân loại (E6-05)
async function renderClassify() {
  toolbar('Phân loại', `sách trong “${S.ov.config.inbox_name}”`);
  view.replaceChildren(h('div', { class: 'pad muted' }, 'Đang tính đề xuất…'));
  let data;
  try { data = await call('propose'); } catch (e) { return fail(e); }
  const rows = data.proposals.map((p) => ({ p, checked: p.preselected, target: p.target || '' }));
  let tab = 'all';
  const host = h('div', { class: 'pad' }), bar = h('div', { class: 'sticky-bar' });
  const labels = { High: ['Tin cậy cao', 'ok'], Medium: ['Trung bình', 'warn'], Manual: ['Cần làm tay', 'bad'] };
  const paintBar = () => {
    const n = rows.filter((r) => r.checked && r.target).length;
    bar.replaceChildren(h('span', { class: 'grow' }, 'Không file nào được di chuyển trước khi bạn bấm Áp dụng. Áp dụng xong vẫn hoàn tác được.'),
      h('button', { class: 'btn primary', disabled: !n, onclick: apply }, `Áp dụng ${n} di chuyển`));
  };
  const apply = async () => {
    const moves = rows.filter((r) => r.checked && r.target).map((r) => ({ book_id: r.p.book.id, target_dir: r.target }));
    if (!(await modal({ title: `Di chuyển ${moves.length} sách?`, body: 'Di chuyển trong cùng ổ (đổi tên tức thì). Trùng tên sẽ thêm (1), không ghi đè.', ok: 'Áp dụng' }))) return;
    try { reportMoves(await call('apply_moves', { moves, kind: 'classify' })); } catch (e) { fail(e); }
    await refreshOverview();
    render();
  };
  const paint = () => {
    const shown = rows.filter((r) => tab === 'all' || r.p.bucket === tab);
    const tabs = h('div', { class: 'tabs', role: 'tablist' }, ...[['all', 'Tất cả'], ['High', 'Tin cậy cao'], ['Medium', 'Trung bình'], ['Manual', 'Cần làm tay']].map(([k, l]) =>
      h('button', { role: 'tab', 'aria-selected': tab === k ? 'true' : 'false', onclick: () => { tab = k; paint(); } }, `${l} (${k === 'all' ? rows.length : rows.filter((r) => r.p.bucket === k).length})`)));
    const table = h('table', { class: 'list' }, h('thead', {}, h('tr', {}, h('th', {}, ''), h('th', {}, 'Sách'), h('th', {}, 'Thư mục đích'), h('th', {}, 'Tin cậy'), h('th', {}, 'Lý do'))),
      h('tbody', {}, ...shown.map((r) => {
        const sel = h('select', { 'aria-label': 'Thư mục đích', onchange: (e) => { r.target = e.target.value; r.checked = !!r.target; paint(); } },
          h('option', { value: '' }, '— để nguyên —'), ...data.targets.map((t) => h('option', { value: t.path, selected: t.path === r.target }, t.label)));
        const [lab, cls] = labels[r.p.bucket];
        return h('tr', {},
          h('td', {}, h('input', { type: 'checkbox', checked: r.checked && !!r.target, disabled: !r.target, 'aria-label': 'Chọn', onchange: (e) => { r.checked = e.target.checked; paintBar(); } })),
          h('td', {}, h('div', { style: 'display:flex;gap:10px;align-items:center' }, h('div', { style: 'width:30px;flex:none' }, coverFor(r.p.book)),
            h('div', {}, h('div', {}, r.p.book.file_name), h('div', { class: 'muted small' }, r.p.book.authors || '')))),
          h('td', {}, sel),
          h('td', {}, h('span', { class: 'tag ' + cls }, r.p.bucket === 'Manual' ? lab : `${r.p.confidence}%`)),
          h('td', { class: 'small' }, r.p.reason, r.p.suggestion ? h('div', { class: 'muted' }, 'Gợi ý: ' + r.p.suggestion) : null));
      })));
    host.replaceChildren(tabs, h('div', { style: 'height:12px' }), rows.length ? table : h('div', { class: 'empty' }, h('div', {}, h('h2', {}, 'Hộp thư đến trống'), h('p', {}, `Chép sách mới vào “${S.ov.config.inbox_name}” rồi quét lại.`))));
    paintBar();
  };
  paint();
  view.replaceChildren(host, bar);
}

// ------------------------------------------------------------------ hoạt động (E5-05, E9-04, E8-04)
async function renderActivity() {
  toolbar('Hoạt động', 'nhật ký xoá / di chuyển / hoàn tác', h('button', { class: 'btn', onclick: () => call('open_recycle_bin').catch(fail) }, 'Mở Thùng rác'));
  let batches, reports;
  try { [batches, reports] = await Promise.all([call('list_batches'), call('report_history')]); } catch (e) { return fail(e); }
  const kinds = { dedupe: 'Dọn trùng', trash: 'Xoá', classify: 'Phân loại', manual: 'Chuyển tay' };
  const host = h('div', { class: 'pad' });
  if (!batches.length) host.append(h('p', { class: 'muted' }, 'Chưa có thao tác nào.'));
  for (const b of batches) {
    const ops = h('div');
    let open = false;
    const canUndo = (b.kind === 'classify' || b.kind === 'manual') && !b.undone_at && b.done;
    host.append(h('div', { class: 'group' },
      h('div', { class: 'group-head' },
        h('button', { class: 'btn small', 'aria-expanded': 'false', onclick: async (e) => {
          open = !open; e.currentTarget.setAttribute('aria-expanded', String(open)); e.currentTarget.textContent = open ? '▾' : '▸';
          if (!open) return ops.replaceChildren();
          const list = await call('batch_ops', { id: b.id });
          ops.replaceChildren(...list.map((o) => h('div', { class: 'member' },
            h('span', { class: 'tag ' + (o.status === 'done' ? 'ok' : o.status === 'failed' || o.status === 'undo_failed' ? 'bad' : '') }, o.status),
            h('div', { class: 'info' }, h('div', { class: 'path' }, o.src), o.dst ? h('div', { class: 'path' }, '→ ' + o.dst) : null, o.message ? h('div', { class: 'small muted' }, o.message) : null))));
        } }, '▸'),
        h('div', { class: 'grow' }, h('b', {}, kinds[b.kind] || b.kind), h('span', { class: 'muted small' }, `  ·  ${fmtDate(b.created_at)} · ${b.done} xong${b.failed ? `, ${b.failed} lỗi` : ''} · ${fmtBytes(b.bytes)}`)),
        b.undone_at ? h('span', { class: 'tag' }, 'Đã hoàn tác') : null,
        canUndo ? h('button', { class: 'btn small', onclick: () => undo(b.id) }, 'Hoàn tác') : null), ops));
  }
  host.append(h('div', { class: 'section-title' }, 'Lịch sử quét'));
  for (const r of reports) {
    const st = r.stats;
    host.append(h('div', { class: 'member' }, h('span', { class: 'tag' }, r.kind === 'background' ? 'Ngầm' : 'App'),
      h('div', { class: 'info' }, h('div', {}, `${fmtDate(r.started_at)} · ${r.status}`),
        st ? h('div', { class: 'small muted' }, `${st.total_books} sách · ${st.exact_files} trùng chắc chắn · ${st.suspect_groups} nghi trùng · ${st.inbox_books} chờ phân loại · ${st.scan.missing} mất` +
          (st.scan.skipped_roots.length ? ` · bỏ qua: ${st.scan.skipped_roots.join(', ')}` : '')) : null)));
  }
  view.replaceChildren(host);
}


// ------------------------------------------------------------------ iPad (I3)
let ipadTimer = null;
let lastQr = null; // QR đang hiện: { p, until } – giữ qua các lần vẽ lại
async function renderIpad() {
  clearInterval(ipadTimer);
  toolbar('iPad / iPhone', 'gửi sách sang iPad, iPhone qua Wi-Fi nhà – mỗi máy một hàng chờ');
  const host = h('div', { class: 'pad' });
  view.replaceChildren(host);
  const open = new Set(); // thiết bị đang mở danh sách "đã có trên máy"
  // Wi-Fi là mạng Công cộng → Windows chặn iPad/điện thoại (I3-07). Kiểm tra chậm (~1 s) nên chỉ làm
  // khi vào màn hình, khi bật kết nối và sau khi sửa; không làm trong vòng làm mới 5 giây.
  let net = null, fixing = false;
  const checkNet = async () => { net = await call('ipad_net_check').catch(() => null); };
  const fixNet = async () => {
    fixing = true; draw();
    try {
      net = await call('ipad_net_fix', { nets: net.public.map((n) => n.index) });
      toast(net.public.length ? 'Chưa đổi được loại mạng — hãy đổi trong Cài đặt Windows.' : 'Đã chuyển sang mạng Riêng tư và mở cổng tường lửa.');
    } catch (e) { fail(e); }
    fixing = false; draw();
  };
  const netWarn = () => !net || !net.public.length ? null : h('div', { class: 'member' },
    h('span', { class: 'tag warn' }, 'Bị chặn'),
    h('div', { class: 'info grow' },
      h('div', {}, `Mạng ${net.public.map((n) => `“${n.name}”`).join(', ')} đang là mạng Công cộng — Windows sẽ chặn iPad/iPhone/Android kết nối tới máy này.`),
      h('div', { class: 'small muted' }, 'Bấm Sửa để chuyển sang mạng Riêng tư và mở cổng 47470 cho thiết bị trong nhà (Windows hỏi quyền quản trị một lần). Chỉ làm với mạng nhà bạn tin cậy, không làm với Wi-Fi quán cà phê, sân bay.')),
    h('button', { class: 'btn primary', disabled: fixing, onclick: fixNet }, fixing ? 'Đang sửa…' : 'Sửa'));
  const draw = async () => {
    if (S.view.name !== 'ipad') return clearInterval(ipadTimer);
    let st;
    try { st = await call('ipad_state'); } catch (e) { return fail(e); }
    if (!st.running || (S.devices && S.devices.length !== st.devices.length)) lastQr = null; // QR đã dùng / server tắt
    S.devices = st.devices;
    const kids = [];
    kids.push(h('div', { class: 'section-title' }, 'Kết nối'),
      h('div', { class: 'member' },
        h('span', { class: 'tag ' + (st.running ? 'ok' : '') }, st.running ? 'Đang bật' : 'Đang tắt'),
        h('div', { class: 'info grow' },
          h('div', {}, st.running ? `Cho phép iPad/iPhone kết nối tại ${st.addrs.map((a) => `${a}:${st.port}`).join(', ') || 'mạng nội bộ'}` : 'Máy chủ đồng bộ tắt. Chỉ bật khi cần gửi sách; tự tắt khi đóng app.'),
          h('div', { class: 'small muted' }, 'Chỉ chấp nhận thiết bị trong mạng nội bộ, cần token ghép nối, chỉ đọc với ổ sách. Nếu Windows hỏi tường lửa, chọn mạng Riêng tư.')),
        h('button', { class: 'btn ' + (st.running ? '' : 'primary'), onclick: async () => { const on = !st.running; await call('ipad_enable', { on }).catch(fail); if (on) await checkNet(); draw(); } },
          st.running ? 'Tắt' : 'Bật kết nối')),
      netWarn());

    if (st.running) {
      const qrBox = h('div', { class: 'qr-box' });
      if (lastQr && Date.now() < lastQr.until) renderQr(qrBox, lastQr.p); // vẽ lại sau mỗi lần làm mới màn hình
      kids.push(h('div', { class: 'section-title' }, 'Ghép thiết bị mới'),
        h('div', { class: 'member' }, h('div', { class: 'info grow' },
          h('div', {}, 'Trên iPad/iPhone: Đồng bộ → đặt tên máy → quét mã QR, hoặc gõ địa chỉ và mã 6 số. QR giữ nguyên ít nhất 1 phút, dùng một lần – mỗi máy một mã.'), qrBox),
          h('button', { class: 'btn', onclick: () => showQr(qrBox) }, 'Hiện mã QR')));
    }

    kids.push(h('div', { class: 'section-title' }, `Thiết bị đã ghép (${st.devices.length})`));
    if (!st.devices.length) kids.push(h('p', { class: 'muted' }, 'Chưa ghép iPad hay iPhone nào. Bật kết nối rồi ghép ở trên.'));
    for (const d of st.devices) {
      const phone = d.kind === 'iphone';
      const over = d.free_bytes != null && d.queue_bytes > d.free_bytes;
      kids.push(h('div', { class: 'device-card' },
        h('div', { class: 'member' }, h('span', { class: 'tag' }, phone ? '▯ iPhone' : '▭ iPad'),
          h('div', { class: 'info grow' }, h('div', {}, h('b', {}, d.name)),
            h('div', { class: 'small muted' }, [`Ghép ${fmtDate(d.created_at)}`, `kết nối gần nhất ${fmtDate(d.last_seen)}`, d.free_bytes != null ? `trống ${fmtBytes(d.free_bytes)}` : null,
              `có ${d.books} sách`, phone ? 'chỉ nhận sách chữ (không PDF, truyện tranh)' : null].filter(Boolean).join(' · '))),
          h('button', { class: 'btn small', onclick: () => renameDevice(d) }, 'Đổi tên'),
          h('button', { class: 'btn small danger', onclick: async () => {
            if (await modal({ title: `Gỡ “${d.name}”?`, body: 'Máy này sẽ không tải được sách nữa cho tới khi ghép lại; hàng chờ của nó bị xoá. Sách đã tải trên máy vẫn còn.', ok: 'Gỡ', danger: true })) { await call('ipad_remove_device', { id: d.id }).catch(fail); await refreshOverview(); draw(); }
          } }, 'Gỡ')),
        h('div', { class: 'device-body' },
          h('p', { class: over ? 'warn' : 'muted' }, `Hàng chờ: ${d.queue.length} cuốn · ${fmtBytes(d.queue_bytes)}` +
            (over ? ' — vượt dung lượng trống của máy, sẽ không tải hết được. Bỏ bớt sách.' : '')),
          ...(d.queue.length ? [
            h('div', {}, h('button', { class: 'btn small', onclick: async () => { await call('ipad_queue_remove', { ids: d.queue.map((i) => i.id), device: d.id }).catch(fail); draw(); } }, 'Xoá hàng chờ của máy này')),
            ...d.queue.map((it) => bookRow(it, h('button', { class: 'btn small', onclick: async () => { await call('ipad_queue_remove', { ids: [it.id], device: d.id }).catch(fail); draw(); } }, 'Bỏ'))),
          ] : [h('p', { class: 'muted small' }, `Hàng chờ trống. Chuột phải sách / bộ trong thư viện → “Gửi sang ${d.name}”.`)]),
          d.books ? h('div', {}, h('button', { class: 'btn small link', onclick: () => { open.has(d.id) ? open.delete(d.id) : open.add(d.id); draw(); } },
            open.has(d.id) ? `Ẩn ${d.books} sách đã có trên máy` : `Xem ${d.books} sách đã có trên máy`)) : null,
          open.has(d.id) ? h('div', { class: 'device-books' }, ...(await call('ipad_device_books', { id: d.id }).catch(() => [])).map((it) => bookRow(it))) : null)));
    }
    host.replaceChildren(...kids);
  };
  const bookRow = (it, action) => h('div', { class: 'member' }, h('span', { class: 'tag' }, it.format.toUpperCase()),
    h('div', { class: 'info grow' }, h('div', {}, it.title), h('div', { class: 'small muted' }, [it.series ? `${it.series}${it.volume != null ? ' · vol ' + it.volume : ''}` : it.authors, fmtBytes(it.size), action && !it.sha256 ? 'đang tính hash…' : null].filter(Boolean).join(' · '))),
    action || null);
  async function renameDevice(d) {
    const input = h('input', { type: 'text', value: d.name, style: 'width:100%' });
    setTimeout(() => { input.focus(); input.select(); }, 0);
    if (!(await modal({ title: 'Đổi tên thiết bị', content: input, ok: 'Lưu' }))) return;
    try { const name = await call('ipad_rename_device', { id: d.id, name: input.value }); toast(`Đã đổi tên thành “${name}”`); await refreshOverview(); } catch (e) { fail(e); }
    draw();
  }
  function renderQr(box, p) {
    const img = h('div', { class: 'qr' });
    img.innerHTML = p.svg;
    box.replaceChildren(img, h('div', {}, h('b', { class: 'code' }, p.code.replace(/(\d{3})(\d{3})/, '$1 $2'))), h('div', { class: 'small muted' }, `${p.addrs.map((a) => `${a}:${p.port}`).join(', ')}`));
  }
  async function showQr(box) {
    try {
      const p = await call('ipad_pair_qr'); // máy chủ giữ nguyên mã tối thiểu 60 giây
      lastQr = { p, until: Date.now() + p.remaining_ms };
      renderQr(box, p);
    } catch (e) { fail(e); }
  }
  await draw();
  checkNet().then(draw); // không chặn lần vẽ đầu
  ipadTimer = setInterval(draw, 5000);
}

// ------------------------------------------------------------------ cài đặt (E9-03)
function renderSettings() {
  toolbar('Cài đặt', '');
  const c = structuredClone(S.ov.config);
  const field = (label, input, hint) => h('div', { class: 'field' }, h('label', {}, label), input, hint ? h('div', { class: 'hint' }, hint) : null);
  const num = (k, min, max) => h('input', { type: 'number', min, max, value: c[k], onchange: (e) => { c[k] = Math.max(min, Math.min(max, +e.target.value || min)); } });
  const chk = (k, label, onChange) => h('label', { class: 'switch' }, h('input', { type: 'checkbox', checked: c[k], onchange: (e) => { c[k] = e.target.checked; onChange?.(e); } }), label);
  const theme = h('select', { class: 'pill', onchange: (e) => { store('theme', e.target.value || undefined); applyTheme(); } },
    ...[['', 'Theo Windows'], ['light', 'Sáng'], ['dark', 'Tối']].map(([v, l]) => h('option', { value: v, selected: load('theme', '') === v }, l)));
  const evalOut = h('span', { class: 'muted small' });
  const form = h('div', { class: 'form pad' },
    field('Thư mục thư viện', h('div', { style: 'display:flex;gap:8px;align-items:center;min-width:0' }, h('span', { class: 'path', style: 'flex:1' }, c.library_root || '(chưa chọn)'), h('button', { class: 'btn', onclick: chooseLibrary }, 'Đổi…'))),
    field('Thư mục hộp thư đến', h('input', { type: 'text', value: c.inbox_name, onchange: (e) => { c.inbox_name = e.target.value.trim() || 'Chưa phân loại'; } }), 'Tên thư mục con trong thư viện chứa sách mới. Thư mục này không bao giờ là đích phân loại.'),
    field('Bỏ qua thư mục', h('input', { type: 'text', value: c.exclude_names.join('; '), onchange: (e) => { c.exclude_names = e.target.value.split(';').map((s) => s.trim()).filter(Boolean); } }), 'Phân cách bằng dấu ;'),
    field('Chế độ chỉ đọc', chk('read_only', 'Không xoá, không di chuyển file nào'), 'Bật khi thử trên kho thật để đối chiếu kết quả trước.'),
    field('Quét ngầm', chk('auto_clean_exact_in_background', 'Tự dọn trùng chắc chắn khi quét ngầm', (e) => {
      if (e.target.checked) modal({ title: 'Bật tự dọn trùng?', body: 'Tác vụ ngầm sẽ tự đưa bản trùng chắc chắn (SHA-256) vào Thùng rác mà không hỏi. Mặc định tắt.', ok: 'Đã hiểu', cancel: 'Tắt lại' })
        .then((ok) => { if (!ok) { c.auto_clean_exact_in_background = false; e.target.checked = false; } });
    }), 'Mặc định tắt: quét ngầm chỉ lập danh sách.'),
    field('Tự phân loại hộp thư đến', chk('auto_triage', 'Mỗi lần quét, tự xếp sách vào thể loại', (e) => {
      if (e.target.checked) modal({ title: 'Bật tự phân loại?', body: 'Mỗi lần quét, sách trong hộp thư đến (kể cả thư mục kiểu Calibre) sẽ được đọc tag, chuyển vào thư mục thể loại — tạo thư mục mới nếu cần, sách chưa chắc vào “Khác” — rồi mọi file không phải sách (ảnh, âm thanh, video, .opf…) và thư mục con thừa được đưa vào Thùng rác. Truyện tranh và file .zip/.rar được giữ nguyên. Nhật ký di chuyển: triage-journal.jsonl trong thư mục dữ liệu app.', ok: 'Đã hiểu', cancel: 'Tắt lại' })
        .then((ok) => { if (!ok) { c.auto_triage = false; e.target.checked = false; } });
    }), 'Mặc định bật. Xem trước bằng lệnh: librarian triage.'),
    field('Giờ quét ngầm', h('input', { type: 'text', value: c.scan_time, onchange: (e) => { c.scan_time = e.target.value; } }), 'HH:MM – đăng ký Task Scheduler sẽ có ở M6.'),
    field('Ngưỡng tin cậy cao', num('classify_high', 1, 100), 'Đề xuất từ mức này trở lên được tick sẵn.'),
    field('Ngưỡng trung bình', num('classify_medium', 1, 100), 'Dưới mức này: đưa vào “Cần làm tay”.'),
    field('Số luồng băm', num('hash_threads', 1, 16), 'SSD ngoài: 2–4.'),
    field('Giao diện', theme),
    field('Độ chính xác phân loại', h('div', { style: 'display:flex;gap:10px;align-items:center' }, h('button', { class: 'btn', onclick: async () => {
      evalOut.textContent = 'Đang đo…';
      try {
        const ev = await call('evaluate');
        const pct = (a, b) => (b ? Math.round((100 * a) / b) : 0) + '%';
        evalOut.textContent = `${ev.total} sách: đúng ${pct(ev.correct, ev.total)} · cao ${ev.high_correct}/${ev.high_total} · trung bình ${ev.medium_correct}/${ev.medium_total} · làm tay ${ev.manual}`;
      } catch (e) { evalOut.textContent = String(e); }
    } }, 'Đo'), evalOut), 'Giấu thư mục của từng cuốn rồi đoán lại (leave-one-out).'),
    h('div', {}, h('button', { class: 'btn primary', onclick: async () => {
      if (c.classify_medium > c.classify_high) return fail('Ngưỡng trung bình phải ≤ ngưỡng cao');
      try { await call('save_config', { config: c }); toast('Đã lưu cài đặt.'); await refreshOverview(); } catch (e) { fail(e); }
    } }, 'Lưu')),
    h('p', { class: 'muted small' }, 'Dữ liệu app: ' + S.ov.home));
  view.replaceChildren(form);
}

function applyTheme() {
  const t = load('theme', '');
  if (t) document.documentElement.dataset.theme = t; else delete document.documentElement.dataset.theme;
}

// ------------------------------------------------------------------ phím tắt toàn cục
document.addEventListener('keydown', (e) => {
  if ($('#modal-root').children.length || readerOpen()) return;
  const typing = /INPUT|SELECT|TEXTAREA/.test(e.target.tagName);
  if (e.key === 'f' && e.ctrlKey) { e.preventDefault(); $('#toolbar input[type=search]')?.focus(); return; }
  // F5 = làm mới thư viện (thay vì tải lại trang WebView).
  if (e.key === 'F5') { e.preventDefault(); if (!S.scanning && S.ov?.library_ok) startScan(); return; }
  if (S.view.name === 'suspects') return suspectKeys?.(e);
  if (typing) { if (e.key === 'Escape') e.target.blur(); return; }
  if (!grid) return;
  if (e.key === 'Delete' && S.sel.size) { e.preventDefault(); deleteSelection(); }
  else if (e.key === 'Escape') { S.sel.clear(); grid.refreshSelection(); }
  else if (e.key === 'a' && e.ctrlKey) { e.preventDefault(); grid.loadedItems().forEach((it) => S.sel.add(key(it))); grid.refreshSelection(); }
  else if (e.key.toLowerCase() === 'g' && !e.ctrlKey && !e.altKey && !e.metaKey) setLayout(S.layout === 'grid' ? 'list' : 'grid');
  else if (e.key === 'Enter' && S.sel.size === 1) { const it = grid.loadedItems().find((x) => key(x) === [...S.sel][0]); if (it) openItem(it); }
});
view.addEventListener('click', (e) => { if (e.target === view || e.target.classList?.contains('pad') || e.target.classList?.contains('grid-wrap')) { S.sel.clear(); grid?.refreshSelection(); } });
document.addEventListener('contextmenu', (e) => { if (!e.target.closest('.card') && !/INPUT|TEXTAREA/.test(e.target.tagName)) e.preventDefault(); });

// ------------------------------------------------------------------ khởi động
applyTheme();
try {
  await refreshOverview();
  render();
  const startFile = await call('take_open_request');
  if (startFile) openPath(startFile);
} catch (e) {
  view.replaceChildren(h('div', { class: 'empty' }, h('div', {}, h('h2', {}, 'Không mở được dữ liệu'), h('p', {}, String(e)))));
}
