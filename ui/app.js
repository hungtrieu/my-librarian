// My Librarian – giao diện (không framework). Nghiệp vụ ở Rust; file này chỉ hiển thị và gọi lệnh.
import { h, fold, highlight, fmtBytes, fmtDate, genCover, debounce } from './ui-kit.js';
import { openReader, readerOpen } from './reader.js';

const TAURI = window.__TAURI__;
const api = TAURI ? { invoke: TAURI.core.invoke, listen: TAURI.event.listen } : await import('./mock.js');
const call = (cmd, args = {}) => api.invoke(cmd, args);
const COVER_FORMATS = new Set(['epub', 'mobi', 'azw', 'azw3', 'cbz', 'cbr', 'fb2']);
const coverUrl = (b) => (TAURI && b.id > 0 && COVER_FORMATS.has(b.format) ? `http://cover.localhost/${b.id}?v=${b.mtime}` : null);
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
  S.volumes = await call('volumes_info').catch(() => []);
  if (S.ov.notice) modal({ title: 'Đã khôi phục dữ liệu thư viện', body: S.ov.notice, cancel: null, ok: 'Đã hiểu' });
  S.scanning = S.scanning || S.ov.scanning;
  if (S.ov.cleaning && !CL.el) openCleanModal(); // giao diện tải lại khi đang dọn trùng ở nền
  renderNav();
  renderScanBox();
  renderBanner();
}
const stats = () => S.ov?.report?.stats || null;

function navItem(label, ico, target, count, badge, drop, depth) {
  const v = S.view;
  const cur = v.name === target.name && (target.scope ? v.scope === target.scope : true) && (target.category ? v.category === target.category : true)
    && (target.scope === 'category' ? fold(v.folder || '') === fold(target.folder || '') : true);
  // `drop`: thư mục thể loại / thư mục con nhận sách kéo thả từ lưới/danh sách ({ label, rel }).
  return h('button', { class: 'nav-item' + (depth > 1 ? ' sub' : ''), style: depth > 1 ? `padding-left:${12 + (depth - 1) * 16}px` : null,
    'aria-current': cur ? 'page' : null, 'data-drop': drop ? drop.label : null, 'data-rel': drop ? drop.rel : null, onclick: () => go(target) },
    h('span', { class: 'ico', 'aria-hidden': 'true' }, ico), h('span', { class: 'label' }, label),
    badge ? h('span', { class: 'badge' }, badge) : count != null ? h('span', { class: 'count' }, count.toLocaleString('vi-VN')) : null);
}

/** Thư mục con (do app quản lý) của thể loại `name`, theo thứ tự cây. */
function subfoldersOf(name) {
  const k = fold(name);
  return (S.ov.subfolders || []).filter((sf) => fold(sf.rel.split('/')[0]) === k);
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
    nav.push(h('div', { class: 'nav-section with-add' }, h('span', {}, 'Bộ sưu tập'),
      h('button', { class: 'nav-add', title: 'Thêm thể loại / quản lý thư mục', 'aria-label': 'Thêm thể loại', onclick: (e) => {
        e.stopPropagation();
        showMenu(e.clientX, e.clientY, [{ label: 'Thêm thể loại…', run: () => addFolder(null) }, { label: 'Quản lý thư mục thể loại…', run: () => openFolderManager() }]);
      } }, '+')));
    const inboxKey = fold(ov.config.inbox_name);
    for (const c of ov.categories) {
      if (fold(c.name) === inboxKey) continue;
      const el = navItem(c.name, '▤', { name: 'library', scope: 'category', category: c.name }, c.books, null, { label: c.name, rel: c.name });
      if (fold(c.name) !== fold('Khác')) {
        el.addEventListener('contextmenu', (e) => {
          e.preventDefault();
          const node = { rel: c.name, name: c.name, depth: 1, books: c.books };
          showMenu(e.clientX, e.clientY, [
            { label: 'Đổi tên thể loại…', run: () => renameFolder(node) },
            { label: 'Thêm thư mục con…', run: () => addFolder(c.name) },
            { label: 'Quản lý thư mục con…', run: () => openFolderManager(c.name) },
            '-',
            { label: `Chặn “${c.name}” hoặc thư mục con…`, danger: true, run: () => openBlockPicker(c.name) },
            { label: 'Xoá thể loại…', danger: true, run: () => deleteFolder(node) },
          ]);
        });
      }
      nav.push(el);
      // Thư mục con do app quản lý (thư mục con chép theo từ nguồn được làm phẳng khi quét).
      for (const sf of subfoldersOf(c.name)) {
        const se = navItem(sf.name, '↳', { name: 'library', scope: 'category', category: c.name, folder: sf.rel }, sf.books, null, { label: relLabel(sf.rel), rel: sf.rel }, sf.depth);
        se.addEventListener('contextmenu', (e) => {
          e.preventDefault();
          const node = { rel: sf.rel, name: sf.name, depth: sf.depth, books: sf.books };
          showMenu(e.clientX, e.clientY, [
            { label: 'Đổi tên thư mục con…', run: () => renameFolder(node) },
            { label: 'Thêm thư mục con…', run: () => addFolder(sf.rel) },
            '-',
            { label: `Chặn “${sf.name}”…`, danger: true, run: () => blockFolder(node) },
            { label: 'Xoá thư mục con…', danger: true, run: () => deleteFolder(node) },
          ]);
        });
        nav.push(se);
      }
    }
  }
  nav.push(h('div', { class: 'nav-section' }, 'Dọn dẹp'),
    navItem('Trùng chắc chắn', '⧉', { name: 'dupes' }, null, st?.exact_files || null),
    navItem('Nghi trùng', '≈', { name: 'suspects' }, null, st?.suspect_groups || null),
    navItem('Phân loại', '⇢', { name: 'classify' }, null, st?.inbox_books || null),
    navItem('Phân loại Khác', '⇉', { name: 'otherclassify' }, (ov.categories.find((c) => fold(c.name) === fold('Khác'))?.books) || null),
    navItem('Tag → thể loại', '#', { name: 'tags' }, (ov.config.tag_map || []).length || null),
    ov.facets.on_ipad ? navItem('Có trên iPad/iPhone', '▭', { name: 'library', scope: 'ipad' }, ov.facets.on_ipad) : null,
    ov.facets.problems ? navItem('Không đọc được', '⚠', { name: 'library', scope: 'problems' }, ov.facets.problems) : null,
    h('div', { class: 'nav-section' }, 'Khác'),
    navItem('Ổ lưu sách', '◫', { name: 'volumes' }, null, (S.volumes || []).filter((v) => v.low || !v.online).length || null),
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
    case 'triage_other': return { text: `Đang chia nhỏ thư mục Khác ${of('sách')}`, pct };
    case 'hash_quick': return { text: `Đang băm nhanh ${of('file')}`, pct };
    case 'hash_full': return { text: `Đang băm đầy đủ ${of('file')}`, pct };
    case 'suspect_hash': return { text: `Đang băm bổ sung để tìm nghi trùng ${of('file')}`, pct };
    case 'reject_deleted': return { text: `Đang so sách mới với sách đã xoá trước đây ${of('sách')}`, pct };
    case 'trash': return { text: `Đang đưa sách đã xoá trước đây (tải lại) vào Thùng rác ${of('file')}`, pct };
    case 'block': return { text: `Đang xoá sách thuộc thể loại bị chặn ${of('file')}`, pct };
    case 'block_clean': return { text: 'Đang dọn thư mục bị chặn…', pct: null };
    case 'flatten': return { text: `Đang đưa sách ra khỏi thư mục con lạ ${of('file')}`, pct };
    case 'flatten_clean': return { text: `Đang dọn thư mục con đã hết sách ${of('thư mục')}`, pct };
    case 'move': return { text: `Đang chuyển sách ${of('file')}`, pct };
    case 'analyze': {
      const part = ['tìm trùng chắc chắn', 'dọn trùng chắc chắn', 'tìm nghi trùng', 'đề xuất thể loại'][p.done || 0] || 'hoàn tất';
      return { text: `Đang phân tích: ${part} (phần ${Math.min((p.done || 0) + 1, p.total || 4)} / ${p.total || 4})`, pct };
    }
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
// Bấm đóng cửa sổ khi đang quét / dọn trùng (payload: 'scan' | 'clean'): hỏi lại; đồng ý thì dừng việc đó
// (phần đã làm được giữ lại) rồi app tự thoát.
let closeAsked = false;
api.listen('close-requested', async (e) => {
  if (closeAsked || S.quitting) return;
  closeAsked = true;
  const clean = e.payload === 'clean';
  const ok = await modal(clean ? {
    title: 'Đang dọn trùng chắc chắn',
    body: 'Việc đưa bản trùng vào Thùng rác chưa xong. Đóng app bây giờ sẽ dừng xoá sau file đang xử lý: các file đã xoá vẫn nằm trong Thùng rác Windows (khôi phục được), các bản còn lại giữ nguyên – lần sau dọn tiếp.',
    ok: 'Dừng xoá và đóng', cancel: 'Tiếp tục dọn', danger: true,
  } : {
    title: 'Đang quét thư viện',
    body: 'Quét chưa xong. Đóng app bây giờ sẽ dừng quét – phần đã quét được giữ lại, lần sau quét tiếp phần còn lại. Ổ chậm có thể cần vài giây để dừng hẳn.',
    ok: 'Dừng quét và đóng', cancel: 'Tiếp tục quét', danger: true,
  });
  closeAsked = false;
  if (!ok) return;
  S.quitting = true; renderScanBox(); paintClean();
  call('quit_app').catch(fail);
});

// Số liệu báo cáo vừa được tính lại ở nền lúc mở app: cập nhật badge / banner (quét đang chạy thì để scan-done lo).
api.listen('stats-updated', async () => {
  if (S.scanning) return;
  try {
    const ov = await call('overview');
    S.ov = { ...ov, notice: null }; // thông báo khôi phục (nếu có) đã hiện lúc mở app
    renderNav();
    renderBanner();
  } catch (e) { /* lần tải sau sẽ đúng */ }
});

api.listen('scan-health', (e) => { if (!S.scanning) return; S.health = e.payload; renderScanBox(); });

api.listen('scan-done', async (e) => {
  const stalled = S.health?.stalled;
  S.scanning = false; S.progress = null; S.health = null; S.autoSuspectsDone = false; // quét mới có thể sinh nhóm nghi trùng mới cần tự dọn
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

// ------------------------------------------------------------------ dọn trùng chạy nền (hộp tiến trình)
// Lõi gửi `clean-progress` ({ phase: 'trash' | 'refresh', done, total }), `clean-health` mỗi giây (giám sát treo)
// và đúng một `clean-done`. Hộp này không đóng được bằng Esc / bấm ra ngoài – chỉ "Dừng" hoặc khi xong.
const CL = { el: null, progress: null, health: null, stopping: false, kind: 'exact' };
const CL_TITLE = { exact: 'Đang dọn trùng chắc chắn', get suspects() { return `Đang tự dọn nghi trùng ≥${suspectAuto()}%`; }, block: 'Đang xoá sách trong thư mục bị chặn', balance: 'Đang chuyển sách sang ổ khác', undo: 'Đang trả sách về ổ cũ', tagmove: 'Đang chuyển sách cùng tag ra khỏi “Khác”', folderdel: 'Đang xoá thư mục', manualmove: 'Đang chuyển sách', otherclassify: 'Đang chuyển sách ra khỏi “Khác”' };
const CL_NOTE = {
  balance: 'Mỗi file được chép vào file tạm ở ổ mới, so SHA-256 với bản gốc, đổi tên rồi mới xoá bản cũ – mất điện giữa chừng cũng không hỏng sách. Dừng giữa chừng an toàn: sách đã chuyển giữ nguyên ở ổ mới (hoàn tác được ở Hoạt động), phần còn lại ở ổ cũ.',
  tagmove: 'Sách ở lại ổ đang chứa nó (chỉ đổi tên, tức thì). Dừng giữa chừng an toàn: sách đã chuyển giữ nguyên, hoàn tác được ở Hoạt động.',
  manualmove: 'Sách ở lại ổ đang chứa nó (chỉ đổi tên, tức thì). Dừng giữa chừng an toàn: sách đã chuyển giữ nguyên, hoàn tác được ở Hoạt động.',
  otherclassify: 'Sách ở lại ổ đang chứa nó (chỉ đổi tên, tức thì). Dừng giữa chừng an toàn: sách đã chuyển giữ nguyên, hoàn tác được ở Hoạt động.',
  folderdel: 'Sách trong thư mục được chuyển ra trước (thể loại → “Khác”, thư mục con → thư mục cha, cùng ổ), rồi thư mục vào Thùng rác. Dừng giữa chừng an toàn: thư mục được giữ lại.',
  undo: 'Chép an toàn từng file về chỗ cũ (file tạm → so SHA-256 → đổi tên → mới xoá). Dừng giữa chừng an toàn: bấm Hoàn tác lần nữa để làm tiếp phần còn lại.',
  block: 'Sách vào Thùng rác Windows (khôi phục được). Dừng giữa chừng an toàn: thư mục vẫn bị chặn, phần còn lại được dọn ở lần quét sau.',
};

function openCleanModal() {
  if (CL.el) return;
  Object.assign(CL, { progress: null, health: null, stopping: false });
  // Dựng một lần, mỗi sự kiện chỉ cập nhật chữ / thanh / nút tại chỗ – vẽ lại cả hộp thì nút "Dừng" bị thay giữa
  // lúc bấm (tiến trình tới mỗi file) và cú bấm bị mất.
  CL.txt = h('div', { class: 'txt' });
  CL.bar = h('i');
  CL.healthBox = h('div');
  CL.stop = h('button', { class: 'btn danger', onclick: stopClean }, 'Dừng');
  const title = CL_TITLE[CL.kind] || CL_TITLE.exact;
  CL.el = h('div', { class: 'modal-back' }, h('div', { class: 'modal', role: 'dialog', 'aria-modal': 'true', 'aria-label': title },
    h('h3', {}, title),
    h('p', {}, CL_NOTE[CL.kind] || 'Bản giữ lại được kiểm tra còn nguyên trước mỗi lần xoá. Dừng giữa chừng an toàn: file đã xoá nằm trong Thùng rác Windows (khôi phục được), phần còn lại giữ nguyên.'),
    CL.txt, h('div', { class: 'progress' }, CL.bar), CL.healthBox,
    h('div', { class: 'actions', style: 'margin-top:14px' }, CL.stop)));
  $('#modal-root').append(CL.el);
  paintClean();
}

function paintClean() {
  if (!CL.el) return;
  const p = CL.progress, n = (x) => (x || 0).toLocaleString('vi-VN');
  let text = 'Đang chuẩn bị…', pct = null;
  if (S.quitting) text = 'Đang dừng xoá để đóng app…';
  else if (CL.stopping) text = 'Đang dừng sau file hiện tại…';
  else if (p?.phase === 'trash') { text = `Đã đưa ${n(p.done)} / ${n(p.total)} bản thừa vào Thùng rác`; pct = p.total ? Math.min(100, Math.round((p.done / p.total) * 100)) : null; }
  else if (p?.phase === 'block') { text = p.total ? `Đã đưa ${n(p.done)} / ${n(p.total)} sách vào Thùng rác` : 'Đang tìm sách trong thư mục bị chặn…'; pct = p.total ? Math.min(100, Math.round((p.done / p.total) * 100)) : null; }
  else if (p?.phase === 'block_clean') text = 'Đang đưa thư mục bị chặn vào Thùng rác…';
  else if (p?.phase === 'move') { text = `Đã chuyển ${n(p.done)} / ${n(p.total)} sách ${({ undo: 'về chỗ cũ', tagmove: `vào “${CL.label}”`, manualmove: `vào “${CL.label}”`, otherclassify: 'ra khỏi “Khác”', folderdel: 'ra khỏi thư mục' })[CL.kind] || 'sang ổ khác'}`; pct = p.total ? Math.min(100, Math.round((p.done / p.total) * 100)) : null; }
  else if (p?.phase === 'refresh' && ['balance', 'undo', 'tagmove', 'manualmove', 'otherclassify', 'folderdel'].includes(CL.kind)) text = 'Đã chuyển xong – đang cập nhật số liệu…';
  else if (p?.phase === 'suspect_hash') { text = `Đang tính độ tin cậy nghi trùng: ${n(p.done)} / ${n(p.total)} file`; pct = p.total ? Math.min(100, Math.round((p.done / p.total) * 100)) : null; }
  else if (p?.phase === 'refresh') text = 'Đã xoá xong – đang cập nhật số liệu trùng / nghi trùng…';
  CL.txt.textContent = text;
  CL.bar.className = pct == null ? '' : 'det';
  CL.bar.style.width = pct == null ? '' : pct + '%';
  CL.healthBox.replaceChildren(...[healthLine(CL.health)].filter(Boolean));
  CL.stop.disabled = CL.stopping || S.quitting || p?.phase === 'refresh';
  CL.stop.textContent = CL.stopping || S.quitting ? 'Đang dừng…' : 'Dừng';
}

async function stopClean() {
  CL.stopping = true; paintClean();
  try { await call('cancel_clean'); } catch (e) { fail(e); }
}

/** Bắt đầu dọn các nhóm (full_hash) đã chọn; `keep`: full_hash → id bản người dùng chọn giữ. */
async function startClean(hashes, keep) {
  CL.kind = 'exact';
  openCleanModal(); // mở trước: sự kiện tiến trình đầu tiên có thể tới trước khi lệnh trả về
  try { await call('start_clean_exact', { hashes, keep }); } catch (e) { closeCleanModal(); fail(e); }
}

function closeCleanModal() { CL.el?.remove(); CL.el = null; }

/** Tự dọn các nhóm nghi trùng có độ tin cậy ≥ ngưỡng trong Cài đặt (giữ bản đề xuất, còn lại vào Thùng rác). */
async function startCleanSuspects() {
  CL.kind = 'suspects';
  openCleanModal();
  try { await call('start_clean_suspects'); } catch (e) { closeCleanModal(); fail(e); }
}

api.listen('clean-progress', (e) => { if (!CL.el) openCleanModal(); CL.progress = e.payload; CL.health = null; paintClean(); });
api.listen('clean-health', (e) => { if (!CL.el) return; CL.health = e.payload; paintClean(); });
api.listen('clean-done', async (e) => {
  const stalled = CL.health?.stalled;
  closeCleanModal();
  if (S.quitting) return; // app đang tự thoát
  const p = e.payload;
  const kind = CL.kind;
  if (kind === 'suspects') S.autoSuspectsDone = true; // đã thử tự dọn trong phiên này – không tự chạy lại liên tục
  const what = kind === 'block' ? 'xoá sách bị chặn' : kind === 'balance' ? 'chuyển sách sang ổ khác' : kind === 'undo' ? 'hoàn tác' : kind === 'tagmove' ? 'chuyển sách cùng tag' : kind === 'manualmove' || kind === 'otherclassify' ? 'chuyển sách' : kind === 'folderdel' ? 'xoá thư mục' : 'dọn trùng';
  if (!p.ok && stalled) modal({ title: `Đã dừng ${what} vì bị treo`, body: p.message, cancel: null, ok: 'Đã hiểu' });
  else if (!p.ok) fail(`Lỗi khi ${what}: ` + p.message);
  else if (kind === 'tagmove' || kind === 'manualmove' || kind === 'otherclassify') {
    const r = p.report;
    const where = kind === 'otherclassify' ? 'ra khỏi “Khác”' : `vào “${CL.label}”`;
    if (r.moved.length) toast((r.cancelled ? 'Đã dừng: ' : '') + `Đã chuyển ${r.moved.length.toLocaleString('vi-VN')} sách ${where}.`, { action: { label: 'Hoàn tác', run: () => undo(r.batch_id) }, ms: 9000 });
    else toast('Không còn sách nào cần chuyển.');
    if (r.failures.length) fail(`${r.failures.length} file không chuyển được: ${r.failures[0].message}`);
    await refreshOverview().catch(fail);
    if (S.view.name === 'library' || S.view.name === 'series') await refreshKeepScroll();
    else render();
    if (kind === 'manualmove' && !r.cancelled) await offerSimilarFromOther(r.moved);
    return;
  } else if (kind === 'folderdel') {
    const r = p.report, m = r.moves;
    if (r.cancelled) toast(`Đã dừng: ${m.moved.length} sách đã chuyển ra, thư mục “${CL.label}” được giữ lại.`, m.moved.length ? { action: { label: 'Hoàn tác', run: () => undo(m.batch_id) }, ms: 9000 } : {});
    else if (r.removed.length) toast(`Đã xoá “${CL.label}”` + (m.moved.length ? ` – ${m.moved.length.toLocaleString('vi-VN')} sách đã chuyển ra.` : '.') + ' Thư mục nằm trong Thùng rác.',
      m.moved.length ? { action: { label: 'Hoàn tác chuyển sách', run: () => undo(m.batch_id) }, ms: 9000 } : {});
    if (r.kept.length) modal({ title: 'Một số thư mục được giữ lại', cancel: null, ok: 'Đóng', content: h('div', { style: 'max-height:240px;overflow:auto;margin-bottom:16px' }, ...r.kept.map((k) => h('div', { class: 'path', style: 'margin-bottom:6px' }, k))) });
    if (m.failures.length) fail(`${m.failures.length} sách không chuyển ra được: ${m.failures[0].message}`);
    if (S.view.name === 'library' && S.view.scope === 'category' && fold(S.view.category) === fold(CL.label.split(' › ')[0]) && !CL.label.includes(' › ')) { S.view = { name: 'library', scope: 'all' }; }
    render();
  } else if (kind === 'undo') {
    const r = p.report;
    toast(r.cancelled ? `Đã dừng: ${r.restored} file đã về chỗ cũ. Bấm Hoàn tác ở Hoạt động để làm tiếp.` : `Đã trả ${r.restored} file về chỗ cũ.`, { error: !!r.failures.length, ms: 9000 });
    if (r.failures.length) fail(`${r.failures.length} file không hoàn tác được: ${r.failures[0].message}`);
  } else if (kind === 'balance') {
    const r = p.report;
    if (r.moved.length) toast((r.cancelled ? 'Đã dừng: ' : '') + `Đã chuyển ${r.moved.length.toLocaleString('vi-VN')} sách sang ổ khác.`, { action: { label: 'Hoàn tác', run: () => undo(r.batch_id) }, ms: 9000 });
    else if (!r.failures.length) toast('Không có sách nào cần chuyển.');
    if (r.failures.length) fail(`${r.failures.length} file không chuyển được: ${r.failures[0].message}`);
  } else if (kind === 'block') {
    const r = p.report;
    const msg = r.cancelled ? `Đã dừng: ${r.trashed} sách vào Thùng rác. Thư mục vẫn bị chặn – phần còn lại được dọn ở lần quét sau.`
      : `Đã chặn “${CL.label}”: ${r.trashed} sách vào Thùng rác (${fmtBytes(r.freed_bytes)}).`;
    toast(msg, { action: { label: 'Mở Thùng rác', run: () => call('open_recycle_bin') }, ms: 9000 });
    if (r.failures.length || r.aborted) reportTrash({ ...r, trashed: 0 });
  } else {
    const r = p.report;
    if (r.cancelled) toast(`Đã dừng: ${r.trashed} bản vào Thùng rác (${fmtBytes(r.freed_bytes)}), các bản còn lại giữ nguyên.`, { action: { label: 'Mở Thùng rác', run: () => call('open_recycle_bin') }, ms: 9000 });
    else reportTrash(r);
    if (r.cancelled && (r.failures.length || r.aborted)) reportTrash({ ...r, trashed: 0 });
  }
  await refreshOverview().catch(fail);
  if (['dupes', 'suspects', 'settings', 'volumes', 'activity'].includes(S.view.name) || kind === 'block') render();
});

// ------------------------------------------------------------------ thể loại bị chặn
const relLabel = (rel) => rel.split('/').join(' › ');

/** Chặn thư mục `node.rel`: hỏi lại, rồi lõi lưu cấu hình và đưa sách trong đó vào Thùng rác ở nền (hộp tiến trình). */
async function blockFolder(node) {
  if (S.ov.config.read_only) return fail('Đang bật chế độ chỉ đọc – tắt trong Cài đặt để chặn thể loại.');
  const ok = await modal({
    title: `Chặn “${relLabel(node.rel)}”?`,
    body: (node.books ? `${node.books.toLocaleString('vi-VN')} cuốn trong thư mục này (kể cả thư mục con) sẽ được đưa vào Thùng rác Windows ngay, rồi cả thư mục. ` : 'Thư mục sẽ được đưa vào Thùng rác Windows. ')
      + 'Từ nay, sách tự phân loại vào đây cũng vào thẳng Thùng rác và thư mục không được tạo lại. Sách vẫn khôi phục được từ Thùng rác; muốn đọc lại thể loại này thì bỏ chặn trong Cài đặt.',
    ok: 'Chặn và xoá', danger: true });
  if (!ok) return;
  CL.kind = 'block';
  CL.label = relLabel(node.rel);
  openCleanModal(); // mở trước: sự kiện tiến trình đầu tiên có thể tới trước khi lệnh trả về
  try { await call('block_category', { rel: node.rel }); } catch (e) { closeCleanModal(); fail(e); }
}

async function unblockFolder(rel) {
  try { await call('unblock_category', { rel }); } catch (e) { return fail(e); }
  toast(`Đã bỏ chặn “${relLabel(rel)}”. Sách đã xoá vẫn nằm trong Thùng rác Windows.`);
  await refreshOverview().catch(fail);
  if (S.view.name === 'settings') render();
}

/** Hộp chọn thư mục cần chặn: cây thư mục thể loại (3 cấp); `scope` = chỉ một thể loại và thư mục con của nó. */
async function openBlockPicker(scope) {
  let nodes;
  try { nodes = await call('category_tree'); } catch (e) { return fail(e); }
  if (scope) nodes = nodes.filter((n) => fold(n.rel.split('/')[0]) === fold(scope));
  const blockedRoots = new Set(S.ov.config.blocked_categories.map(fold));
  const list = h('div', { class: 'block-tree' });
  // Đóng hộp bằng chính nút "Đóng" của nó (gỡ cả phím tắt Esc), rồi mới mở hộp xác nhận.
  const pick = (run) => () => { $('#modal-root').lastElementChild?.querySelector('.actions .btn')?.click(); run(); };
  for (const n of nodes) {
    // Mục bị chặn trực tiếp: bỏ chặn được; thư mục con của mục bị chặn: chỉ hiện "Đã chặn".
    const action = !n.blocked ? h('button', { class: 'btn small danger', onclick: pick(() => blockFolder(n)) }, 'Chặn…')
      : blockedRoots.has(fold(n.rel)) ? h('button', { class: 'btn small', onclick: pick(() => unblockFolder(n.rel)) }, 'Bỏ chặn')
        : h('span', { class: 'tag' }, 'Đã chặn');
    list.append(h('div', { class: 'block-row' + (n.blocked ? ' is-blocked' : ''), style: `padding-left:${(n.depth - 1) * 20 + 4}px` },
      h('span', { class: 'grow' }, (n.depth > 1 ? '└ ' : '') + n.name),
      h('span', { class: 'muted small' }, n.exists ? `${n.books.toLocaleString('vi-VN')} cuốn` : 'đã xoá khỏi ổ'),
      action));
  }
  if (!nodes.length) list.append(h('p', { class: 'muted' }, 'Không có thư mục thể loại nào.'));
  await modal({ title: scope ? `Chặn trong “${scope}”` : 'Chặn thể loại', ok: 'Đóng', cancel: null,
    content: h('div', {},
      h('p', { class: 'muted small' }, 'Muốn chặn một phần của thể loại (vd. chỉ Đam mỹ trong “Ngôn tình - Đam mỹ”)? Tự tạo thư mục con (Đam mỹ, Ngôn tình) trong thư mục thể loại và chuyển sách vào – khi phân loại sách mới, app tự nhận ra thư mục con theo tag / tiêu đề.'),
      list) });
}

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
  ({ library: renderLibrary, series: renderLibrary, dupes: renderDupes, suspects: renderSuspects, classify: renderClassify, otherclassify: renderOtherClassify,
    activity: renderActivity, settings: renderSettings, ipad: renderIpad, tags: renderTags, volumes: renderVolumes })[v]();
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

const key = (it) => (it.kind !== 'series' ? `b:${it.id}` : it.work ? `w:${it.work}` : `s:${it.category ?? ''}|${it.series}`);
/** Nhãn định dạng của ô tác phẩm, vd. "EPUB · MOBI · PDF". */
const fmtList = (it) => (it.formats || []).map((f) => f.toUpperCase()).join(' · ');
/** Ảnh bìa đại diện của một ô bộ / tác phẩm: dùng đúng định dạng của bản chính để lấy bìa. */
const groupCoverBook = (it) => ({ id: it.cover_book_id, format: it.work ? it.formats[0] : 'cbz', mtime: 0, title: it.series, authors: it.authors });

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
  if (it.kind === 'series' && it.work) {
    const st = it.continue_book_id ? 'Đang đọc' : it.read ? 'Đã đọc' : fmtBytes(it.size);
    return [h('div', { class: 'thumb' }, coverFor(groupCoverBook(it), it.series, it.authors)),
      h('div', { class: 'c-title' }, h('span', { class: 't' }, highlight(it.series, S.q)), h('span', { class: 'tag', title: 'Một tác phẩm, nhiều định dạng' }, `${it.formats.length} định dạng`)),
      h('div', { class: 'c-author' }, highlight(it.authors || '—', S.q)),
      h('div', { class: 'c-fmt', title: fmtList(it) }, fmtList(it)),
      h('div', { class: 'c-dir' }, it.category || '—'),
      h('div', { class: 'c-prog' }, h('span', { class: it.read || it.continue_book_id ? '' : 'muted' }, st))];
  }
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
  if (it.kind === 'series' && it.work) {
    cover = h('div', { style: 'position:relative;width:100%;max-height:100%;display:flex;align-items:flex-end' },
      coverFor(groupCoverBook(it), it.series, it.authors), h('span', { class: 'vol-badge', title: 'Một tác phẩm, nhiều định dạng' }, `${it.formats.length} định dạng`));
    title = it.series; author = it.authors || '—';
    state = h('div', { class: 'state' }, it.continue_book_id ? 'Đang đọc · ' + fmtList(it) : it.read ? 'Đã đọc · ' + fmtList(it) : fmtList(it) + ' · ' + fmtBytes(it.size));
  } else if (it.kind === 'series') {
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
  const cv = it.kind === 'series' ? coverFor(groupCoverBook(it), it.series, it.authors) : coverFor(it);
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
  if (drop && d.target) moveToFolder(d.target.dataset.rel);
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

/** Lô chuyển tay lớn hơn chừng này cuốn thì chạy nền với hộp tiến trình (dừng được), không giữ giao diện chờ. */
const BIG_MOVE = 300;

/**
 * Thả vùng chọn lên thư mục thể loại / thư mục con `rel` (`A` hoặc `A/B`): di chuyển file (kind manual – hoàn tác được
 * ở toast và màn Hoạt động). Lô lớn chạy nền.
 */
async function moveToFolder(rel) {
  if (!rel) return;
  const parts = rel.split('/');
  const cat = S.ov.categories.find((c) => fold(c.name) === fold(parts[0]));
  if (!cat) return;
  const sf = parts.length > 1 ? (S.ov.subfolders || []).find((x) => fold(x.rel) === fold(rel)) : null;
  if (parts.length > 1 && !sf) return;
  const label = sf ? relLabel(sf.rel) : cat.name;
  if (S.ov.config.read_only) return fail('Đang bật chế độ chỉ đọc – tắt trong Cài đặt để di chuyển sách.');
  let books;
  try { books = (await selectedBooks()).filter((b) => b.id); } catch (e) { return fail(e); }
  // Đã nằm đúng thư mục (không tính thư mục con của nó).
  const want = (sf ? sf.rel : cat.name).split('/').map(fold).join('/');
  const dirOf = (b) => (b.rel_path || '').split(/[\\/]/).slice(0, -1).map(fold).join('/');
  const todo = books.filter((b) => dirOf(b) !== want && !(fold(b.category || '') === fold(cat.name) && !sf && !S.ov.config.flatten_subfolders));
  const already = books.length - todo.length;
  if (!todo.length) return toast(books.length === 1 ? `“${books[0].title}” đã ở trong “${label}”.` : `${books.length} quyển đã ở trong “${label}”.`);
  const dirFor = (b) => {
    if (sf) return sf.path;
    // Không làm phẳng: bộ truyện vào thư mục con cùng tên bộ nếu có (hành vi cũ).
    const s = !S.ov.config.flatten_subfolders && b.series && cat.series_dirs.find((x) => fold(x) === fold(b.series));
    return s ? cat.path + '\\' + s : cat.path;
  };
  const moves = todo.map((b) => ({ book_id: b.id, target_dir: dirFor(b) }));
  S.sel.clear();
  if (moves.length > BIG_MOVE) {
    CL.kind = 'manualmove';
    CL.label = label;
    openCleanModal();
    try { await call('start_apply_moves', { moves, kind: 'manual' }); } catch (e) { closeCleanModal(); fail(e); }
    return;
  }
  try {
    const r = await call('apply_moves', { moves, kind: 'manual' });
    if (r.failures.length) fail(`${r.failures.length} quyển không di chuyển được: ${r.failures[0].message}`);
    if (r.moved.length) toast(`Đã chuyển ${r.moved.length} quyển vào “${label}”` + (already ? ` · ${already} quyển đã ở đó sẵn` : '') + '.',
      { action: { label: 'Hoàn tác', run: () => undo(r.batch_id) } });
    await Promise.all([refreshOverview().catch(fail), refreshKeepScroll()]);
    await offerSimilarFromOther(r.moved);
    return;
  } catch (e) { fail(e); }
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
  if (it.kind === 'series' && it.work) return read(it.continue_book_id ?? it.cover_book_id);
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

/** Màn chi tiết một tác phẩm: liệt kê từng định dạng (mở / xoá / gửi riêng từng file). */
function openWork(it) {
  const back = S.view.name === 'series' ? S.view.back : S.view;
  go({ name: 'series', work: it.work, category: it.category, series: it.series, notWorth: inNotWorth(), back });
}

function itemMenu(x, y, it) {
  const n = S.sel.size;
  if (it.kind === 'series' && it.work) {
    return showMenu(x, y, [
      { label: `Đọc (${it.formats[0].toUpperCase()})`, run: () => openItem(it) },
      { label: `Xem ${it.formats.length} định dạng (${fmtList(it)})`, run: () => openWork(it) },
      { label: 'Hiện trong thư mục', run: () => call('reveal', { id: it.cover_book_id }).catch(fail) },
      { label: 'Thông tin & tag…', run: () => showBookInfo(it.cover_book_id) },
      n <= 1 ? { label: 'Sửa tiêu đề…', run: () => editTitle(it) } : null,
      '-',
      ...sendItems(n),
      { label: 'Đánh dấu đã đọc', run: () => markRead('read') },
      { label: 'Đánh dấu chưa đọc', run: () => markRead('unread') },
      notWorthItem(n, false),
      { label: n > 1 ? `Chuyển ${n} mục vào thư mục…` : 'Chuyển mọi định dạng vào thư mục…', run: moveSelection },
      '-', { label: n > 1 ? `Xoá ${n} mục vào Thùng rác…` : `Xoá cả ${it.volumes} file vào Thùng rác…`, danger: true, run: deleteSelection },
    ].filter(Boolean));
  }
  if (it.kind === 'series') {
    return showMenu(x, y, [
      { label: 'Mở bộ', run: () => openItem(it) },
      it.continue_book_id ? { label: 'Đọc tiếp', run: () => read(it.continue_book_id) } : null,
      ...sendItems(),
      notWorthItem(n, true),
      S.ov?.categories.length ? { label: n > 1 ? `Chuyển ${n} mục vào thư mục thể loại…` : `Chuyển cả bộ (${it.volumes} tập) vào thư mục thể loại…`, run: () => moveSelectionToCategory(it) } : null,
      '-', { label: n > 1 ? `Xoá ${n} mục vào Thùng rác…` : 'Xoá cả bộ vào Thùng rác…', danger: true, run: deleteSelection },
    ].filter(Boolean));
  }
  showMenu(x, y, [
    { label: 'Đọc', run: () => openItem(it) },
    { label: 'Mở bằng ứng dụng mặc định', run: () => openExternal(it.id) },
    { label: 'Hiện trong thư mục', run: () => call('reveal', { id: it.id }).catch(fail) },
    { label: 'Sao chép đường dẫn', run: () => navigator.clipboard.writeText(it.path).then(() => toast('Đã sao chép đường dẫn')) },
    it.id > 0 ? { label: 'Thông tin & tag…', run: () => showBookInfo(it.id) } : null,
    n <= 1 && it.id > 0 ? { label: 'Sửa tiêu đề…', run: () => editTitle(it) } : null,
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

// ------------------------------------------------------------------ sửa tiêu đề
const CONTENT_FORMATS = new Set(['epub', 'fb2']); // ghi được tiêu đề vào nội dung (retitle.rs)

/** Tên file (không đuôi) theo tiêu đề – khớp `retitle::file_stem_for` phía Rust (chỉ để xem trước). */
function fileStemFor(t) {
  let s = '';
  for (const c of t) s += c === ':' ? ' -' : '/\\|'.includes(c) ? '-' : c === '"' ? "'" : '?*<>'.includes(c) ? '' : c < ' ' ? ' ' : c;
  s = [...s.split(/\s+/).filter(Boolean).join(' ')].slice(0, 150).join('').replace(/^[. ]+|[. ]+$/g, '');
  return /^(con|prn|aux|nul|com\d|lpt\d)$/i.test(s.split('.')[0].trim()) ? s + '_' : s;
}
const extOf = (name) => { const i = name.lastIndexOf('.'); return i > 0 ? name.slice(i) : ''; };

/** Sửa tiêu đề một cuốn, hoặc mọi định dạng của một tác phẩm (ô "n định dạng"). */
async function editTitle(it) {
  if (S.scanning || CL.el) return toast('Đang quét hoặc dọn trùng – đợi xong rồi sửa tiêu đề.');
  let books;
  try { books = it.kind === 'series' && it.work ? (await call('query', { q: { work: it.work, not_worth: inNotWorth(), limit: 1000 } })).items : [it]; } catch (e) { return fail(e); }
  if (!books.length) return;
  const cur = it.kind === 'series' ? it.series : it.title;
  const writable = books.filter((b) => CONTENT_FORMATS.has(b.format) && !b.drm);
  const input = h('input', { type: 'text', class: 'text-input', value: cur, 'aria-label': 'Tiêu đề mới' });
  const rename = h('input', { type: 'checkbox', checked: true });
  const content = h('input', { type: 'checkbox', checked: writable.length > 0, disabled: !writable.length });
  const preview = h('div', { class: 'small muted', style: 'margin-top:6px;word-break:break-all' });
  const paint = () => {
    const stem = fileStemFor(input.value.trim());
    preview.textContent = !rename.checked ? '' : stem ? 'Tên file mới: ' + books.map((b) => stem + extOf(b.file_name)).join(', ') : 'Tiêu đề không dùng được làm tên file – giữ tên cũ';
  };
  input.addEventListener('input', paint); rename.addEventListener('change', paint);
  input.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); input.closest('.modal')?.querySelector('.actions .primary')?.click(); } });
  paint();
  const fmts = [...new Set(books.map((b) => b.format.toUpperCase()))].join(', ');
  const contentNote = writable.length
    ? `EPUB / FB2: sửa ngay trong sách (đọc lại để kiểm tra trước khi thay), bản gốc vào Thùng rác Windows – khôi phục được.${writable.length < books.length ? ' Định dạng khác chỉ sửa trong thư viện.' : ''}`
    : `${fmts} chưa ghi được vào nội dung – chỉ sửa trong thư viện (giữ qua các lần quét) và tên file.`;
  setTimeout(() => { input.focus(); input.select(); }, 0);
  const ok = await modal({
    title: books.length > 1 ? `Sửa tiêu đề (${books.length} định dạng)` : 'Sửa tiêu đề',
    ok: 'Lưu',
    content: h('div', { style: 'margin-bottom:16px' }, input, preview,
      h('label', { class: 'check' }, rename, h('span', {}, 'Đổi tên file theo tiêu đề mới')),
      h('label', { class: 'check' }, content, h('span', {}, 'Ghi tiêu đề vào nội dung sách', h('div', { class: 'small muted' }, contentNote))),
      h('p', { class: 'small muted', style: 'margin:10px 0 0' }, 'Tiêu đề mới được dùng ngay cho tìm kiếm và so nghi trùng; lần quét sau không ghi đè.')),
  });
  if (!ok) return;
  const title = input.value.trim();
  if (!title) return toast('Tiêu đề không được để trống.', { error: true });
  try {
    const r = await call('retitle_books', { ids: books.map((b) => b.id), title, renameFile: rename.checked, writeContent: content.checked });
    const renamed = r.done.filter((d) => d.renamed).length, written = r.done.filter((d) => d.content_written).length;
    const bits = [renamed ? `đổi tên ${renamed} file` : null, written ? `ghi vào nội dung ${written} file` : null].filter(Boolean);
    if (r.done.length) toast(`Đã sửa tiêu đề thành “${r.title}”${bits.length ? ' · ' + bits.join(', ') : ''}.`, written ? { action: { label: 'Mở Thùng rác', run: () => call('open_recycle_bin') } } : {});
    const notes = [...r.failures.map((f) => [f.path, f.message]), ...r.done.filter((d) => d.note).map((d) => [d.new_path, d.note])];
    if (notes.length) modal({ title: r.failures.length ? `${r.failures.length} file không sửa được` : 'Lưu ý', cancel: null, ok: 'Đóng',
      content: h('div', { style: 'max-height:240px;overflow:auto;margin-bottom:16px' }, ...notes.map(([p, m]) => h('div', { class: 'path', style: 'margin-bottom:6px' }, p, h('br'), h('span', { class: 'muted' }, m)))) });
  } catch (e) { return fail(e); }
  await refreshOverview().catch(fail);
  if (S.view.name === 'series') render(); else await refreshKeepScroll();
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
    } else if (k.startsWith('w:')) {
      const r = await call('query', { q: { work: k.slice(2), not_worth: inNotWorth(), limit: 100000 } });
      out.push(...r.items);
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
  const moves = books.map((b) => ({ book_id: b.id, target_dir: sel.value }));
  if (moves.length > BIG_MOVE) {
    CL.kind = 'manualmove';
    CL.label = targets.find((t) => t.path === sel.value)?.label || '';
    openCleanModal();
    try { await call('start_apply_moves', { moves, kind: 'manual' }); } catch (e) { closeCleanModal(); fail(e); }
    return;
  }
  try {
    const r = await call('apply_moves', { moves, kind: 'manual' });
    reportMoves(r);
    await refreshOverview();
    render();
    await offerSimilarFromOther(r.moved);
    return;
  } catch (e) { fail(e); }
  await refreshOverview();
  render();
}

/**
 * Chọn một thư mục thể loại / thư mục con rồi chuyển toàn bộ vùng chọn vào đó (bộ sách: mọi tập trong bộ).
 * Dùng chung `moveToFolder` với kéo thả: giữ thư mục con cùng tên bộ, bỏ qua tập đã ở đúng chỗ, lô lớn chạy nền.
 */
async function moveSelectionToCategory(it) {
  if (S.ov.config.read_only) return fail('Đang bật chế độ chỉ đọc – tắt trong Cài đặt để di chuyển sách.');
  const inboxKey = fold(S.ov.config.inbox_name);
  const opts = [];
  for (const c of S.ov.categories) {
    if (fold(c.name) === inboxKey) continue;
    opts.push(h('option', { value: c.name }, c.name));
    for (const sf of subfoldersOf(c.name)) opts.push(h('option', { value: sf.rel }, '  '.repeat(sf.depth - 1) + '↳ ' + sf.name));
  }
  if (!opts.length) return toast('Chưa có thư mục thể loại nào.');
  const sel = h('select', { class: 'pill', style: 'width:100%' }, ...opts);
  // Mặc định chọn thể loại hiện tại của bộ nếu có (dễ chuyển sang thư mục con cùng thể loại).
  if (it?.category && opts.some((o) => o.value === it.category)) sel.value = it.category;
  const n = S.sel.size;
  const title = n > 1 ? `Chuyển ${n} mục vào thư mục thể loại…` : `Chuyển cả bộ “${it.series}” (${it.volumes} tập) vào…`;
  const ok = await modal({ title, ok: 'Chuyển', content: h('div', { style: 'margin-bottom:16px' }, sel) });
  if (!ok) return;
  await moveToFolder(sel.value);
}

function reportMoves(r) {
  if (r.failures.length) fail(`${r.failures.length} file không di chuyển được: ${r.failures[0].message}`);
  if (r.moved.length) toast(`Đã di chuyển ${r.moved.length} file.`, { action: { label: 'Hoàn tác', run: () => undo(r.batch_id) } });
}

async function undo(id) {
  try {
    CL.kind = 'undo'; // lô chép qua ổ khác: hoàn tác chạy nền, sự kiện tiến trình có thể tới trước khi lệnh trả về
    const u = await call('undo_batch', { id });
    if (u.background) { openCleanModal(); return; }
    toast(`Đã trả ${u.restored} file về chỗ cũ` + (u.failures.length ? `, ${u.failures.length} file không hoàn tác được` : '.'), { error: !!u.failures.length });
  } catch (e) { fail(e); }
  await refreshOverview();
  render();
}

// ------------------------------------------------------------------ màn thư viện
function libraryQuery() {
  const v = S.view, base = { q: S.q || null, sort: S.sort, format: S.format || null, read_status: S.readStatus || null };
  if (v.name === 'series' && v.work) return { ...base, format: null, work: v.work, sort: 'relevance', not_worth: !!v.notWorth };
  if (v.name === 'series') return { ...base, series: v.series, category: v.category ?? '', sort: 'relevance', not_worth: !!v.notWorth };
  switch (v.scope) {
    case 'reading': return { ...base, read_status: 'reading', sort: S.q ? S.sort : 'recent_read' };
    case 'series': return { ...base, group_series: true, only_series: true };
    case 'inbox': return { ...base, category: S.ov.config.inbox_name };
    case 'problems': return { ...base, problems: true };
    case 'ipad': return { ...base, on_ipad: true };
    case 'not_worth': return { ...base, not_worth: true, group_series: true };
    case 'category': return { ...base, category: v.category, folder: v.folder || null, group_series: true };
    default: return { ...base, group_series: true };
  }
}

function libraryTitle() {
  const v = S.view;
  if (v.name === 'series') return v.series;
  return { all: 'Thư viện', reading: 'Đang đọc', series: 'Bộ truyện', inbox: S.ov.config.inbox_name, category: v.folder ? relLabel(v.folder) : v.category, problems: 'Không đọc được', ipad: 'Có trên iPad', not_worth: 'Không đáng đọc' }[v.scope];
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
  if (S.view.name === 'series' && S.view.work) {
    host.append(h('div', { style: 'display:flex;gap:8px;margin:2px 0 14px' },
      h('button', { class: 'btn small', onclick: () => go(S.view.back || { name: 'library', scope: 'all' }) }, '‹ Quay lại'),
      h('span', { class: 'muted small', style: 'align-self:center' }, 'Một tác phẩm, nhiều định dạng' + (S.view.category ? ` · ${S.view.category}` : ''))));
  } else if (S.view.name === 'series') {
    host.append(h('div', { style: 'display:flex;gap:8px;margin:2px 0 14px' },
      h('button', { class: 'btn small', onclick: () => go({ name: 'library', scope: S.view.notWorth ? 'not_worth' : 'series' }) }, S.view.notWorth ? '‹ Không đáng đọc' : '‹ Bộ truyện'),
      h('span', { class: 'muted small', style: 'align-self:center' }, S.view.category || '')));
  }
  view.replaceChildren(host);
  await reloadGrid(host);
  if (S.view.name === 'series' && !S.view.work && grid) {
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
    if (!(await modal({ title: `Dọn ${files} bản trùng?`, body: 'Bản giữ lại được kiểm tra còn nguyên trước khi xoá. Các bản thừa vào Thùng rác Windows. Nhiều file có thể mất vài phút – có thể dừng giữa chừng.', ok: 'Dọn', danger: true }))) return;
    await startClean(sel.map((g) => g.full_hash), Object.fromEntries([...keep].filter(([h]) => sel.some((g) => g.full_hash === h))));
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
/** Ngưỡng tự dọn nghi trùng (Cài đặt) – kẹp như `Config::suspect_auto_threshold` phía Rust. */
const suspectAuto = () => Math.min(100, Math.max(50, S.ov?.config?.suspect_auto ?? 85));
async function renderSuspects() {
  const AUTO_SUSPECT = suspectAuto();
  toolbar('Nghi trùng', `cùng tên/ISBN, nội dung khác – nhóm tin cậy ≥${AUTO_SUSPECT}% được tự dọn (giữ bản đề xuất), còn lại cần bạn duyệt`);
  view.replaceChildren(h('div', { class: 'pad muted' }, 'Đang tải…'));
  let groups;
  try { groups = await call('suspect_groups'); } catch (e) { return fail(e); }
  if ((stats()?.suspect_groups || 0) !== groups.length) refreshOverview().catch(fail);
  // Nhóm chắc chắn cao: tự dọn ở nền (một lần mỗi phiên, để lỗi / bấm Dừng không bị chạy lại liên tục).
  const auto = groups.filter((g) => g.confidence >= AUTO_SUSPECT); // kể cả nhóm có mục "đã xoá trước đây"
  if (auto.length && !S.autoSuspectsDone && !S.scanning && !CL.el) { startCleanSuspects(); }
  // Còn lại: duyệt tay, nhóm dễ (tin cậy cao) trước.
  groups.sort((a, b) => b.confidence - a.confidence);
  let i = 0;
  const host = h('div', { class: 'pad' });
  view.replaceChildren(host);
  const decide = async (decision) => {
    const g = groups[i];
    try { await call('decide', { ids: g.members.map((b) => b.id), decision }); } catch (e) { return fail(e); }
    // Không hiện toast: duyệt nhanh nhóm kế tiếp, toast chỉ che nút. Lỗi vẫn báo.
    groups.splice(i, 1); i = Math.min(i, groups.length - 1); paint(); refreshOverview().catch(fail);
  };
  // Chọn giữ một bản: xoá ngay các bản còn lại (vào Thùng rác Windows – khôi phục được), không hỏi lại, sang nhóm kế.
  // Xoá chạy nối tiếp ở nền: giao diện sang nhóm kế ngay, không chờ file vào Thùng rác xong (duyệt nhanh liên tục).
  // Thành công không hiện toast (toast che nút); chỉ báo khi có file không xoá được. Số liệu sidebar cập nhật khi
  // hàng chờ xoá chạy hết.
  let queue = Promise.resolve(), pending = 0;
  // Mục "đã xoá trước đây" (id âm) không còn file: chọn nó = xoá mọi bản đang có trong nhóm; chọn giữ một bản đang có
  // thì ghi "Giữ tất cả" với các mục đã xoá để nhóm không hiện lại.
  const keepOnly = (b) => {
    const g = groups[i];
    if (!g || !g.members.includes(b)) return;
    const drop = g.members.filter((x) => x.id !== b.id && x.id > 0), ghosts = g.members.filter((x) => x.id < 0);
    groups.splice(i, 1); i = Math.max(0, Math.min(i, groups.length - 1)); paint();
    pending++;
    queue = queue.then(async () => {
      try {
        const r = drop.length ? await call('trash_books', { ids: drop.map((x) => x.id) }) : { trashed: 0, freed_bytes: 0, failures: [] };
        if (b.id > 0 && ghosts.length) await call('decide', { ids: [b.id, ...ghosts.map((x) => x.id)], decision: 'keep_both' });
        if (r.failures.length || r.aborted) reportTrash({ ...r, trashed: 0 });
      } catch (e) { fail(e); groups.splice(i, 0, g); paint(); }
      if (--pending === 0) refreshOverview().catch(fail);
    });
  };
  const paint = () => {
    if (!groups.length) {
      host.replaceChildren(h('div', { class: 'empty' }, h('div', {}, h('h2', {}, 'Đã duyệt hết'), h('p', {}, 'Không còn nhóm nghi trùng nào.'))));
      refreshOverview();
      return;
    }
    const g = groups[i];
    host.replaceChildren(
      h('div', { class: 'section-title' }, `Nhóm ${i + 1}/${groups.length}`, h('span', { class: 'tag warn' }, g.reason),
        h('span', { class: 'tag ' + (g.confidence >= 70 ? 'ok' : 'bad'), title: 'Độ tin cậy là cùng một cuốn (ISBN, tên, tác giả, dung lượng, số trang)' }, `Tin cậy ${g.confidence}%`)),
      h('div', { class: 'compare' }, ...g.members.map((b, k) => b.id < 0
        ? h('div', { class: 'cmp-card deleted' },
          h('div', { class: 'cv' }, coverFor(b)),
          h('div', {}, h('span', { class: 'tag bad' }, 'Đã xoá trước đây'), h('div', {}, h('b', {}, b.title)), h('div', { class: 'muted small' }, b.authors || '—')),
          h('dl', {}, ...[['Định dạng', b.format.toUpperCase()], ['Dung lượng', fmtBytes(b.size)], ['Số trang', b.page_count ?? '—'], ['ISBN', b.isbn || '—'],
            ['Chỗ cũ', b.rel_path]].flatMap(([a, v]) => [h('dt', {}, a), h('dd', {}, String(v))])),
          h('div', { style: 'display:flex;gap:6px;flex-wrap:wrap' },
            h('button', { class: 'btn small', title: 'Bạn đã xoá cuốn này: đưa các bản đang có trong nhóm vào Thùng rác', onclick: () => keepOnly(b) }, 'Vẫn xoá bản mới', k < 9 ? h('span', { class: 'kbd', style: 'margin-left:6px' }, String(k + 1)) : null)))
        : h('div', { class: 'cmp-card' },
        h('div', { class: 'cv' }, coverFor(b)),
        h('div', {}, h('b', {}, b.title), h('div', { class: 'muted small' }, b.authors || '—')),
        h('dl', {}, ...[['Định dạng', b.format.toUpperCase()], ['Dung lượng', fmtBytes(b.size)], ['Số trang', b.page_count ?? '—'], ['ISBN', b.isbn || '—'],
          ['Thư mục', b.rel_path], ['Sửa lúc', fmtDate(Math.floor(b.mtime / 1000))]].flatMap(([a, v]) => [h('dt', {}, a), h('dd', {}, String(v))])),
        h('div', { style: 'display:flex;gap:6px;flex-wrap:wrap' },
          h('button', { class: 'btn small ' + (b.id === g.keeper_id ? 'primary' : ''), title: b.id === g.keeper_id ? 'Bản đề xuất giữ (nhiều trang / dung lượng lớn hơn)' : '', onclick: () => keepOnly(b) }, b.id === g.keeper_id ? 'Giữ bản này ★' : 'Giữ bản này', k < 9 ? h('span', { class: 'kbd', style: 'margin-left:6px' + (b.id === g.keeper_id ? ';color:#fff;border-color:#fff8' : '') }, String(k + 1)) : null),
          h('button', { class: 'btn small', title: 'Mở trong trình đọc để so sánh', onclick: () => read(b.id) }, 'Mở thử'))))),
      h('div', { style: 'display:flex;gap:8px;margin-top:18px;flex-wrap:wrap;align-items:center' },
        h('button', { class: 'btn', disabled: i === 0, onclick: () => { i--; paint(); } }, '← Trước'),
        h('button', { class: 'btn', disabled: i >= groups.length - 1, onclick: () => { i++; paint(); } }, 'Sau →'),
        h('span', { style: 'flex:1' }),
        h('button', { class: 'btn', onclick: () => decide('not_duplicate') }, 'Không trùng ', h('span', { class: 'kbd' }, 'N')),
        h('button', { class: 'btn', onclick: () => decide('keep_both') }, 'Giữ tất cả ', h('span', { class: 'kbd' }, 'K')),
        h('button', { class: 'btn primary', title: 'Giữ bản đề xuất ★, các bản còn lại vào Thùng rác', onclick: () => keepOnly(g.members.find((x) => x.id === g.keeper_id) || g.members[0]) }, 'Giữ bản ★ ', h('span', { class: 'kbd', style: 'color:#fff;border-color:#fff8' }, 'Enter'))));
  };
  paint();
  suspectKeys = (e) => {
    if (S.view.name !== 'suspects' || !groups.length || $('#modal-root').children.length || e.target.tagName === 'INPUT') return;
    if (e.key === 'ArrowRight' && i < groups.length - 1) { i++; paint(); }
    else if (e.key === 'ArrowLeft' && i > 0) { i--; paint(); }
    else if (e.key.toLowerCase() === 'n') decide('not_duplicate');
    else if (e.key.toLowerCase() === 'k') decide('keep_both');
    else if (/^[1-9]$/.test(e.key) && groups[i].members[+e.key - 1]) keepOnly(groups[i].members[+e.key - 1]);
    else if (e.key === 'Enter') { e.preventDefault(); keepOnly(groups[i].members.find((x) => x.id === groups[i].keeper_id) || groups[i].members[0]); }
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

// ------------------------------------------------------------------ tag → thể loại
/**
 * Duyệt tag (metadata "subject") của sách và gán mỗi tag vào một thư mục thể loại (kể cả thư mục con). Các cách viết
 * cùng khoá (Tiểu thuyết / tieu-thuyet) là một dòng; nhiều tag (novel, fiction…) gán được vào cùng thư mục. Lưu thay
 * đổi → xem trước sách nào sẽ chuyển đi đâu → lõi chuyển (lô "tagmap", hoàn tác được ở Hoạt động).
 */
async function renderTags() {
  if (!S.ov.library_ok) return renderWelcome();
  const T = { tags: [], targets: [], pending: new Map(), sel: new Set(), q: fold(S.view.tag || ''), mode: 'all', limit: 300 };
  const search = h('input', { type: 'search', placeholder: 'Tìm tag…', 'aria-label': 'Tìm tag', value: S.view.tag || '' });
  search.addEventListener('input', debounce(() => { T.q = fold(search.value.trim()); T.limit = 300; paint(); }, 160));
  const modeSel = h('select', { class: 'pill', 'aria-label': 'Lọc', onchange: (e) => { T.mode = e.target.value; T.limit = 300; paint(); } },
    ...[['all', 'Mọi tag'], ['unmapped', 'Chưa gán'], ['mapped', 'Đã gán']].map(([v, l]) => h('option', { value: v }, l)));
  toolbar('Tag → thể loại', 'gán metadata của sách vào thư mục thể loại', h('div', { class: 'search' }, search), modeSel,
    h('button', { class: 'btn', title: 'Chuyển mọi sách mang tag đã gán về đúng thư mục', onclick: () => preview([], true) }, 'Áp dụng lại tất cả'));
  view.replaceChildren(h('div', { class: 'pad muted' }, 'Đang đọc tag của thư viện…'));
  let tree;
  try { [T.tags, tree] = await Promise.all([call('tag_list'), call('category_tree')]); } catch (e) { return fail(e); }
  // Thư mục đích chọn được: cây thể loại (3 cấp, không gồm hộp thư đến / Khác / thư mục bị chặn) + đích đã gán mà
  // thư mục không còn trên đĩa (sẽ được tạo lại khi chuyển).
  T.targets = tree.filter((n) => !n.blocked).map((n) => ({ rel: n.rel }));
  for (const t of T.tags) if (t.target && !T.targets.some((x) => fold(x.rel) === fold(t.target))) {
    T.targets.push({ rel: t.target, missing: true });
  }
  T.targets.sort((a, b) => fold(a.rel).localeCompare(fold(b.rel), 'vi'));

  const cur = (t) => (T.pending.has(t.key) ? T.pending.get(t.key).target : t.target) || '';
  const setTarget = (t, target) => {
    if ((target || '') === (t.target || '')) T.pending.delete(t.key);
    else T.pending.set(t.key, { tag: t.label, target: target || null });
  };
  const targetSelect = (value, onchange, label) => h('select', { 'aria-label': label, onchange: (e) => onchange(e.target.value) },
    h('option', { value: '' }, '— chưa gán —'),
    ...T.targets.map((x) => h('option', { value: x.rel, selected: fold(x.rel) === fold(value) }, relLabel(x.rel) + (x.missing ? ' (chưa có)' : ''))));
  const host = h('div', { class: 'pad' }), bar = h('div', { class: 'sticky-bar' });

  const paintBar = () => {
    const n = T.pending.size;
    const bulk = T.sel.size ? [h('span', { class: 'muted small' }, `${T.sel.size} tag đã chọn →`),
      targetSelect('', (v) => { for (const t of T.tags) if (T.sel.has(t.key)) setTarget(t, v); T.sel.clear(); paint(); }, 'Gán các tag đã chọn vào')] : [];
    bar.replaceChildren(h('span', { class: 'grow' }, n ? `${n} thay đổi chưa lưu. Sách mang các tag này sẽ được chuyển sang thư mục mới – xem trước trước khi chuyển.`
      : 'Chọn thư mục cho từng tag, hoặc tick nhiều tag rồi gán một lần. Tiểu thuyết / tieu-thuyet / TIỂU THUYẾT là cùng một tag.'),
    ...bulk,
    n ? h('button', { class: 'btn', onclick: () => { T.pending.clear(); paint(); } }, 'Bỏ thay đổi') : null,
    h('button', { class: 'btn primary', disabled: !n, onclick: () => preview([...T.pending.values()], false) }, n ? `Lưu ${n} thay đổi…` : 'Lưu'));
  };

  const paint = () => {
    const shown = T.tags.filter((t) => (!T.q || fold(t.variants.join(' ') + ' ' + (t.target || '')).includes(T.q))
      && (T.mode === 'all' || (T.mode === 'mapped') === !!cur(t)));
    const rows = shown.slice(0, T.limit).map((t) => {
      const changed = T.pending.has(t.key);
      return h('tr', { class: changed ? 'changed' : '' },
        h('td', {}, h('input', { type: 'checkbox', checked: T.sel.has(t.key), 'aria-label': 'Chọn tag', onchange: (e) => { e.target.checked ? T.sel.add(t.key) : T.sel.delete(t.key); paintBar(); } })),
        h('td', {}, h('div', {}, t.label), t.variants.length > 1 ? h('div', { class: 'muted small' }, t.variants.filter((v) => v !== t.label).join(' · ')) : null),
        h('td', { class: 'num' }, t.books.toLocaleString('vi-VN')),
        h('td', { class: 'small' }, ...t.folders.map(([f, n]) => h('span', { class: 'tag', style: 'margin:0 4px 2px 0;display:inline-block' }, `${f ? relLabel(f) : '(gốc)'} · ${n}`))),
        h('td', {}, targetSelect(cur(t), (v) => { setTarget(t, v); paint(); }, `Thể loại cho tag ${t.label}`), changed ? h('span', { class: 'tag warn', style: 'margin-left:6px' }, 'chưa lưu') : null));
    });
    const table = h('table', { class: 'list tag-map' },
      h('thead', {}, h('tr', {}, h('th', {}, ''), h('th', {}, 'Tag'), h('th', {}, 'Sách'), h('th', {}, 'Đang nằm ở'), h('th', {}, 'Thể loại'))),
      h('tbody', {}, ...rows));
    const more = shown.length > T.limit ? h('div', { style: 'text-align:center;margin:12px' },
      h('button', { class: 'btn', onclick: () => { T.limit += 300; paint(); } }, `Hiện thêm (${(shown.length - T.limit).toLocaleString('vi-VN')} tag)`)) : null;
    host.replaceChildren(T.tags.length ? (shown.length ? table : h('p', { class: 'muted' }, 'Không có tag nào khớp.'))
      : h('div', { class: 'empty' }, h('div', {}, h('h2', {}, 'Chưa có tag nào'), h('p', {}, 'Sách trong thư viện chưa có metadata thể loại (subject / tag). Quét lại sau khi thêm sách có tag.'))), more || '');
    paintBar();
  };

  /** Xem trước rồi lưu + chuyển. `all`: áp dụng lại mọi tag đã gán. */
  async function preview(changes, all) {
    if (S.ov.config.read_only) return fail('Đang bật chế độ chỉ đọc – tắt trong Cài đặt để gán tag và chuyển sách.');
    let plan;
    try { plan = await call('tag_map_plan', { changes, all }); } catch (e) { return fail(e); }
    const n = plan.moves.length;
    const list = h('div', { class: 'block-tree' });
    for (const m of plan.moves.slice(0, 500)) list.append(h('div', { class: 'block-row' },
      h('span', { class: 'grow', title: m.file_name }, m.title || m.file_name),
      h('span', { class: 'muted small', style: 'white-space:nowrap' }, `${relLabel(m.from) || '(gốc)'} → ${relLabel(m.to)}`)));
    if (n > 500) list.append(h('div', { class: 'block-row muted small' }, `… và ${(n - 500).toLocaleString('vi-VN')} cuốn nữa`));
    const conflicts = plan.conflicts.length ? h('div', {},
      h('p', { class: 'small' }, h('b', {}, `${plan.conflicts.length} cuốn không chuyển`), ' vì mang nhiều tag gán vào các thư mục khác nhánh – gán lại một trong các tag, hoặc chuyển tay:'),
      h('div', { class: 'block-tree', style: 'max-height:160px' }, ...plan.conflicts.slice(0, 200).map((c) => h('div', { class: 'block-row' },
        h('span', { class: 'grow', title: c.file_name }, c.title || c.file_name), h('span', { class: 'muted small' }, c.targets.map(relLabel).join(' / ')))))) : null;
    const ok = await modal({
      title: all ? 'Áp dụng lại mọi tag đã gán?' : `Lưu ${changes.length} thay đổi?`,
      ok: n ? `${all ? '' : 'Lưu và '}chuyển ${n.toLocaleString('vi-VN')} cuốn` : (all ? 'Đóng' : 'Lưu'),
      cancel: all && !n ? null : 'Huỷ',
      content: h('div', {},
        h('p', { class: 'small' }, n ? `${n.toLocaleString('vi-VN')} cuốn sẽ được chuyển (trùng tên thêm (1), không ghi đè). Thư mục bộ / tác giả bên trong đi theo sách. Hoàn tác được ở Hoạt động.`
          : 'Không có sách nào cần chuyển.' + (all ? '' : ' Sách mới mang các tag này sẽ được xếp theo thể loại đã gán.')),
        n ? list : null, conflicts),
    });
    if (!ok || (all && !n)) return;
    try { reportMoves(await call('tag_map_save', { changes, all })); } catch (e) { return fail(e); }
    if (!all) toast(`Đã lưu ${changes.length} thay đổi.`);
    await refreshOverview().catch(fail);
    render();
  }

  paint();
  view.replaceChildren(host, bar);
}

// ------------------------------------------------------------------ thông tin sách, tag
/** Khoá tag giống lõi (tagmap::tag_key): bỏ dấu, ký tự ngăn cách thành khoảng trắng. */
const tagKey = (t) => fold(t).replace(/[^\p{L}\p{N}]+/gu, ' ').trim();
const splitTags = (s) => (s || '').split(/[;,|]/).map((t) => t.trim()).filter((t) => tagKey(t));

/** Hỏi một dòng chữ (tên thư mục…); trả về chuỗi đã cắt khoảng trắng, hoặc null nếu huỷ. */
async function askText({ title, value = '', ok = 'Lưu', hint }) {
  const input = h('input', { type: 'text', class: 'text-input', value, 'aria-label': title });
  input.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); input.closest('.modal')?.querySelector('.actions .primary')?.click(); } });
  setTimeout(() => { input.focus(); input.select(); }, 0);
  const res = await modal({ title, ok, content: h('div', { style: 'margin-bottom:16px' }, input, hint ? h('p', { class: 'small muted', style: 'margin:8px 0 0' }, hint) : null) });
  return res ? input.value.trim() : null;
}

/** Hộp "Thông tin & tag": metadata đọc từ file sách, tag (bấm để mở ở màn Tag → thể loại) và thể loại đang gán. */
async function showBookInfo(id) {
  let b;
  try { b = await call('book', { id }); } catch (e) { return fail(e); }
  if (!b) return fail('Không tìm thấy sách.');
  const rules = new Map((S.ov.config.tag_map || []).map((r) => [tagKey(r.tag), r.target]));
  const tags = splitTags(b.subjects);
  const row = (k, v) => (v == null || v === '' ? null : h('tr', {}, h('th', {}, k), h('td', {}, v)));
  const closeAnd = (run) => () => { $('#modal-root').lastElementChild?.querySelector('.actions .btn')?.click(); run(); };
  const tagBox = tags.length ? h('div', { class: 'tag-chips' }, ...tags.map((t) => {
    const target = rules.get(tagKey(t));
    return h('button', { class: 'tag chip', title: 'Mở ở màn Tag → thể loại', onclick: closeAnd(() => go({ name: 'tags', tag: t })) }, t, target ? h('span', { class: 'muted' }, ' → ' + relLabel(target)) : null);
  })) : h('span', { class: 'muted' }, 'Sách không có tag (metadata subject).');
  const status = { unread: 'Chưa đọc', reading: 'Đang đọc', read: 'Đã đọc' }[b.read_status] || b.read_status;
  const edit = await modal({ title: b.title, ok: 'Sửa thông tin…', cancel: 'Đóng', content: h('div', { style: 'margin-bottom:16px' },
    h('table', { class: 'meta-table' }, h('tbody', {},
      row('Tag', tagBox),
      row('Tác giả', b.authors),
      row('Bộ', b.series ? b.series + (b.volume ? ` · tập ${b.volume}` : '') : null),
      row('Ngôn ngữ', b.language),
      row('ISBN', b.isbn),
      row('Số trang', b.page_count),
      row('Định dạng', `${b.format.toUpperCase()} · ${fmtBytes(b.size)}` + (b.drm ? ' · có DRM' : '')),
      row('Thể loại', b.category ? (b.in_inbox ? b.category + ' (chờ phân loại)' : b.category) : null),
      row('Đường dẫn', h('span', { class: 'path' }, b.path)),
      row('Trạng thái đọc', status + (b.progress ? ` · ${Math.round(b.progress * 100)}%` : '')),
      row('Tiêu đề', b.title_from_meta ? 'đọc từ metadata của sách' : 'lấy từ tên file'),
      row('SHA-256', b.full_hash ? h('span', { class: 'path small' }, b.full_hash) : null),
      row('Lỗi đọc', b.error),
    ))) });
  if (edit) await editBookInfo(b);
}

/**
 * Sửa thông tin sách: luôn sửa trong thư viện (giữ qua các lần quét); EPUB / FB2 ghi được luôn vào file (bản gốc vào
 * Thùng rác Windows, khôi phục được). Định dạng khác chỉ sửa trong thư viện.
 */
async function editBookInfo(b) {
  const split = (s) => (s || '').split(';').map((x) => x.trim()).filter(Boolean);
  const inp = (value, ph) => h('input', { type: 'text', class: 'pill', style: 'width:100%', value: value ?? '', placeholder: ph || '' });
  const f = {
    title: inp(b.title), authors: inp(split(b.authors).join('; '), 'Nhiều tác giả: cách nhau bằng ;'),
    tags: inp(split(b.subjects).join('; '), 'Văn học; Tuổi học trò'), series: inp(b.series), volume: h('input', { type: 'number', min: 0, class: 'pill', style: 'width:90px', value: b.volume ?? '' }),
    language: inp(b.language, 'vi, en…'), isbn: inp(b.isbn),
  };
  const canWrite = ['epub', 'fb2'].includes(b.format) && !b.drm;
  const write = h('input', { type: 'checkbox', checked: canWrite && !S.ov.config.read_only, disabled: !canWrite || S.ov.config.read_only });
  const row = (k, el) => h('tr', {}, h('th', {}, k), h('td', {}, el));
  const ok = await modal({ title: 'Sửa thông tin sách', ok: 'Lưu', content: h('div', { style: 'margin-bottom:16px' },
    h('table', { class: 'meta-table' }, h('tbody', {}, row('Tiêu đề', f.title), row('Tác giả', f.authors), row('Tag', f.tags),
      row('Bộ', h('div', { style: 'display:flex;gap:8px' }, f.series, f.volume)), row('Ngôn ngữ', f.language), row('ISBN', f.isbn))),
    h('label', { class: 'check' }, write, h('span', {}, 'Ghi vào file sách',
      h('div', { class: 'small muted' }, canWrite ? 'Ghi vào metadata của file (EPUB / FB2) – trình đọc khác cũng thấy. Bản gốc vào Thùng rác Windows.'
        : `${b.format.toUpperCase()}${b.drm ? ' có DRM' : ''}: chưa ghi được vào file – chỉ sửa trong thư viện (vẫn giữ sau mỗi lần quét).`)))) });
  if (!ok) return;
  const meta = { title: f.title.value, authors: split(f.authors.value), tags: split(f.tags.value), series: f.series.value,
    volume: f.volume.value === '' ? null : Math.max(0, Math.round(+f.volume.value)), language: f.language.value, isbn: f.isbn.value };
  try {
    const r = await call('edit_book_meta', { id: b.id, meta, writeFile: write.checked });
    toast(r.written ? 'Đã lưu thông tin vào file sách và thư viện.' : 'Đã lưu thông tin trong thư viện.' + (r.note && write.checked ? ' ' + r.note : ''), { error: !!(r.note && write.checked) });
  } catch (e) { return fail(e); }
  await refreshOverview().catch(fail);
  await refreshKeepScroll();
}

const CRIT_KIND = { author: 'Tác giả', series: 'Bộ / tên sách', tag: 'Tag', part: 'Cụm trong tên file' };

/**
 * Sau khi chuyển tay sách ra khỏi "Khác": tìm các cuốn còn trong "Khác" (mọi ổ) giống sách vừa chuyển – theo tác giả
 * (kể cả tên tác giả nằm trong tên file), bộ / tên sách, tag, cụm tên file chung – và hỏi có chuyển luôn không. Dùng
 * được với sách không có metadata. Chuyển ở nền với hộp tiến trình, dừng được, hoàn tác được.
 */
async function offerSimilarFromOther(moved) {
  if (!moved?.length || S.ov.config.read_only) return;
  if (!moved.some((m) => /[\\/]kh[aá]c[\\/]/i.test(m.from) || fold(m.from).includes('\\khac\\') || fold(m.from).includes('/khac/'))) return;
  let sugs;
  try { sugs = await call('other_similar', { moved: moved.map((m) => ({ book_id: m.book_id, from: m.from })) }); } catch (e) { return fail(e); }
  for (const sg of sugs) {
    // Tick sẵn dấu hiệu mạnh (tác giả, bộ) và tag; cụm tên file để người dùng tự chọn.
    const checks = sg.criteria.map((c) => ({ c, box: h('input', { type: 'checkbox', checked: c.kind !== 'part' }) }));
    const remember = h('input', { type: 'checkbox' });
    const total = h('b');
    const recount = () => { total.textContent = checks.filter((x) => x.box.checked).reduce((n, x) => n + x.c.books, 0).toLocaleString('vi-VN'); };
    checks.forEach((x) => x.box.addEventListener('change', recount));
    recount();
    const hasTag = sg.criteria.some((c) => c.kind === 'tag');
    const ok = await modal({
      title: `Chuyển luôn sách giống vậy vào “${relLabel(sg.target)}”?`,
      ok: 'Chuyển', cancel: 'Không',
      content: h('div', { style: 'margin-bottom:16px' },
        h('p', { class: 'small' }, `Trong “Khác” còn ${sg.books.toLocaleString('vi-VN')} cuốn giống sách bạn vừa chuyển. Chọn dấu hiệu để chuyển các cuốn đó vào “${relLabel(sg.target)}” (tối đa `, total, ' cuốn):'),
        h('div', { class: 'block-tree', style: 'max-height:300px' }, ...checks.map(({ c, box }) => h('label', { class: 'block-row', title: c.samples.join('\n') }, box,
          h('span', { class: 'grow' }, h('span', { class: 'muted small' }, CRIT_KIND[c.kind] + ': '), c.label,
            c.samples.length ? h('div', { class: 'muted small' }, 'vd. ' + c.samples.slice(0, 3).join(' · ')) : null),
          h('span', { class: 'muted small' }, `${c.books.toLocaleString('vi-VN')} cuốn`)))),
        hasTag ? h('label', { class: 'check' }, remember, h('span', {}, 'Ghi nhớ tag: sách mới mang các tag đã chọn cũng vào thư mục này', h('div', { class: 'small muted' }, 'Lưu vào màn Tag → thể loại; đổi lúc nào cũng được.'))) : null),
    });
    if (!ok) continue;
    const criteria = checks.filter((x) => x.box.checked).map((x) => ({ kind: x.c.kind, key: x.c.key, label: x.c.label }));
    if (!criteria.length) continue;
    CL.kind = 'tagmove';
    CL.label = relLabel(sg.target);
    openCleanModal();
    try { await call('start_move_similar', { target: sg.target, criteria, remember: remember.checked }); } catch (e) { closeCleanModal(); fail(e); }
    return; // mỗi lần một việc nền; các gợi ý khác (hiếm) bỏ qua
  }
}

// ------------------------------------------------------------------ phân loại sách trong "Khác"
/**
 * Màn "Phân loại Khác": app học từ sách đã nằm trong các thư mục thể loại (tên sách, tag, tác giả – kể cả tác giả
 * đoán từ tên file) rồi đoán thể loại cho từng cuốn trong "Khác". Chọn thể loại + ngưỡng tin cậy → chuyển cả loạt ở
 * nền (hoàn tác được ở Hoạt động).
 */
async function renderOtherClassify() {
  if (!S.ov.library_ok) return renderWelcome();
  const O = S.otherCls || (S.otherCls = { min: S.ov.config.classify_medium || 45, picked: new Set(), excluded: new Set(), open: null });
  const minSel = h('select', { class: 'pill', 'aria-label': 'Ngưỡng tin cậy', onchange: (e) => { O.min = +e.target.value; O.picked.clear(); load(); } },
    ...[[S.ov.config.classify_high || 75, 'Tin cậy cao'], [S.ov.config.classify_medium || 45, 'Từ trung bình'], [30, 'Cả đoán yếu (≥30%)']].map(([v, l]) => h('option', { value: v, selected: O.min === v }, l)));
  toolbar('Phân loại Khác', 'đoán thể loại cho sách trong “Khác”', minSel);
  const host = h('div', { class: 'pad' }), bar = h('div', { class: 'sticky-bar' });
  let data = null;
  const load = async () => {
    host.replaceChildren(h('div', { class: 'muted' }, 'Đang học từ các thư mục thể loại và đoán sách trong “Khác”…'));
    bar.replaceChildren();
    try { data = await call('other_view', { minConf: O.min }); } catch (e) { return fail(e); }
    paint();
  };
  const chosen = () => data.groups.filter((g) => O.picked.has(g.target));
  const paintBar = () => {
    const n = chosen().reduce((s, g) => s + g.books, 0) - [...O.excluded].filter((id) => chosen().some((g) => g.samples.some((x) => x.book_id === id))).length;
    bar.replaceChildren(h('span', { class: 'grow' }, 'Sách ở lại ổ đang chứa nó (chỉ đổi tên). Chuyển xong vẫn hoàn tác được ở Hoạt động.'),
      h('button', { class: 'btn primary', disabled: n <= 0, onclick: apply }, `Chuyển ${Math.max(0, n).toLocaleString('vi-VN')} sách`));
  };
  const apply = async () => {
    const targets = chosen().map((g) => g.target);
    if (!(await modal({ title: 'Chuyển sách ra khỏi “Khác”?', body: `Vào: ${targets.join(', ')}. Chạy nền, dừng được giữa chừng.`, ok: 'Chuyển' }))) return;
    CL.kind = 'otherclassify';
    CL.label = targets.join(', ');
    openCleanModal();
    try { await call('start_other_classify', { targets, minConf: O.min, exclude: [...O.excluded] }); O.picked.clear(); } catch (e) { closeCleanModal(); fail(e); }
  };
  const paint = () => {
    if (!data.total) {
      host.replaceChildren(h('div', { class: 'empty' }, h('div', {}, h('h2', {}, '“Khác” đang trống'), h('p', {}, 'Không còn sách nào cần phân loại.'))));
      return;
    }
    const head = h('p', { class: 'small muted' }, `“Khác” có ${data.total.toLocaleString('vi-VN')} cuốn; app đoán được thể loại cho ${data.guessed.toLocaleString('vi-VN')} cuốn, học từ ${data.training.toLocaleString('vi-VN')} cuốn đã nằm trong thư mục thể loại. `
      + 'Càng nhiều sách được phân loại đúng, lần đoán sau càng tốt – chuyển xong một đợt rồi mở lại màn này.');
    const rows = data.groups.map((g) => {
      const box = h('input', { type: 'checkbox', checked: O.picked.has(g.target), 'aria-label': 'Chọn ' + g.target,
        onchange: (e) => { e.target.checked ? O.picked.add(g.target) : O.picked.delete(g.target); paintBar(); } });
      const open = O.open === g.target;
      const tr = h('tr', {},
        h('td', {}, box),
        h('td', {}, h('b', {}, g.target)),
        h('td', {}, g.books.toLocaleString('vi-VN')),
        h('td', {}, h('span', { class: 'tag ok' }, `${g.high.toLocaleString('vi-VN')} tin cậy cao`)),
        h('td', {}, h('button', { class: 'btn small', onclick: () => { O.open = open ? null : g.target; paint(); } }, open ? 'Ẩn' : 'Xem ví dụ')));
      if (!open) return [tr];
      const detail = h('tr', {}, h('td', {}), h('td', { colspan: 4 }, h('div', { class: 'small', style: 'max-height:320px;overflow:auto' },
        ...g.samples.map((x) => h('label', { class: 'block-row' },
          h('input', { type: 'checkbox', checked: !O.excluded.has(x.book_id), onchange: (e) => { e.target.checked ? O.excluded.delete(x.book_id) : O.excluded.add(x.book_id); paintBar(); } }),
          h('span', { class: 'grow' }, x.file_name, h('div', { class: 'muted' }, x.reason)), h('span', { class: 'muted' }, `${x.confidence}%`))),
        g.books > g.samples.length ? h('div', { class: 'muted', style: 'margin-top:6px' }, `… và ${(g.books - g.samples.length).toLocaleString('vi-VN')} cuốn khác (sắp theo độ tin cậy)`) : null)));
      return [tr, detail];
    }).flat();
    host.replaceChildren(head, data.groups.length
      ? h('table', { class: 'list' }, h('thead', {}, h('tr', {}, h('th', {}, ''), h('th', {}, 'Thể loại đoán'), h('th', {}, 'Số sách'), h('th', {}, ''), h('th', {}, ''))), h('tbody', {}, ...rows))
      : h('div', { class: 'empty' }, h('div', {}, h('h2', {}, 'Chưa đoán được cuốn nào'), h('p', {}, 'Hạ ngưỡng tin cậy, hoặc chuyển tay vài cuốn vào đúng thể loại để app học thêm.'))));
    paintBar();
  };
  view.replaceChildren(host, bar);
  await load();
}

// ------------------------------------------------------------------ thư mục thể loại (thêm / đổi tên / xoá)
async function addFolder(parent) {
  if (S.ov.config.read_only) return fail('Đang bật chế độ chỉ đọc – tắt trong Cài đặt để sửa thư mục.');
  const name = await askText({ title: parent ? `Thêm thư mục con trong “${relLabel(parent)}”` : 'Thêm thể loại', ok: 'Tạo',
    hint: parent ? 'Tạo thư mục thật trên ổ. Khi phân loại sách mới, app tự nhận ra thư mục con theo tag / tiêu đề.' : 'Tạo thư mục thể loại thật ở thư mục thư viện chính.' });
  if (!name) return;
  try { const rel = await call('folder_create', { parent, name }); toast(`Đã tạo “${relLabel(rel)}”.`); } catch (e) { return fail(e); }
  await refreshOverview().catch(fail);
  if (FM.open) openFolderManager(FM.scope);
}

async function renameFolder(node) {
  if (S.ov.config.read_only) return fail('Đang bật chế độ chỉ đọc – tắt trong Cài đặt để sửa thư mục.');
  const name = await askText({ title: `Đổi tên “${relLabel(node.rel)}”`, value: node.name, ok: 'Đổi tên',
    hint: 'Đổi tên thư mục thật trên mọi ổ có thư mục này (tức thì, không chép file). Đường dẫn sách, thể loại bị chặn và tag → thể loại được cập nhật theo.' });
  if (!name || name === node.name) return;
  try {
    const r = await call('folder_rename', { rel: node.rel, name });
    if (r.failures.length) fail(`Không đổi được tên ở ${r.failures.length} ổ: ${r.failures[0]}`);
    if (r.dirs.length) toast(`Đã đổi thành “${relLabel(r.rel)}” · ${r.books.toLocaleString('vi-VN')} sách cập nhật đường dẫn.`);
    if (S.view.scope === 'category' && node.depth === 1 && fold(S.view.category) === fold(node.name)) S.view = { ...S.view, category: r.rel };
  } catch (e) { return fail(e); }
  await refreshOverview().catch(fail);
  render();
  if (FM.open) openFolderManager(FM.scope && node.depth === 1 && fold(FM.scope) === fold(node.name) ? name : FM.scope);
}

async function deleteFolder(node) {
  if (S.ov.config.read_only) return fail('Đang bật chế độ chỉ đọc – tắt trong Cài đặt để sửa thư mục.');
  const top = node.depth === 1;
  const ok = await modal({
    title: `Xoá ${top ? 'thể loại' : 'thư mục'} “${relLabel(node.rel)}”?`,
    body: (node.books ? `${node.books.toLocaleString('vi-VN')} cuốn bên trong được chuyển ${top ? 'vào “Khác”' : 'lên thư mục cha'} trước (cùng ổ, hoàn tác được) – không xoá sách nào. ` : '')
      + 'Sau đó thư mục (trên mọi ổ) cùng file không phải sách còn lại vào Thùng rác Windows. Thể loại bị chặn / tag gán vào thư mục này cũng được bỏ.',
    ok: 'Xoá thư mục', danger: true });
  if (!ok) return;
  FM.open = false;
  CL.kind = 'folderdel';
  CL.label = relLabel(node.rel);
  openCleanModal();
  try { await call('start_folder_delete', { rel: node.rel }); } catch (e) { closeCleanModal(); fail(e); }
}

/** Hộp quản lý thư mục thể loại: cây (3 cấp) với đổi tên / thêm thư mục con / xoá; `scope` = một thể loại. */
const FM = { open: false, scope: null };
async function openFolderManager(scope) {
  let nodes;
  try { nodes = await call('category_tree'); } catch (e) { return fail(e); }
  if (scope) nodes = nodes.filter((n) => fold(n.rel.split('/')[0]) === fold(scope));
  nodes = nodes.filter((n) => n.exists && fold(n.rel.split('/')[0]) !== fold('Khác')); // hộp thư đến / Khác không sửa được
  $('#modal-root').querySelector('.folder-manager')?.closest('.modal')?.querySelector('.actions .btn')?.click(); // hộp cũ (đang làm mới)
  const list = h('div', { class: 'block-tree folder-manager' });
  const pick = (run) => () => { FM.open = true; $('#modal-root').lastElementChild?.querySelector('.actions .btn')?.click(); FM.open = true; run(); };
  for (const n of nodes) {
    list.append(h('div', { class: 'block-row' + (n.blocked ? ' is-blocked' : ''), style: `padding-left:${(n.depth - 1) * 20 + 4}px` },
      h('span', { class: 'grow', title: relLabel(n.rel) }, (n.depth > 1 ? '└ ' : '') + n.name),
      h('span', { class: 'muted small' }, `${n.books.toLocaleString('vi-VN')} cuốn`),
      h('button', { class: 'btn small', onclick: pick(() => renameFolder(n)) }, 'Đổi tên'),
      n.depth < 3 ? h('button', { class: 'btn small', title: 'Thêm thư mục con', onclick: pick(() => addFolder(n.rel)) }, '+ Con') : null,
      h('button', { class: 'btn small danger', onclick: pick(() => deleteFolder(n)) }, 'Xoá')));
  }
  if (!nodes.length) list.append(h('p', { class: 'muted', style: 'padding:8px' }, 'Chưa có thư mục thể loại nào.'));
  FM.open = false; FM.scope = scope || null;
  await modal({ title: scope ? `Thư mục trong “${scope}”` : 'Thư mục thể loại', ok: 'Đóng', cancel: null,
    content: h('div', {},
      h('p', { class: 'muted small' }, 'Sửa ở đây là sửa thư mục thật trên ổ cứng (mọi ổ thư viện có thư mục đó). Hộp thư đến và “Khác” không sửa được.'),
      scope ? h('button', { class: 'btn', style: 'margin-bottom:10px', onclick: pick(() => addFolder(scope)) }, '+ Thêm thư mục con')
        : h('button', { class: 'btn', style: 'margin-bottom:10px', onclick: pick(() => addFolder(null)) }, '+ Thêm thể loại'),
      list) });
}

// ------------------------------------------------------------------ ổ lưu sách (thư viện trên nhiều ổ)
async function addLibraryFolder() {
  try {
    const p = await call('add_library_folder');
    if (!p) return;
    toast(`Đã thêm “${p}” vào thư viện. Đang quét…`);
    await refreshOverview();
    render();
    startScan();
  } catch (e) { fail(e); }
}

async function removeLibraryFolder(path) {
  const ok = await modal({ title: `Bỏ “${path}” khỏi thư viện?`, body: 'File trên ổ giữ nguyên, app chỉ thôi quản lý thư mục này (sách trong đó không còn được phân loại / cân bằng). Thêm lại lúc nào cũng được.', ok: 'Bỏ khỏi thư viện' });
  if (!ok) return;
  try { await call('remove_library_folder', { path }); } catch (e) { return fail(e); }
  await refreshOverview().catch(fail);
  render();
}

/** Màn "Ổ lưu sách": dung lượng từng ổ thư viện, thêm / bỏ ổ, chuyển bớt sách từ ổ sắp đầy sang ổ còn chỗ. */
async function renderVolumes() {
  toolbar('Ổ lưu sách', 'thư viện trên nhiều ổ cứng', h('button', { class: 'btn', onclick: addLibraryFolder }, '+ Thêm thư mục ở ổ khác…'));
  let vols;
  try { vols = S.volumes = await call('volumes_info'); } catch (e) { return fail(e); }
  const minGb = S.ov.config.min_free_gb;
  const GB = 1024 ** 3;
  const host = h('div', { class: 'pad' });
  for (const v of vols) {
    const used = v.total_bytes ? 1 - v.free_bytes / v.total_bytes : 0;
    host.append(h('div', { class: 'vol' + (v.low ? ' low' : '') + (v.online ? '' : ' offline') },
      h('div', { class: 'vol-head' },
        h('span', { class: 'path grow' }, v.path), v.primary ? h('span', { class: 'tag' }, 'chính') : null,
        !v.online ? h('span', { class: 'tag bad' }, 'chưa kết nối') : v.low ? h('span', { class: 'tag warn' }, `còn dưới ${minGb} GB`) : null,
        !v.primary ? h('button', { class: 'btn small', onclick: () => removeLibraryFolder(v.path) }, 'Bỏ') : null),
      v.online ? h('div', { class: 'progress vol-bar', title: `Đã dùng ${Math.round(used * 100)}%` }, h('i', { class: 'det', style: `width:${Math.round(used * 100)}%` })) : null,
      h('div', { class: 'muted small' }, (v.online ? `Còn trống ${fmtBytes(v.free_bytes)} / ${fmtBytes(v.total_bytes)} · ` : '')
        + `${v.books.toLocaleString('vi-VN')} sách (${fmtBytes(v.book_bytes)})` + (v.inbox ? ` · ${v.inbox.toLocaleString('vi-VN')} chờ phân loại` : ''))));
  }
  host.append(h('div', { class: 'muted small', style: 'margin:14px 0' },
    'Cách app xếp sách để tiết kiệm ổ: sách luôn ở lại ổ đang chứa nó – chuyển trong cùng ổ chỉ là đổi tên, tức thì, không tốn thêm chỗ. ',
    'App không bao giờ tự chép sách qua ổ khác (chép hàng loạt giữa hai ổ chậm, dễ treo). Chỉ khi bạn bấm “Chuyển bớt” bên dưới, và mỗi file được chép an toàn: file tạm → so SHA-256 → đổi tên → mới xoá bản cũ. ',
    `Ổ còn dưới ${minGb} GB thì được đánh dấu: hãy tải sách mới vào hộp thư đến ở ổ còn chỗ, hoặc chuyển bớt. `,
    'Bản trùng giữa hai ổ hiện ở “Trùng chắc chắn” – dọn đi là lấy lại chỗ.'));
  const online = vols.filter((v) => v.online);
  if (online.length >= 2) {
    const fromSel = h('select', { class: 'pill', 'aria-label': 'Chuyển bớt từ ổ' }, ...online.map((v) => h('option', { value: v.path, selected: v.low }, v.path)));
    const gb = h('input', { type: 'number', min: 1, value: Math.max(1, Math.ceil(Math.max(0, ...online.filter((v) => v.low).map((v) => minGb - v.free_bytes / GB)))) || 10, style: 'width:90px', 'aria-label': 'Số GB' });
    host.append(h('div', { class: 'field' }, h('label', {}, 'Chuyển bớt sách sang ổ khác'),
      h('div', { style: 'display:flex;gap:8px;align-items:center;flex-wrap:wrap' }, 'Từ', fromSel, 'chuyển ít nhất', gb, 'GB',
        h('button', { class: 'btn primary', onclick: () => previewBalance(fromSel.value, +gb.value) }, 'Xem trước…')),
      h('div', { class: 'hint' }, 'Chuyển nguyên thư mục (bộ, tác giả…), thư mục lớn trước, sang ổ còn chỗ – giữ nguyên đường dẫn thể loại. Chép xong, kiểm tra đủ dung lượng mới xoá bản cũ; hoàn tác được.')));
  } else if (vols.length < 2) {
    host.append(h('div', { class: 'empty', style: 'min-height:200px' }, h('div', {}, h('h2', {}, 'Mới có một ổ'),
      h('p', {}, 'Sách nhiều quá một ổ cứng? Thêm một thư mục ở ổ khác (vd. G:\\sach) – app quét cả hai, gộp thể loại cùng tên, và tự xếp sách mới sang ổ còn chỗ.'))));
  }
  view.replaceChildren(host);
}

async function previewBalance(from, wantGb) {
  if (S.ov.config.read_only) return fail('Đang bật chế độ chỉ đọc – tắt trong Cài đặt để chuyển sách.');
  let plan;
  try { plan = await call('balance_preview', { from, wantGb }); } catch (e) { return fail(e); }
  if (!plan.groups.length) return modal({ title: 'Không có gì để chuyển', body: 'Không tìm được thư mục nào vừa ổ còn chỗ (mỗi ổ đích phải còn trên mức giữ trống sau khi nhận).', cancel: null, ok: 'Đóng' });
  const books = plan.groups.reduce((s, g) => s + g.books, 0);
  const list = h('div', { class: 'block-tree' }, ...plan.groups.slice(0, 500).map((g) => h('div', { class: 'block-row' },
    h('span', { class: 'grow', title: g.rel }, relLabel(g.rel)),
    h('span', { class: 'muted small', style: 'white-space:nowrap' }, `${g.books} sách · ${fmtBytes(g.bytes)} → ${g.to_root}`))));
  const ok = await modal({ title: `Chuyển ${fmtBytes(plan.bytes)} sang ổ khác?`, ok: `Chuyển ${books.toLocaleString('vi-VN')} sách`,
    content: h('div', {}, h('p', { class: 'small' }, `${plan.groups.length.toLocaleString('vi-VN')} thư mục, ${books.toLocaleString('vi-VN')} sách. Chạy nền, dừng được giữa chừng.`), list) });
  if (!ok) return;
  CL.kind = 'balance';
  openCleanModal();
  try { await call('start_balance', { from, wantGb }); } catch (e) { closeCleanModal(); fail(e); }
}

// ------------------------------------------------------------------ hoạt động (E5-05, E9-04, E8-04)
async function renderActivity() {
  toolbar('Hoạt động', 'nhật ký xoá / di chuyển / hoàn tác', h('button', { class: 'btn', onclick: () => call('open_recycle_bin').catch(fail) }, 'Mở Thùng rác'));
  let batches, reports;
  try { [batches, reports] = await Promise.all([call('list_batches'), call('report_history')]); } catch (e) { return fail(e); }
  const kinds = { dedupe: 'Dọn trùng', trash: 'Xoá', classify: 'Phân loại', manual: 'Chuyển tay', retitle: 'Sửa tiêu đề', metaedit: 'Sửa thông tin', block: 'Chặn thể loại', tagmap: 'Tag → thể loại', balance: 'Cân bằng ổ', folder_delete: 'Xoá thư mục', folder_rename: 'Đổi tên thư mục', flatten: 'Làm phẳng thư mục con', other_out: 'Chuyển ra khỏi Khác' };
  const host = h('div', { class: 'pad' });
  if (!batches.length) host.append(h('p', { class: 'muted' }, 'Chưa có thao tác nào.'));
  for (const b of batches) {
    const ops = h('div');
    let open = false;
    const canUndo = ['classify', 'manual', 'tagmap', 'balance', 'folder_delete', 'flatten', 'other_out'].includes(b.kind) && !b.undone_at && b.done;
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
    field('Thư mục thư viện', h('div', {},
      h('div', { style: 'display:flex;gap:8px;align-items:center;min-width:0' }, h('span', { class: 'path', style: 'flex:1' }, c.library_root || '(chưa chọn)'), h('button', { class: 'btn', onclick: chooseLibrary }, 'Đổi…')),
      ...(c.extra_libraries || []).map((p) => h('div', { style: 'display:flex;gap:8px;align-items:center;min-width:0;margin-top:6px' }, h('span', { class: 'path', style: 'flex:1' }, p), h('button', { class: 'btn', onclick: () => removeLibraryFolder(p) }, 'Bỏ'))),
      h('button', { class: 'btn', style: 'margin-top:6px', onclick: addLibraryFolder }, '+ Thêm thư mục ở ổ khác…')),
      'Sách nhiều quá một ổ: thêm thư mục ở ổ khác (vd. G:\\sach). Thể loại cùng tên trên các ổ được gộp làm một. Xem dung lượng và cân bằng ở “Ổ lưu sách”.'),
    field('Giữ trống mỗi ổ', h('input', { type: 'number', min: 0, max: 100000, value: c.min_free_gb, onchange: (e) => { c.min_free_gb = Math.max(0, +e.target.value || 0); } }),
      'GB. Ổ còn trống dưới mức này được cảnh báo ở “Ổ lưu sách” (app không tự chép sách qua ổ khác – bạn chọn chuyển bớt).'),
    field('Thư mục hộp thư đến', h('input', { type: 'text', value: c.inbox_name, onchange: (e) => { c.inbox_name = e.target.value.trim() || 'Chưa phân loại'; } }), 'Tên thư mục con trong thư viện chứa sách mới. Thư mục này không bao giờ là đích phân loại.'),
    field('Bỏ qua thư mục', h('input', { type: 'text', value: c.exclude_names.join('; '), onchange: (e) => { c.exclude_names = e.target.value.split(';').map((s) => s.trim()).filter(Boolean); } }), 'Phân cách bằng dấu ;'),
    field('Chế độ chỉ đọc', chk('read_only', 'Không xoá, không di chuyển file nào'), 'Bật khi thử trên kho thật để đối chiếu kết quả trước.'),
    field('Tự phân loại hộp thư đến', chk('auto_triage', 'Mỗi lần quét, tự xếp sách vào thể loại', (e) => {
      if (e.target.checked) modal({ title: 'Bật tự phân loại?', body: 'Mỗi lần quét, sách trong hộp thư đến (kể cả thư mục kiểu Calibre) sẽ được đọc tag, chuyển vào thư mục thể loại — tạo thư mục mới nếu cần, sách chưa chắc vào “Khác” — rồi mọi file không phải sách (ảnh, âm thanh, video, .opf…) và thư mục con thừa được đưa vào Thùng rác. Truyện tranh và file .zip/.rar được giữ nguyên. Nhật ký di chuyển: triage-journal.jsonl trong thư mục dữ liệu app.', ok: 'Đã hiểu', cancel: 'Tắt lại' })
        .then((ok) => { if (!ok) { c.auto_triage = false; e.target.checked = false; } });
    }), 'Mặc định bật. Xem trước bằng lệnh: librarian triage.'),
    field('Thư mục con', chk('flatten_subfolders', 'Mỗi lần quét, đưa sách trong thư mục con lạ ra thư mục thể loại'),
      'Mặc định bật (cần bật Tự phân loại). Chỉ giữ thư mục con bạn tạo bằng app (chuột phải lên thể loại → Thêm thư mục con; gõ đúng tên một thư mục có sẵn để giữ nó) và đích của Tag → thể loại. Thư mục con chép theo từ nguồn được làm phẳng: tên file chung chung như “Tập 3” thành “One Piece - Tập 3”, trùng tên thêm (1); thư mục đã hết sách vào Thùng rác. Lô “Làm phẳng thư mục con” hoàn tác được ở Hoạt động.'),
    field('Sách đã xoá', chk('auto_reject_deleted', 'Tải lại đúng cuốn đã xoá thì tự đưa vào Thùng rác'),
      'Mặc định bật. App nhớ dung lượng + mã băm của sách bạn đã xoá (kể cả sau khi dọn sạch Thùng rác Windows). Bản y hệt tải về lần sau bị loại khi quét; bản gần giống hiện ở “Nghi trùng” để bạn duyệt. Khôi phục từ Thùng rác thì app thôi nhớ cuốn đó.'),
    field('Thư mục thể loại', h('button', { class: 'btn', onclick: () => openFolderManager() }, 'Quản lý thư mục…'),
      'Thêm, đổi tên, xoá thể loại và thư mục con – sửa thẳng thư mục thật trên ổ. Cũng mở được bằng chuột phải lên thể loại ở thanh bên.'),
    field('Thể loại bị chặn', h('div', {},
      ...(c.blocked_categories.length ? c.blocked_categories.map((rel) => h('div', { class: 'block-row' },
        h('span', { class: 'grow' }, relLabel(rel)), h('button', { class: 'btn small', onclick: () => unblockFolder(rel) }, 'Bỏ chặn')))
        : [h('div', { class: 'muted small' }, 'Chưa chặn thể loại nào.')]),
      h('button', { class: 'btn', style: 'margin-top:6px', onclick: () => openBlockPicker() }, 'Chặn thư mục…')),
      'Sách trong thư mục bị chặn vào Thùng rác; sách mới tự phân loại vào đó cũng vào thẳng Thùng rác, thư mục không được tạo lại. Chặn được cả thư mục con bạn tự chia (vd. “Ngôn tình - Đam mỹ › Đam mỹ”). Cũng mở được bằng chuột phải lên thể loại ở thanh bên.'),
    field('Phân loại: ngưỡng tin cậy cao', num('classify_high', 1, 100), 'Chỉ dùng khi đoán thể loại (màn Phân loại, Phân loại Khác): đề xuất từ mức này trở lên được tick sẵn.'),
    field('Phân loại: ngưỡng trung bình', num('classify_medium', 1, 100), 'Đoán thể loại dưới mức này: đưa vào “Cần làm tay”.'),
    field('Nghi trùng: ngưỡng tự dọn', num('suspect_auto', 50, 100), 'Nhóm nghi trùng tin cậy từ mức này trở lên (%) được tự dọn – giữ bản đề xuất, bản còn lại vào Thùng rác; dưới mức này chờ bạn duyệt. Mặc định 85, thấp nhất 50.'),
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
      if (!(c.suspect_auto >= 50 && c.suspect_auto <= 100)) return fail('Ngưỡng tự dọn nghi trùng phải từ 50 đến 100');
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
