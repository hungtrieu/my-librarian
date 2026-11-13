// Trình đọc trong app kiểu Apple Books (M5, E7).
// Khung chung: toàn màn hình, thanh trên/dưới tự ẩn (chạm giữa / di chuột lên mép để hiện), mục lục,
// tìm trong sách, bảng "Aa", nhớ vị trí đọc. Bốn bộ máy hiển thị:
//   foliate – EPUB/MOBI/AZW3/FB2 (foliate-js)      comic – CBZ/CBR, từng trang do Rust phục vụ
//   pdf     – pdf.js, tải theo HTTP Range           text  – TXT
import { h, fold, debounce } from './ui-kit.js';

const FOLIATE = './vendor/foliate-js/';
const PDFJS = new URL('./vendor/foliate-js/vendor/pdfjs/', import.meta.url).href;

// ------------------------------------------------------------------ tuỳ chọn đọc (nhớ trên máy, theo từng cuốn)
// Mỗi cuốn nhớ riêng phông, cỡ chữ, màu nền, sáng/tối, lề… vì mỗi bản dàn chữ to nhỏ khác nhau.
// `readerPrefs` giữ bộ tuỳ chọn dùng gần nhất: cuốn chưa chỉnh lần nào (và sách ngoài thư viện) mở theo bộ này.
// `readerPrefsByBook` = { [id sách]: { p: tuỳ chọn, t: lần chỉnh gần nhất } }, chỉ giữ PER_BOOK_MAX cuốn mới nhất.
const PREF_KEY = 'readerPrefs';
const BOOK_PREF_KEY = 'readerPrefsByBook';
const PER_BOOK_MAX = 300;
const DEFAULTS = {
  theme: 'auto', fontScale: 100, font: 'publisher', lineHeight: 1.5, margin: 'normal', justify: true, flow: 'paginated', phoneFlow: 'scrolled',
  comicLayout: 'auto', comicFit: 'page', comicDir: 'ltr', comicScroll: false, pdfFlow: 'scrolled', pdfZoom: 1,
};
const readJson = (k) => { try { const v = JSON.parse(localStorage.getItem(k) || '{}'); return v && typeof v === 'object' ? v : {}; } catch { return {}; } };
const writeJson = (k, v) => { try { localStorage.setItem(k, JSON.stringify(v)); } catch { /* bỏ qua */ } };
const loadPrefs = (bookId) => ({ ...DEFAULTS, ...readJson(PREF_KEY), ...(bookId >= 0 ? readJson(BOOK_PREF_KEY)[bookId]?.p : null) });
const savePrefs = (p, bookId) => {
  writeJson(PREF_KEY, p);
  if (!(bookId >= 0)) return;
  const all = readJson(BOOK_PREF_KEY);
  all[bookId] = { p, t: Date.now() };
  const ids = Object.keys(all);
  if (ids.length > PER_BOOK_MAX) ids.sort((a, b) => (all[b].t || 0) - (all[a].t || 0)).slice(PER_BOOK_MAX).forEach((id) => delete all[id]);
  writeJson(BOOK_PREF_KEY, all);
};

const THEMES = {
  white: { label: 'Trắng', bg: '#ffffff', fg: '#1d1d1f', ui: 'light' },
  sepia: { label: 'Sepia', bg: '#f8f1e3', fg: '#4f3f2f', ui: 'light' },
  gray: { label: 'Xám', bg: '#4b4b4f', fg: '#ececee', ui: 'dark' },
  night: { label: 'Đêm', bg: '#121212', fg: '#c8c8cb', ui: 'dark' },
};
const FONTS = {
  publisher: { label: 'Gốc', css: null },
  serif: { label: 'Serif', css: 'Charter, "Iowan Old Style", Georgia, "Times New Roman", serif' },
  sans: { label: 'Sans', css: '"Segoe UI Variable Text", "Segoe UI", system-ui, sans-serif' },
};
const MARGINS = { narrow: '4%', normal: '7%', wide: '12%' };

const reduceMotion = () => matchMedia('(prefers-reduced-motion: reduce)').matches;
const appDark = () => (document.documentElement.dataset.theme || (matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light')) === 'dark';
const themeOf = (p) => THEMES[p.theme] || (appDark() ? THEMES.night : THEMES.white);
const pct = (f) => Math.round((f || 0) * 100) + '%';
const clamp = (x, a, b) => Math.max(a, Math.min(b, x));
const parsePage = (loc) => { const m = /^page:(\d+)$/.exec(loc || ''); return m ? +m[1] : null; };

// Biểu tượng vẽ bằng SVG (ký tự ⌕ hiển thị quá nhỏ trên iPad). Cỡ theo font-size của nút: 1.15em.
const ICONS = {
  search: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round"><circle cx="10.5" cy="10.5" r="6.5"/><path d="M15.5 15.5 21 21"/></svg>',
  bookmark: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linejoin="round"><path d="M6 3.5h12v17l-6-4.5-6 4.5z"/></svg>',
  list: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"><path d="M8 6h13M8 12h13M8 18h13"/><circle cx="3.5" cy="6" r="1" fill="currentColor"/><circle cx="3.5" cy="12" r="1" fill="currentColor"/><circle cx="3.5" cy="18" r="1" fill="currentColor"/></svg>',
};
const icon = (name) => { const t = document.createElement('template'); t.innerHTML = ICONS[name]; const el = t.content.firstChild; el.classList.add('r-ic'); el.setAttribute('aria-hidden', 'true'); return el; };

// ------------------------------------------------------------------ điểm vào
let current = null;
export const readerOpen = () => !!current;

/**
 * Mở sách trong trình đọc. `ctx`: { call, toast, fail, modal, bookBase(book) → URL gốc của book://,
 * onClose(changed), onDelete(book) → Promise<bool>,
 * bookmarks?: bool (có lệnh bookmark_list/add/remove), mobile?: bool (iPad/iPhone: bỏ mục chỉ có trên máy tính),
 * leftHand?: bool (iPhone: nhóm nút nhanh dồn về góc dưới trái),
 * phone?: bool (điện thoại: kiểu đọc lưu riêng – mặc định cuộn liền mạch như Apple Books) }.
 */
export async function openReader(id, ctx) {
  if (current) await current.close(false);
  let info;
  try { info = await ctx.call('reader_info', { id }); } catch (e) { ctx.fail(e); return; }
  if (info.engine === 'external') {
    try { await ctx.call('open_external', { id }); ctx.toast(`${info.book.format.toUpperCase()} chưa đọc được trong app – đã mở bằng ứng dụng mặc định.`); }
    catch (e) { ctx.fail(e); }
    return;
  }
  current = new Reader(info, ctx);
  await current.open();
}

// ------------------------------------------------------------------ khung trình đọc
class Reader {
  constructor(info, ctx) {
    this.info = info; this.book = info.book; this.ctx = ctx;
    this.prefs = loadPrefs(this.book.id);
    this.base = ctx.bookBase(this.book);
    this.loc = { fraction: info.progress || 0, locator: info.locator };
    this.changed = false;
    this.chrome = true;
    this.save = debounce(() => this.flush(), 1200);
  }

  async open() {
    const b = this.book;
    this.stage = h('div', { class: 'r-stage', 'aria-label': 'Nội dung sách' });
    this.titleEl = h('div', { class: 'r-title' }, h('b', {}, b.title),
      h('span', {}, b.id < 0 ? 'Ngoài thư viện – không lưu vị trí đọc' : b.authors || ''));
    this.slider = h('input', { type: 'range', min: 0, max: 1, step: 0.0001, value: 0, 'aria-label': 'Vị trí trong sách', class: 'r-slider' });
    this.posLeft = h('span', { class: 'r-pos' });
    this.posRight = h('span', { class: 'r-pos right' });
    this.nextVolBtn = h('button', { class: 'r-btn pill', hidden: true, onclick: () => this.openVolume(this.info.next) });
    this.preview = h('div', { class: 'r-preview', hidden: true });
    const btn = (label, title, onclick, extra = {}) => h('button', { class: 'r-btn', title, 'aria-label': title, onclick, ...extra }, label);
    this.marks = [];
    this.tocTab = 'toc';
    this.tocBtn = btn(this.ctx.bookmarks ? icon('list') : '☰', this.ctx.bookmarks ? 'Mục lục và dấu trang (T)' : 'Mục lục (T)', () => this.togglePanel('toc'));
    this.searchBtn = btn(icon('search'), 'Tìm trong sách (Ctrl+F)', () => this.togglePanel('search'), { class: 'r-btn r-search-btn' });
    this.markBtn = this.ctx.bookmarks && b.id >= 0 ? btn(icon('bookmark'), 'Đánh dấu trang này', () => this.toggleMark(), { class: 'r-btn r-mark-btn', 'aria-pressed': 'false' }) : null;
    const backBtn = btn('‹ Thư viện', 'Quay lại thư viện (Esc)', () => this.close(), { class: 'r-btn text' });
    const styleBtn = btn('Aa', 'Giao diện đọc', () => this.togglePanel('style'));
    const moreBtn = btn('⋯', 'Thêm', (e) => this.moreMenu(e));
    this.top = h('header', { class: 'r-top' + (this.ctx.leftHand ? ' left-hand' : '') },
      // Cầm máy tay trái: đánh dấu trang, mục lục, tìm, Aa ở góc trái; quay lại và "Thêm" sang phải.
      this.ctx.leftHand ? h('div', { class: 'r-group' }, this.markBtn, this.tocBtn, this.searchBtn, styleBtn)
        : h('div', { class: 'r-group' }, backBtn, this.tocBtn),
      this.titleEl,
      this.ctx.leftHand ? h('div', { class: 'r-group' }, moreBtn, backBtn)
        : h('div', { class: 'r-group' }, this.markBtn, this.searchBtn, styleBtn, moreBtn));
    this.bottom = h('footer', { class: 'r-bottom' }, this.preview,
      h('div', { class: 'r-row' }, this.posLeft, this.nextVolBtn, this.posRight), this.slider);
    this.panel = h('aside', { class: 'r-panel', hidden: true, role: 'dialog' });
    this.pop = h('div', { class: 'r-pop', hidden: true, role: 'dialog', 'aria-label': 'Giao diện đọc' });
    this.endCard = h('div', { class: 'r-end', hidden: true });
    this.spinner = h('div', { class: 'r-loading' }, h('div', { class: 'r-spin' }), h('div', {}, 'Đang mở sách…'));
    this.root = h('div', { class: 'reader', role: 'application', 'aria-label': 'Trình đọc: ' + b.title },
      this.stage, this.spinner, this.top, this.bottom, this.panel, this.pop, this.endCard);
    document.body.append(this.root);
    document.body.classList.add('reading');
    this.applyTheme();

    this.onKey = (e) => this.key(e);
    document.addEventListener('keydown', this.onKey, true);
    this.onMove = (e) => this.mouse(e);
    this.root.addEventListener('mousemove', this.onMove);
    this.onUnload = () => this.flush();
    addEventListener('beforeunload', this.onUnload);
    this.slider.addEventListener('input', () => { this.engine?.seek(+this.slider.value, false); this.showPreview(); });
    this.slider.addEventListener('change', () => { this.engine?.seek(+this.slider.value, true); this.preview.hidden = true; });
    this.slider.addEventListener('mousemove', (e) => this.hoverPreview(e));
    this.slider.addEventListener('mouseleave', () => { if (!this.slider.matches(':active')) this.preview.hidden = true; });
    this.media = matchMedia('(prefers-color-scheme: dark)');
    this.onScheme = () => { if (this.prefs.theme === 'auto') this.applyPrefs(false); };
    this.media.addEventListener('change', this.onScheme);

    const Engine = { foliate: FoliateEngine, comic: ComicEngine, pdf: PdfEngine, text: TextEngine }[this.info.engine];
    this.engine = new Engine(this);
    try {
      await this.engine.open(this.info.locator, this.info.progress);
    } catch (e) {
      console.error(e);
      this.spinner.replaceChildren(h('div', { class: 'r-error' }, h('h3', {}, 'Không mở được sách này'), h('p', {}, String(e?.message || e)),
        h('div', { class: 'r-actions' }, h('button', { class: 'btn', onclick: () => this.openExternal() }, 'Mở bằng ứng dụng mặc định'),
          h('button', { class: 'btn primary', onclick: () => this.close() }, 'Quay lại thư viện'))));
      this.setChrome(true);
      return;
    }
    this.spinner.remove();
    if (this.markBtn) {
      try { this.marks = await this.ctx.call('bookmark_list', { id: b.id }); } catch (e) { console.warn('bookmark_list', e); }
      this.updateMark();
    }
    if (!this.engine.hasToc?.() && !this.markBtn) this.tocBtn.hidden = true;
    if (!this.engine.search) this.searchBtn.hidden = true;
    this.updateNextVol();
    this.setChrome(true);
    this.autoHide(2500);
  }

  // ---------------------------------------------------------------- vị trí
  relocate({ fraction, locator, left, right, atEnd }) {
    this.loc = { fraction: clamp(fraction || 0, 0, 1), locator };
    if (!this.slider.matches(':active')) this.slider.value = this.engine.sliderValue?.() ?? this.loc.fraction;
    this.posLeft.textContent = left || '';
    this.posRight.textContent = right ?? pct(this.loc.fraction);
    this.atEnd = !!atEnd;
    this.updateNextVol();
    this.updateMark();
    if (!atEnd) this.endCard.hidden = true;
    this.changed = true;
    this.save();
  }

  async flush() {
    if (!this.changed) return;
    this.changed = false;
    try {
      const status = await this.ctx.call('save_position', { id: this.book.id, locator: this.loc.locator ?? null, progress: this.loc.fraction });
      this.book.read_status = status;
    } catch (e) { console.warn('save_position', e); }
  }

  // ---------------------------------------------------------------- đánh dấu trang
  /** Các dấu trang nằm trong trang đang hiện. */
  marksHere() {
    return this.marks.filter((m) => {
      if (this.engine.onPage && /^epubcfi\(/.test(m.locator || '')) return this.engine.onPage(m.locator);
      if (parsePage(m.locator) != null) return m.locator === this.loc.locator;
      return Math.abs(m.progress - this.loc.fraction) < 0.002;
    });
  }
  updateMark() {
    if (!this.markBtn) return;
    const on = this.marksHere().length > 0;
    this.markBtn.classList.toggle('on', on);
    this.markBtn.setAttribute('aria-pressed', on ? 'true' : 'false');
    this.markBtn.title = this.markBtn.ariaLabel = on ? 'Bỏ đánh dấu trang này' : 'Đánh dấu trang này';
  }
  async toggleMark() {
    const here = this.marksHere();
    try {
      if (here.length) {
        for (const m of here) await this.ctx.call('bookmark_remove', { bookmark: m.id });
        this.marks = this.marks.filter((m) => !here.includes(m));
        this.ctx.toast('Đã bỏ đánh dấu trang.');
      } else {
        const label = this.posLeft.textContent || pct(this.loc.fraction);
        const m = await this.ctx.call('bookmark_add', { id: this.book.id, locator: this.loc.locator ?? null, progress: this.loc.fraction, label });
        this.marks = [...this.marks, m].sort((a, b) => a.progress - b.progress);
        this.ctx.toast('Đã đánh dấu trang.');
      }
    } catch (e) { this.ctx.fail(e); }
    this.updateMark();
    if (!this.panel.hidden && this.panel.dataset.kind === 'toc') this.renderToc();
    this.autoHide(2500);
  }
  goToMark(m) {
    if (/^epubcfi\(/.test(m.locator || '')) this.engine.goTo(m.locator);
    else this.engine.seek(m.progress, true);
    this.closeOverlays();
  }

  updateNextVol() {
    const n = this.info.next;
    this.nextVolBtn.hidden = !(n && this.atEnd);
    if (n) this.nextVolBtn.textContent = `Vol tiếp: ${n.title} ›`;
  }

  /** Bấm "sau" khi đã ở trang cuối: hiện thẻ kết thúc (gợi ý vol kế tiếp – E7-15). */
  showEnd() {
    const n = this.info.next, p = this.info.series_pos;
    this.endCard.replaceChildren(h('div', { class: 'r-end-box' },
      h('div', { class: 'r-end-mark' }, '✓'),
      h('h3', {}, 'Đã đọc hết'),
      h('p', {}, this.book.title + (p ? ` · vol ${p[0]}/${p[1]}` : '')),
      h('div', { class: 'r-actions' },
        n ? h('button', { class: 'btn primary', onclick: () => this.openVolume(n) }, `Đọc tiếp: ${n.title}`) : null,
        h('button', { class: 'btn', onclick: () => this.close() }, 'Về thư viện'),
        h('button', { class: 'btn link', onclick: () => { this.endCard.hidden = true; } }, 'Ở lại'))));
    this.endCard.hidden = false;
    this.endCard.querySelector('.btn')?.focus();
  }

  next() {
    if (!this.endCard.hidden && this.info.next) return this.openVolume(this.info.next);
    if (this.atEnd) return this.showEnd();
    return this.engine.next();
  }
  prev() { this.endCard.hidden = true; return this.engine.prev(); }

  async openVolume(v) {
    if (!v) return;
    const ctx = this.ctx;
    await this.close(false);
    await openReader(v.id, ctx);
  }

  // ---------------------------------------------------------------- thanh điều khiển tự ẩn
  setChrome(on) {
    this.chrome = on;
    this.root.classList.toggle('chrome-hidden', !on);
    if (!on) { this.preview.hidden = true; }
  }
  autoHide(ms = 2200) {
    clearTimeout(this.hideT);
    this.hideT = setTimeout(() => {
      if (!this.panel.hidden || !this.pop.hidden || this.root.matches(':hover .r-top, :hover .r-bottom') || this.slider.matches(':active')) return;
      this.setChrome(false);
    }, ms);
  }
  toggleChrome() { this.setChrome(!this.chrome); if (this.chrome) this.autoHide(3500); }
  mouse(e) {
    const y = e.clientY, H = innerHeight;
    if (y < 70 || y > H - 90) { if (!this.chrome) this.setChrome(true); clearTimeout(this.hideT); }
    else if (this.chrome && this.panel.hidden && this.pop.hidden) this.autoHide(1500);
  }
  /** Chạm/bấm trong trang: mép trái/phải lật trang, giữa hiện/ẩn thanh điều khiển. */
  tap(xFrac) {
    if (!this.pop.hidden || !this.panel.hidden) { this.closeOverlays(); return; }
    if (this.engine.paged?.() !== false && xFrac < 0.28) { this.engine.dir?.() === 'rtl' ? this.next() : this.prev(); return; }
    if (this.engine.paged?.() !== false && xFrac > 0.72) { this.engine.dir?.() === 'rtl' ? this.prev() : this.next(); return; }
    this.toggleChrome();
  }
  closeOverlays() { this.panel.hidden = true; this.pop.hidden = true; this.root.classList.remove('panel-open'); this.autoHide(); }

  // ---------------------------------------------------------------- phím tắt
  key(e) {
    if (!this.root.isConnected) return;
    if (document.querySelector('#modal-root')?.children.length) return;
    const typing = /INPUT|SELECT|TEXTAREA/.test(e.target.tagName) && e.target.type !== 'range';
    const k = e.key, ctrl = e.ctrlKey || e.metaKey;
    const stop = () => { e.preventDefault(); e.stopPropagation(); };
    if (k === 'Escape') {
      stop();
      if (!this.endCard.hidden) this.endCard.hidden = true;
      else if (!this.pop.hidden || !this.panel.hidden) this.closeOverlays();
      else if (typing) e.target.blur();
      else this.close();
      return;
    }
    if (k === 'F11') { stop(); this.fullscreen(); return; }
    if (ctrl && k.toLowerCase() === 'f') { stop(); if (this.engine.search) this.togglePanel('search', true); return; }
    if (ctrl && (k === '=' || k === '+')) { stop(); this.zoom(+1); return; }
    if (ctrl && k === '-') { stop(); this.zoom(-1); return; }
    if (ctrl && k === '0') { stop(); this.zoom(0); return; }
    // Không chặn phím tới ô nhập/vùng cuộn: bộ bắt phím của app tự bỏ qua khi trình đọc đang mở.
    if (typing) return;
    if (this.engine.ownsKey?.(e)) return;
    const rtl = this.engine.dir?.() === 'rtl';
    if (k === 'ArrowRight') { stop(); rtl ? this.prev() : this.next(); }
    else if (k === 'ArrowLeft') { stop(); rtl ? this.next() : this.prev(); }
    else if (k === 'PageDown' || (k === ' ' && !e.shiftKey) || k === 'ArrowDown' && this.engine.paged?.() !== false) { stop(); this.next(); }
    else if (k === 'PageUp' || (k === ' ' && e.shiftKey) || k === 'ArrowUp' && this.engine.paged?.() !== false) { stop(); this.prev(); }
    else if (k === 'Home') { stop(); this.engine.seek(+this.slider.min, true); }
    else if (k === 'End') { stop(); this.engine.seek(+this.slider.max, true); }
    else if (k.toLowerCase() === 't' && this.engine.hasToc?.()) { stop(); this.togglePanel('toc'); }
    else if (k === 'Delete') { stop(); this.deleteBook(); }
  }

  zoom(d) {
    const p = this.prefs;
    if (this.info.engine === 'pdf') p.pdfZoom = d === 0 ? 1 : clamp(+(p.pdfZoom * (d > 0 ? 1.15 : 1 / 1.15)).toFixed(3), 0.4, 5);
    else if (this.info.engine === 'comic') { p.comicFit = d > 0 ? 'width' : 'page'; }
    else p.fontScale = d === 0 ? 100 : clamp(p.fontScale + d * 10, 60, 250);
    this.applyPrefs();
  }

  async fullscreen() {
    try {
      if (window.__TAURI__) await this.ctx.call('toggle_fullscreen');
      else if (document.fullscreenElement) await document.exitFullscreen();
      else await this.root.requestFullscreen();
    } catch (e) { console.warn(e); }
  }

  // ---------------------------------------------------------------- giao diện
  applyTheme() {
    const t = themeOf(this.prefs);
    this.root.style.setProperty('--r-bg', t.bg);
    this.root.style.setProperty('--r-fg', t.fg);
    this.root.dataset.ui = t.ui;
    this.root.dataset.theme = this.prefs.theme === 'auto' ? (appDark() ? 'night' : 'white') : this.prefs.theme;
    this.root.classList.toggle('no-motion', reduceMotion());
  }
  /** Áp tuỳ chọn; `persist` = người đọc vừa chỉnh → ghi lại cho riêng cuốn này. */
  applyPrefs(persist = true) {
    if (persist) savePrefs(this.prefs, this.book.id);
    this.applyTheme();
    this.engine?.apply(this.prefs);
    if (!this.pop.hidden) this.renderStyle();
  }

  togglePanel(which, force) {
    if (which === 'style') {
      this.panel.hidden = true; this.root.classList.remove('panel-open');
      this.pop.hidden = force != null ? !force : !this.pop.hidden;
      if (!this.pop.hidden) { this.renderStyle(); this.setChrome(true); clearTimeout(this.hideT); }
      return;
    }
    this.pop.hidden = true;
    const open = force ?? (this.panel.hidden || this.panel.dataset.kind !== which);
    this.panel.hidden = !open;
    this.root.classList.toggle('panel-open', open);
    if (!open) return this.autoHide();
    this.panel.dataset.kind = which;
    this.setChrome(true); clearTimeout(this.hideT);
    which === 'toc' ? this.renderToc() : this.renderSearch();
  }

  renderToc() {
    if (this.markBtn && (this.tocTab === 'marks' || !this.engine.hasToc?.())) return this.renderMarks();
    const items = this.engine.toc?.() || [];
    const curHref = this.engine.currentTocHref?.();
    const list = (arr, depth) => h('ol', { class: 'r-toc' + (depth ? ' sub' : '') }, ...arr.map((it) => h('li', {},
      h('button', { class: 'r-toc-item' + (it.href === curHref ? ' current' : ''), 'aria-current': it.href === curHref ? 'true' : null,
        onclick: () => { this.engine.goTo(it.href); this.closeOverlays(); } }, h('span', {}, it.label || '—'), it.page != null ? h('span', { class: 'muted' }, String(it.page)) : null),
      it.subitems?.length ? list(it.subitems, depth + 1) : null)));
    this.panel.replaceChildren(this.tocHead('Mục lục'),
      h('div', { class: 'r-panel-body' }, items.length ? list(items, 0) : h('p', { class: 'muted pad-s' }, 'Sách này không có mục lục.')));
    this.panel.querySelector('.current')?.scrollIntoView({ block: 'center' });
  }

  /** Đầu ngăn mục lục: có dấu trang thì thêm hai thẻ "Mục lục | Dấu trang". */
  tocHead(title) {
    const close = h('button', { class: 'r-btn', 'aria-label': 'Đóng', onclick: () => this.closeOverlays() }, '✕');
    if (!this.markBtn || !this.engine.hasToc?.()) return h('div', { class: 'r-panel-head' }, h('b', {}, title), close);
    const tab = (k, label) => h('button', { role: 'radio', 'aria-checked': this.tocTab === k ? 'true' : 'false', onclick: () => { this.tocTab = k; this.renderToc(); } }, label);
    return h('div', { class: 'r-panel-head' }, h('div', { class: 'r-seg r-tabs', role: 'radiogroup' }, tab('toc', 'Mục lục'), tab('marks', `Dấu trang (${this.marks.length})`)), close);
  }

  renderMarks() {
    const here = new Set(this.marksHere());
    const del = (m) => h('button', { class: 'r-btn r-mark-del', 'aria-label': 'Xoá dấu trang', onclick: async (e) => {
      e.stopPropagation();
      try { await this.ctx.call('bookmark_remove', { bookmark: m.id }); this.marks = this.marks.filter((x) => x !== m); } catch (err) { this.ctx.fail(err); }
      this.updateMark(); this.renderToc();
    } }, '✕');
    const items = this.marks.map((m) => h('li', { class: 'r-mark-row' },
      h('button', { class: 'r-toc-item' + (here.has(m) ? ' current' : ''), onclick: () => this.goToMark(m) },
        h('span', {}, m.label || '—'), h('span', { class: 'muted' }, pct(m.progress))), del(m)));
    this.panel.replaceChildren(this.tocHead('Dấu trang'),
      h('div', { class: 'r-panel-body' }, items.length ? h('ol', { class: 'r-toc' }, ...items)
        : h('p', { class: 'muted pad-s' }, 'Chưa có dấu trang. Bấm biểu tượng dấu trang ở thanh dưới để đánh dấu trang đang đọc.')));
  }

  renderSearch() {
    const input = h('input', { type: 'search', placeholder: 'Tìm trong sách (có/không dấu)…', value: this.lastQuery || '', 'aria-label': 'Tìm trong sách' });
    const status = h('div', { class: 'muted small pad-s' });
    const results = h('div', { class: 'r-results' });
    const run = async () => {
      const q = input.value.trim();
      this.lastQuery = q;
      const token = (this.searchToken = {});
      results.replaceChildren();
      this.engine.clearSearch?.();
      if (q.length < 2) { status.textContent = q ? 'Gõ ít nhất 2 ký tự.' : ''; return; }
      status.textContent = 'Đang tìm…';
      let n = 0;
      try {
        for await (const r of this.engine.search(q)) {
          if (token !== this.searchToken) return;
          if (r.progress != null) { status.textContent = `Đang tìm… ${pct(r.progress)}`; continue; }
          results.append(h('div', { class: 'r-res-group' }, r.label || ''));
          for (const it of r.items) {
            n++;
            results.append(h('button', { class: 'r-res', onclick: () => this.engine.goTo(it.target, true) },
              h('span', {}, it.pre), h('mark', {}, it.match), h('span', {}, it.post)));
          }
          if (n >= 500) break;
        }
        if (token === this.searchToken) status.textContent = n ? `${n >= 500 ? '500+' : n} kết quả` : 'Không tìm thấy.';
      } catch (e) { status.textContent = 'Lỗi tìm: ' + (e?.message || e); }
    };
    input.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); run(); } });
    input.addEventListener('input', debounce(run, 450));
    this.panel.replaceChildren(h('div', { class: 'r-panel-head' }, h('b', {}, 'Tìm trong sách'), h('button', { class: 'r-btn', 'aria-label': 'Đóng', onclick: () => { this.engine.clearSearch?.(); this.closeOverlays(); } }, '✕')),
      h('div', { class: 'r-search' }, input), status, h('div', { class: 'r-panel-body' }, results));
    setTimeout(() => input.focus(), 0);
    if (this.lastQuery) run();
  }

  renderStyle() {
    const p = this.prefs, eng = this.info.engine;
    const seg = (key, opts) => h('div', { class: 'r-seg', role: 'radiogroup' }, ...opts.map(([v, label, title]) =>
      h('button', { role: 'radio', 'aria-checked': String(p[key] === v), title: title || label, onclick: () => { p[key] = v; this.applyPrefs(); } }, label)));
    const themes = h('div', { class: 'r-themes', role: 'radiogroup', 'aria-label': 'Màu nền' },
      ...[['auto', 'Tự động', null], ...Object.entries(THEMES).map(([k, t]) => [k, t.label, t])].map(([k, label, t]) =>
        h('button', { role: 'radio', 'aria-checked': String(p.theme === k), title: label, 'aria-label': label, class: 'r-swatch ' + k,
          style: t ? `background:${t.bg};color:${t.fg}` : null, onclick: () => { p.theme = k; this.applyPrefs(); } }, 'Aa')));
    const rows = [h('div', { class: 'r-pop-row' }, themes)];
    if (eng === 'foliate' || eng === 'text') {
      rows.push(h('div', { class: 'r-pop-row r-size' },
        h('button', { class: 'r-btn big', 'aria-label': 'Chữ nhỏ hơn', onclick: () => this.zoom(-1) }, h('span', { style: 'font-size:13px' }, 'A')),
        h('span', { class: 'r-size-val' }, p.fontScale + '%'),
        h('button', { class: 'r-btn big', 'aria-label': 'Chữ lớn hơn', onclick: () => this.zoom(+1) }, h('span', { style: 'font-size:21px' }, 'A'))),
      h('div', { class: 'r-pop-row' }, h('label', {}, 'Phông'), seg('font', Object.entries(FONTS).map(([k, f]) => [k, f.label]))),
      h('div', { class: 'r-pop-row' }, h('label', {}, 'Giãn dòng'), seg('lineHeight', [[1.3, 'Hẹp'], [1.5, 'Vừa'], [1.8, 'Rộng']])),
      h('div', { class: 'r-pop-row' }, h('label', {}, 'Lề'), seg('margin', [['narrow', 'Hẹp'], ['normal', 'Vừa'], ['wide', 'Rộng']])),
      h('div', { class: 'r-pop-row' }, h('label', {}, 'Căn đều'), seg('justify', [[true, 'Bật'], [false, 'Tắt']])));
      if (eng === 'foliate' && !this.engine.fixed) rows.push(h('div', { class: 'r-pop-row' }, h('label', {}, 'Kiểu đọc'), seg(this.flowKey(), [['paginated', 'Lật trang'], ['scrolled', 'Cuộn']])));
    } else if (eng === 'comic') {
      rows.push(h('div', { class: 'r-pop-row' }, h('label', {}, 'Kiểu đọc'), seg('comicScroll', [[false, 'Lật trang'], [true, 'Cuộn dọc', 'Cuộn dọc liên tục (webtoon)']])));
      if (!p.comicScroll) rows.push(h('div', { class: 'r-pop-row' }, h('label', {}, 'Số trang'), seg('comicLayout', [['auto', 'Tự động'], ['single', '1 trang'], ['double', '2 trang']])),
        h('div', { class: 'r-pop-row' }, h('label', {}, 'Vừa'), seg('comicFit', [['page', 'Trang'], ['width', 'Chiều rộng']])));
      rows.push(h('div', { class: 'r-pop-row' }, h('label', {}, 'Hướng'), seg('comicDir', [['ltr', 'Trái → phải'], ['rtl', 'Phải → trái', 'Manga']])));
    } else if (eng === 'pdf') {
      rows.push(h('div', { class: 'r-pop-row' }, h('label', {}, 'Kiểu đọc'), seg('pdfFlow', [['scrolled', 'Cuộn'], ['paginated', 'Từng trang']])),
        h('div', { class: 'r-pop-row r-size' },
          h('button', { class: 'r-btn big', 'aria-label': 'Thu nhỏ', onclick: () => this.zoom(-1) }, '−'),
          h('button', { class: 'r-btn text', onclick: () => this.zoom(0), title: 'Vừa khung (Ctrl+0)' }, Math.round(p.pdfZoom * 100) + '%'),
          h('button', { class: 'r-btn big', 'aria-label': 'Phóng to', onclick: () => this.zoom(+1) }, '+')));
    }
    this.pop.replaceChildren(...rows);
  }

  moreMenu(e) {
    const r = e.currentTarget.getBoundingClientRect();
    const items = [
      this.info.prev ? { label: `‹ Vol trước: ${this.info.prev.title}`, run: () => this.openVolume(this.info.prev) } : null,
      this.info.next ? { label: `Vol sau: ${this.info.next.title} ›`, run: () => this.openVolume(this.info.next) } : null,
      this.ctx.mobile ? null : { label: 'Toàn màn hình (F11)', run: () => this.fullscreen() },
      this.ctx.mobile ? null : { label: 'Mở bằng ứng dụng mặc định', run: () => this.openExternal() },
      this.book.id < 0 ? null : { label: 'Đánh dấu đã đọc', run: async () => { await this.ctx.call('set_read_status', { ids: [this.book.id], status: 'read' }).catch(this.ctx.fail); this.changed = false; this.ctx.toast('Đã đánh dấu đã đọc.'); } },
      this.book.id < 0 ? null : '-',
      this.book.id < 0 ? null : { label: 'Xoá vào Thùng rác… (Delete)', danger: true, run: () => this.deleteBook() },
    ].filter(Boolean);
    const m = h('div', { class: 'menu r-menu', role: 'menu' }, ...items.map((it) => it === '-' ? h('hr') :
      h('button', { role: 'menuitem', class: it.danger ? 'danger' : '', onclick: () => { m.remove(); it.run(); } }, it.label)));
    this.root.append(m);
    // Thanh điều khiển ở đáy (iPad/iPhone): mở menu lên phía trên nút.
    m.style.top = (r.bottom + 6 + m.offsetHeight > innerHeight - 8 ? Math.max(8, r.top - 6 - m.offsetHeight) : r.bottom + 6) + 'px';
    m.style.left = Math.max(8, Math.min(r.right - m.offsetWidth, innerWidth - m.offsetWidth - 8)) + 'px';
    clearTimeout(this.hideT);
    const off = (ev) => { if (!m.contains(ev.target)) { m.remove(); removeEventListener('pointerdown', off, true); } };
    setTimeout(() => addEventListener('pointerdown', off, true), 0);
  }

  async openExternal() {
    try { await this.ctx.call('open_external', { id: this.book.id }); } catch (e) { this.ctx.fail(e); }
  }

  /** Xoá sách đang đọc (E7-14): xác nhận → Thùng rác → về thư viện. */
  async deleteBook() {
    if (this.book.id < 0) return this.ctx.toast('Sách này không nằm trong thư viện – không xoá từ đây.');
    await this.flush();
    if (await this.ctx.onDelete(this.book)) await this.close(true, true);
  }

  /** Điện thoại lưu kiểu đọc riêng (`phoneFlow`, mặc định cuộn) để không kế thừa 'Lật trang' đã lưu từ trước. */
  flowKey() { return this.ctx.phone ? 'phoneFlow' : 'flow'; }
  flow() { return this.prefs[this.flowKey()]; }

  // ---------------------------------------------------------------- slider + xem trước trang
  showPreview() {
    const label = this.engine.previewLabel?.(+this.slider.value);
    if (!label) return;
    this.preview.hidden = false;
    if (label !== this.lastPreview) this.preview.replaceChildren(...this.engine.previewContent?.(+this.slider.value) || [h('span', {}, label)]);
    this.lastPreview = label;
    const r = this.slider.getBoundingClientRect(), f = (+this.slider.value - +this.slider.min) / ((+this.slider.max - +this.slider.min) || 1);
    this.preview.style.left = clamp(r.left + f * r.width - this.preview.offsetWidth / 2, 8, innerWidth - this.preview.offsetWidth - 8) + 'px';
  }
  hoverPreview(e) {
    if (this.slider.matches(':active')) return;
    const r = this.slider.getBoundingClientRect(), f = clamp((e.clientX - r.left) / r.width, 0, 1);
    const v = +this.slider.min + f * (+this.slider.max - +this.slider.min);
    const label = this.engine.previewLabel?.(v);
    if (!label) return;
    this.preview.hidden = false;
    if (label !== this.lastPreview) this.preview.replaceChildren(...this.engine.previewContent?.(v) || [h('span', {}, label)]);
    this.lastPreview = label;
    this.preview.style.left = clamp(e.clientX - this.preview.offsetWidth / 2, 8, innerWidth - this.preview.offsetWidth - 8) + 'px';
  }

  // ---------------------------------------------------------------- đóng
  async close(notify = true, deleted = false) {
    if (this.closed) return;
    this.closed = true;
    clearTimeout(this.hideT);
    if (!deleted) await this.flush();
    document.removeEventListener('keydown', this.onKey, true);
    removeEventListener('beforeunload', this.onUnload);
    this.media.removeEventListener('change', this.onScheme);
    try { this.engine?.destroy(); } catch (e) { console.warn(e); }
    this.root.remove();
    document.body.classList.remove('reading');
    if (current === this) current = null;
    if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
    if (notify) this.ctx.onClose?.();
  }
}

// ------------------------------------------------------------------ foliate: EPUB/MOBI/AZW3/FB2
function foliateCSS(p, t) {
  const font = FONTS[p.font]?.css;
  const force = p.theme !== 'white' && !(p.theme === 'auto' && !appDark());
  return `
    @namespace epub "http://www.idpf.org/2007/ops";
    html { color-scheme: ${t.ui}; font-size: ${p.fontScale}% !important; -webkit-text-size-adjust: none; }
    html, body { background: transparent !important; color: ${t.fg} !important; }
    ${force ? `body *:not(a):not(img):not(svg):not(svg *) { color: inherit !important; background-color: transparent !important; border-color: currentColor; }
    a:link, a:visited { color: ${t.ui === 'dark' ? '#8ab4f8' : '#7a4d1e'} !important; }` : ''}
    ${font ? `body, p, div, span, li, blockquote, dd, dt, td, th, h1, h2, h3, h4, h5, h6 { font-family: ${font} !important; }` : ''}
    p, li, blockquote, dd { line-height: ${p.lineHeight} !important; text-align: ${p.justify ? 'justify' : 'start'}; hyphens: auto; widows: 2; orphans: 2; }
    [align="left"] { text-align: left; } [align="right"] { text-align: right; } [align="center"] { text-align: center; }
    img, svg { max-width: 100%; height: auto; }
    pre { white-space: pre-wrap !important; }
    aside[epub|type~="footnote"], aside[epub|type~="endnote"], aside[epub|type~="note"], aside[epub|type~="rearnote"] { display: none; }
  `;
}

const LAZY_FORMATS = new Set(['mobi', 'azw', 'azw3', 'prc']);

let CFI = null;
class FoliateEngine {
  constructor(r) { this.r = r; }

  async open(locator) {
    await import(FOLIATE + 'view.js');
    CFI ??= await import(FOLIATE + 'epubcfi.js').catch(() => null);
    const url = this.r.base + 'file', name = this.r.book.file_name.toLowerCase();
    let file;
    // MOBI/AZW đọc theo bản ghi nên đọc lười qua Range được – file truyện tranh vài trăm MB mở ngay.
    // EPUB/FB2 vẫn tải cả file (zip.js và trình phân tích FB2 cần File thật).
    const size = LAZY_FORMATS.has(this.r.book.format) ? await remoteSize(url) : null;
    if (size) file = new RemoteFile(url, size, name);
    else {
      const res = await fetch(url);
      if (!res.ok) throw new Error(await res.text() || res.statusText);
      const blob = await res.blob();
      file = new File([blob], name, { type: blob.type });
    }
    const view = (this.view = document.createElement('foliate-view'));
    view.setAttribute('autohide-cursor', '');
    this.r.stage.append(view);
    await view.open(file);
    this.fixed = view.isFixedLayout;
    const book = view.book;
    book.transformTarget?.addEventListener('data', ({ detail }) => {
      detail.data = Promise.resolve(detail.data).catch((e) => { console.warn('Không tải được', detail.name, e); return ''; });
    });
    view.addEventListener('load', (e) => this.onLoad(e.detail));
    view.addEventListener('relocate', (e) => this.onRelocate(e.detail));
    view.addEventListener('external-link', (e) => { e.preventDefault(); });
    this.apply(this.r.prefs);
    const cfi = locator && /^epubcfi\(/.test(locator) ? locator : null;
    await view.init({ lastLocation: cfi, showTextStart: !cfi });
    if (!this.last) this.onRelocate(view.lastLocation || { fraction: 0 });
  }

  onLoad({ doc }) {
    doc.addEventListener('keydown', (e) => this.r.key(e));
    doc.addEventListener('click', (e) => {
      if (e.defaultPrevented || e.target.closest?.('a[href]')) return;
      if (doc.getSelection()?.toString()) return;
      const w = doc.defaultView;
      // Toạ độ trong iframe → tỉ lệ theo khung nhìn của chính iframe (trang hiện tại).
      const frame = w.frameElement?.getBoundingClientRect();
      const x = frame ? (frame.left + e.clientX * (frame.width / w.innerWidth)) : e.clientX;
      const stage = this.r.stage.getBoundingClientRect();
      this.r.tap((x - stage.left) / stage.width);
    });
    doc.addEventListener('wheel', (e) => { if (this.r.flow() === 'paginated' && !this.fixed && Math.abs(e.deltaY) > 20) { e.preventDefault(); e.deltaY > 0 ? this.r.next() : this.r.prev(); } }, { passive: false });
  }

  onRelocate(d) {
    this.last = d;
    const loc = d.location, time = d.time, page = d.pageItem?.label;
    let left = d.tocItem?.label || '';
    if (page) left = (left ? left + ' · ' : '') + 'Trang ' + page;
    else if (loc?.total) left = (left ? left + ' · ' : '') + `Vị trí ${loc.current + 1}/${loc.total}`;
    const mins = Math.round(time?.section || 0);
    const right = pct(d.fraction) + (mins > 0 ? ` · còn ~${mins} phút trong chương` : '');
    this.r.relocate({ fraction: d.fraction, locator: d.cfi, left, right, atEnd: d.fraction >= 0.999 });
  }

  apply(p) {
    const view = this.view, t = themeOf(p);
    if (!view?.renderer) return;
    view.renderer.setStyles?.(foliateCSS(p, t));
    if (!this.fixed) {
      view.renderer.setAttribute('flow', this.r.flow());
      // Chế độ cuộn trên iPad/điện thoại: cuộn liền qua các chương (không dừng ở cuối chương).
      if (this.r.ctx.mobile) view.renderer.setAttribute('continuous', ''); else view.renderer.removeAttribute('continuous');
      view.renderer.setAttribute('gap', MARGINS[p.margin] || '7%');
      view.renderer.setAttribute('margin', '56px');
      view.renderer.setAttribute('max-inline-size', '720px');
      view.renderer.setAttribute('max-column-count', '2');
      if (reduceMotion()) view.renderer.removeAttribute('animated'); else view.renderer.setAttribute('animated', '');
    } else view.renderer.setAttribute('zoom', 'fit-page');
  }

  paged() { return this.fixed || this.r.flow() === 'paginated'; }
  dir() { return this.view?.book?.dir; }
  next() { return this.view.next(); }
  prev() { return this.view.prev(); }
  seek(f, commit) { if (commit) this.view.goToFraction(clamp(f, 0, 1)); }
  previewLabel(f) { return pct(f); }
  hasToc() { return !!this.view?.book?.toc?.length; }
  toc() { return this.view.book.toc; }
  currentTocHref() { return this.last?.tocItem?.href; }
  goTo(target, select) { return select ? this.view.select(target) : this.view.goTo(target); }
  /** Dấu trang `cfi` có nằm trong trang đang hiện không (CFI của trang là một khoảng start..end). */
  onPage(cfi) {
    const cur = this.last?.cfi;
    if (!CFI || !cur || !cfi || !/^epubcfi\(/.test(cfi)) return false;
    try {
      const a = CFI.collapse(cfi);
      return CFI.compare(a, CFI.collapse(cur)) >= 0 && CFI.compare(a, CFI.collapse(cur, true)) <= 0;
    } catch { return cfi === cur; }
  }

  async *search(q) {
    for await (const r of this.view.search({ query: q, matchCase: false, matchDiacritics: false, matchWholeWords: false, locales: 'und' })) {
      if (r === 'done') return;
      if (r.progress != null && !r.subitems) { yield { progress: r.progress }; continue; }
      if (r.subitems) yield { label: r.label, items: r.subitems.map((s) => ({ target: s.cfi, pre: s.excerpt.pre, match: s.excerpt.match, post: s.excerpt.post })) };
    }
  }
  clearSearch() { this.view?.clearSearch(); }

  destroy() { this.view?.close(); this.view?.remove(); }
}

// ------------------------------------------------------------------ truyện tranh: CBZ/CBR (E7-08, E7-09)
class ComicEngine {
  constructor(r) { this.r = r; this.i = 0; this.cache = new Map(); }

  async open(locator, progress) {
    const res = await fetch(this.r.base + 'pages');
    if (!res.ok) throw new Error(await res.text() || res.statusText);
    this.count = (await res.json()).count;
    this.r.slider.min = 0; this.r.slider.max = Math.max(0, this.count - 1); this.r.slider.step = 1;
    const p = parsePage(locator);
    this.i = clamp(p ?? Math.round((progress || 0) * (this.count - 1)), 0, this.count - 1);
    this.r.stage.classList.add('comic');
    this.onClick = (e) => { const s = this.r.stage.getBoundingClientRect(); this.r.tap((e.clientX - s.left) / s.width); };
    this.r.stage.addEventListener('click', this.onClick);
    this.onResize = debounce(() => { if (!this.scroll) this.show(this.i); }, 120);
    addEventListener('resize', this.onResize);
    this.apply(this.r.prefs);
  }

  url(i) { return this.r.base + 'page/' + i; }
  img(i) {
    const el = h('img', { class: 'c-page', alt: `Trang ${i + 1}`, draggable: 'false', decoding: 'async', src: this.url(i) });
    el.onerror = () => el.replaceWith(h('div', { class: 'c-page c-broken' }, `Không đọc được trang ${i + 1}`));
    return el;
  }
  preload(i) {
    for (const j of [i + 1, i + 2, i + 3, i - 1]) {
      if (j < 0 || j >= this.count || this.cache.has(j)) continue;
      const im = new Image(); im.decoding = 'async'; im.src = this.url(j); this.cache.set(j, im);
      if (this.cache.size > 24) this.cache.delete(this.cache.keys().next().value);
    }
  }

  apply(p) {
    const wasScroll = this.scroll;
    this.scroll = !!p.comicScroll;
    this.r.stage.dataset.fit = p.comicFit;
    this.r.stage.dataset.dir = p.comicDir;
    if (this.scroll) { if (!wasScroll || !this.list) this.buildScroll(); }
    else { this.teardownScroll(); this.show(this.i); }
  }

  double() {
    const L = this.r.prefs.comicLayout;
    if (L === 'single') return false;
    if (L === 'double') return true;
    const s = this.r.stage;
    return s.clientWidth > s.clientHeight * 1.15 && this.r.prefs.comicFit === 'page';
  }
  /** Trang mở đầu của "cặp" chứa trang i (trang bìa đứng một mình). */
  spreadStart(i) { return !this.double() || i === 0 ? i : (i % 2 === 1 ? i : i - 1); }
  spread(i) {
    if (!this.double() || i === 0) return [i];
    const a = this.spreadStart(i);
    return a + 1 < this.count ? [a, a + 1] : [a];
  }

  show(i) {
    i = clamp(i, 0, this.count - 1);
    const pages = this.spread(i);
    this.i = pages[0];
    const ordered = this.r.prefs.comicDir === 'rtl' ? [...pages].reverse() : pages;
    const spread = h('div', { class: 'c-spread' + (pages.length > 1 ? ' two' : '') }, ...ordered.map((j) => this.img(j)));
    this.r.stage.replaceChildren(spread);
    this.r.stage.scrollTop = 0;
    this.preload(pages[pages.length - 1]);
    this.report(pages);
  }

  report(pages) {
    const last = pages[pages.length - 1];
    const label = pages.length > 1 ? `Trang ${pages[0] + 1}–${last + 1} / ${this.count}` : `Trang ${pages[0] + 1} / ${this.count}`;
    const fraction = this.count > 1 ? last / (this.count - 1) : 1;
    this.r.relocate({ fraction, locator: 'page:' + pages[0], left: label, right: pct(fraction), atEnd: last >= this.count - 1 });
  }

  next() {
    if (this.scroll) return this.r.stage.scrollBy({ top: this.r.stage.clientHeight * 0.85, behavior: reduceMotion() ? 'auto' : 'smooth' });
    const pages = this.spread(this.i), n = pages[pages.length - 1] + 1;
    if (n < this.count) this.show(n);
  }
  prev() {
    if (this.scroll) return this.r.stage.scrollBy({ top: -this.r.stage.clientHeight * 0.85, behavior: reduceMotion() ? 'auto' : 'smooth' });
    if (this.i <= 0) return;
    this.show(this.spreadStart(this.i - 1));
  }
  seek(v, commit) {
    const i = clamp(Math.round(v), 0, this.count - 1);
    if (this.scroll) { if (commit) this.list.children[i]?.scrollIntoView(); return; }
    if (commit || this.count < 400) this.show(i);
  }
  sliderValue() { return this.i; }
  previewLabel(v) { return `Trang ${Math.round(v) + 1}`; }
  previewContent(v) { const i = Math.round(v); return [h('img', { src: this.url(i), alt: '', class: 'r-thumb' }), h('span', {}, `Trang ${i + 1} / ${this.count}`)]; }
  paged() { return !this.scroll; }
  dir() { return this.r.prefs.comicDir; }
  ownsKey(e) { return this.scroll && ['ArrowDown', 'ArrowUp'].includes(e.key); }

  // Cuộn dọc liên tục (webtoon): ảnh tải lười, theo dõi trang đang xem.
  buildScroll() {
    const list = (this.list = h('div', { class: 'c-scroll' }));
    for (let j = 0; j < this.count; j++) {
      const ph = h('div', { class: 'c-slot', 'data-i': j });
      list.append(ph);
    }
    this.r.stage.replaceChildren(list);
    this.io = new IntersectionObserver((ents) => {
      for (const en of ents) if (en.isIntersecting && !en.target.firstChild) { en.target.append(this.img(+en.target.dataset.i)); en.target.classList.add('loaded'); }
    }, { root: this.r.stage, rootMargin: '200% 0px' });
    [...list.children].forEach((c) => this.io.observe(c));
    this.onScroll = () => {
      const mid = this.r.stage.getBoundingClientRect().top + this.r.stage.clientHeight * 0.4;
      let cur = this.i;
      for (const c of list.children) { const rc = c.getBoundingClientRect(); if (rc.top <= mid && rc.bottom > mid) { cur = +c.dataset.i; break; } }
      if (cur !== this.i || !this.reported) { this.i = cur; this.reported = true; this.report([cur]); }
    };
    this.r.stage.addEventListener('scroll', this.onScroll, { passive: true });
    setTimeout(() => { list.children[this.i]?.scrollIntoView(); this.onScroll(); }, 0);
  }
  teardownScroll() {
    this.io?.disconnect(); this.io = null;
    if (this.onScroll) this.r.stage.removeEventListener('scroll', this.onScroll);
    this.onScroll = null; this.list = null; this.reported = false;
  }

  destroy() {
    this.teardownScroll();
    this.r.stage.removeEventListener('click', this.onClick);
    removeEventListener('resize', this.onResize);
    this.cache.clear();
  }
}

// ------------------------------------------------------------------ đọc file theo khúc (HTTP Range)
const RANGE_PIECE = 32 * 1024 * 1024; // khớp MAX_RANGE phía Rust: một phản hồi không quá 32 MB

/** Đọc byte [begin, end) của url, chia thành nhiều yêu cầu ≤ 32 MB; thử lại một lần nếu lỗi mạng/đọc. */
async function fetchRange(url, begin, end) {
  const out = new Uint8Array(end - begin);
  for (let p = begin; p < end;) {
    const to = Math.min(end, p + RANGE_PIECE);
    let buf;
    for (let attempt = 0; ; attempt++) {
      try {
        const r = await fetch(url, { headers: { Range: `bytes=${p}-${to - 1}` } });
        if (r.status !== 206 && r.status !== 200) throw new Error(await r.text() || r.statusText);
        buf = new Uint8Array(await r.arrayBuffer());
        if (r.status === 200) buf = buf.subarray(p, to); // máy chủ bỏ qua Range
        break;
      } catch (e) { if (attempt >= 1) throw e; }
    }
    if (!buf.length) throw new Error('Đọc file trả về rỗng');
    out.set(buf.subarray(0, to - p), p - begin);
    p += Math.min(buf.length, to - p);
  }
  return out;
}

/** Tổng dung lượng file (qua yêu cầu Range 1 byte); null nếu máy chủ không hỗ trợ Range. */
async function remoteSize(url) {
  const r = await fetch(url, { headers: { Range: 'bytes=0-0' } });
  if (!r.ok) throw new Error(await r.text() || r.statusText);
  await r.arrayBuffer();
  const total = +(r.headers.get('Content-Range') || '').split('/')[1];
  return r.status === 206 && total > 0 ? total : null;
}

/**
 * "File" ảo đọc lười qua HTTP Range: đủ cho foliate (name, type, size, slice(a, b).arrayBuffer()).
 * Lần đọc nhỏ (đầu bản ghi, bảng chỉ mục) đi qua bộ đệm khối 64 KB; lần đọc lớn (ảnh) tải thẳng, không giữ lại.
 */
class RemoteFile {
  static BLOCK = 64 * 1024;
  static MAX_BLOCKS = 512; // ~32 MB đệm
  constructor(url, size, name, type = '') {
    Object.assign(this, { url, size, name, type });
    this.blocks = new Map(); // chỉ số khối → Promise<Uint8Array>
  }
  block(i) {
    let p = this.blocks.get(i);
    if (p) { this.blocks.delete(i); this.blocks.set(i, p); return p; } // đưa về cuối (mới dùng)
    const B = RemoteFile.BLOCK;
    p = fetchRange(this.url, i * B, Math.min(this.size, (i + 1) * B));
    p.catch(() => this.blocks.delete(i));
    this.blocks.set(i, p);
    while (this.blocks.size > RemoteFile.MAX_BLOCKS) this.blocks.delete(this.blocks.keys().next().value);
    return p;
  }
  async read(start, end) {
    if (end <= start) return new Uint8Array(0);
    const B = RemoteFile.BLOCK;
    if (end - start > 4 * B) return fetchRange(this.url, start, end);
    const first = Math.floor(start / B), last = Math.floor((end - 1) / B);
    const parts = await Promise.all(Array.from({ length: last - first + 1 }, (_, k) => this.block(first + k)));
    if (parts.length === 1) return parts[0].slice(start - first * B, end - first * B);
    const out = new Uint8Array(end - start);
    parts.forEach((a, k) => {
      const off = (first + k) * B, from = Math.max(start, off), to = Math.min(end, off + a.length);
      out.set(a.subarray(from - off, to - off), from - start);
    });
    return out;
  }
  // foliate nhận ra "file" nhờ có arrayBuffer(); MOBI không gọi tới (chỉ dùng slice).
  arrayBuffer() { return this.slice().arrayBuffer(); }
  slice(start = 0, end = this.size) {
    const clampPos = (x) => (x < 0 ? Math.max(0, this.size + x) : Math.min(x, this.size));
    const s = clampPos(start), e = clampPos(end ?? this.size);
    return { size: Math.max(0, e - s), arrayBuffer: () => this.read(s, e).then((a) => a.buffer.byteLength === a.length ? a.buffer : a.slice().buffer) };
  }
}

// ------------------------------------------------------------------ PDF (E7-10): pdf.js + HTTP Range
let pdfjsPromise = null;
async function pdfjs() {
  pdfjsPromise ||= import(FOLIATE + 'vendor/pdfjs/pdf.mjs').then((m) => {
    const lib = m?.getDocument ? m : globalThis.pdfjsLib;
    lib.GlobalWorkerOptions.workerSrc = PDFJS + 'pdf.worker.mjs';
    return lib;
  });
  return pdfjsPromise;
}

class PdfEngine {
  constructor(r) { this.r = r; this.i = 0; this.rendered = new Map(); this.sizes = []; }

  async open(locator, progress) {
    const lib = await pdfjs();
    const url = this.r.base + 'file';
    // Khúc đầu cho biết tổng dung lượng; phần còn lại pdf.js tự xin theo Range khi cần.
    const head = await fetch(url, { headers: { Range: 'bytes=0-262143' } });
    if (!head.ok) throw new Error(await head.text() || head.statusText);
    const first = new Uint8Array(await head.arrayBuffer());
    const total = +(head.headers.get('Content-Range') || '').split('/')[1];
    const common = { cMapUrl: PDFJS + 'cmaps/', cMapPacked: true, standardFontDataUrl: PDFJS + 'standard_fonts/', isEvalSupported: false, enableXfa: false };
    let task;
    if (head.status === 206 && total > first.length) {
      const transport = new lib.PDFDataRangeTransport(total, first);
      // pdf.js có thể xin một khối > 32 MB (ảnh trang lớn) – fetchRange tự chia nhỏ rồi trả về một lần.
      // Giới hạn số yêu cầu chạy cùng lúc: pdf.js có thể xin hàng nghìn khúc một lượt (vd. đọc hết cây trang).
      let warned = false, active = 0;
      const queue = [];
      const pump = () => {
        while (active < 6 && queue.length) {
          const [begin, end] = queue.shift();
          active++;
          fetchRange(url, begin, end).then((buf) => transport.onDataRange(begin, buf)).catch((e) => {
            console.error('PDF range', begin, end, e);
            if (!warned) { warned = true; this.r.ctx.fail('Không đọc được một phần file PDF: ' + (e?.message || e)); }
          }).finally(() => { active--; pump(); });
        }
      };
      transport.requestDataRange = (begin, end) => { queue.push([begin, end]); pump(); };
      task = lib.getDocument({ ...common, range: transport, rangeChunkSize: 1 << 20, disableAutoFetch: true, disableStream: true });
    } else {
      task = lib.getDocument({ ...common, data: first });
    }
    this.doc = await task.promise;
    this.count = this.doc.numPages;
    const p1 = await this.doc.getPage(1);
    const vp = p1.getViewport({ scale: 1 });
    this.base = { w: vp.width, h: vp.height };
    this.sizes = Array.from({ length: this.count }, () => null);
    this.sizes[0] = this.base;
    this.r.slider.min = 0; this.r.slider.max = Math.max(0, this.count - 1); this.r.slider.step = 1;
    const p = parsePage(locator);
    this.i = clamp(p ?? Math.round((progress || 0) * (this.count - 1)), 0, this.count - 1);
    this.r.stage.classList.add('pdf');
    this.onClick = (e) => { const s = this.r.stage.getBoundingClientRect(); this.r.tap((e.clientX - s.left) / s.width); };
    this.r.stage.addEventListener('click', this.onClick);
    this.onWheel = (e) => { if (e.ctrlKey) { e.preventDefault(); this.r.zoom(e.deltaY < 0 ? 1 : -1); } else if (!this.scrolled() && Math.abs(e.deltaY) > 30 && this.fitsVertically()) { e.preventDefault(); e.deltaY > 0 ? this.r.next() : this.r.prev(); } };
    this.r.stage.addEventListener('wheel', this.onWheel, { passive: false });
    this.onResize = debounce(() => this.layout(), 150);
    addEventListener('resize', this.onResize);
    this.outline = await this.loadOutline().catch(() => []);
    this.apply(this.r.prefs);
  }

  scrolled() { return this.r.prefs.pdfFlow === 'scrolled'; }
  fitsVertically() { return this.r.stage.scrollHeight <= this.r.stage.clientHeight + 2; }
  size(j) { return this.sizes[j] || this.base; }

  apply() { this.layout(); }

  /** Chiều rộng hiển thị (CSS px) của trang j. */
  cssWidth(j) {
    const s = this.r.stage, sz = this.size(j), z = this.r.prefs.pdfZoom;
    const availW = s.clientWidth - 48, availH = s.clientHeight - (this.scrolled() ? 24 : 32);
    const fitW = Math.min(availW, 1100);
    const w = this.scrolled() ? fitW : Math.min(availW, availH * (sz.w / sz.h));
    return Math.max(120, w * z);
  }

  layout() {
    if (!this.doc) return;
    this.io?.disconnect();
    for (const { task } of this.rendered.values()) task?.cancel?.();
    this.rendered.clear();
    const keep = this.i;
    const wrap = (this.wrap = h('div', { class: 'pdf-pages' + (this.scrolled() ? ' scrolled' : ' single') }));
    if (this.scrolled()) {
      for (let j = 0; j < this.count; j++) wrap.append(this.slot(j));
      this.r.stage.replaceChildren(wrap);
      this.io = new IntersectionObserver((ents) => { for (const en of ents) if (en.isIntersecting) this.render(+en.target.dataset.i); }, { root: this.r.stage, rootMargin: '120% 0px' });
      [...wrap.children].forEach((c) => this.io.observe(c));
      if (this.onScroll) this.r.stage.removeEventListener('scroll', this.onScroll);
      this.onScroll = () => this.trackScroll();
      this.r.stage.addEventListener('scroll', this.onScroll, { passive: true });
      wrap.children[keep]?.scrollIntoView({ block: 'start' });
      this.trackScroll(true);
    } else {
      if (this.onScroll) { this.r.stage.removeEventListener('scroll', this.onScroll); this.onScroll = null; }
      this.r.stage.replaceChildren(wrap);
      this.showPage(keep);
    }
  }

  slot(j) {
    const w = this.cssWidth(j), sz = this.size(j);
    return h('div', { class: 'pdf-page', 'data-i': j, style: `width:${w}px;height:${(w * sz.h) / sz.w}px` }, h('span', { class: 'pdf-num' }, String(j + 1)));
  }

  async render(j) {
    const el = this.wrap?.children[this.scrolled() ? j : 0];
    if (!el || this.rendered.has(j)) return;
    const entry = { task: null };
    this.rendered.set(j, entry);
    try {
      const page = await this.doc.getPage(j + 1);
      const vp1 = page.getViewport({ scale: 1 });
      if (!this.sizes[j]) { this.sizes[j] = { w: vp1.width, h: vp1.height }; }
      const w = this.cssWidth(j), hgt = (w * vp1.height) / vp1.width;
      el.style.width = w + 'px'; el.style.height = hgt + 'px';
      const dpr = Math.min(devicePixelRatio || 1, 2);
      let scale = (w / vp1.width) * dpr;
      const maxPx = 16e6; // giới hạn điểm ảnh mỗi trang để RAM không phình
      if (vp1.width * vp1.height * scale * scale > maxPx) scale = Math.sqrt(maxPx / (vp1.width * vp1.height));
      const vp = page.getViewport({ scale });
      const canvas = h('canvas', { width: Math.floor(vp.width), height: Math.floor(vp.height), 'aria-label': `Trang ${j + 1}` });
      entry.task = page.render({ canvas, canvasContext: canvas.getContext('2d'), viewport: vp });
      await entry.task.promise;
      if (this.rendered.get(j) !== entry) return;
      el.replaceChildren(canvas);
      el.classList.add('done');
      this.evict(j);
    } catch (e) {
      if (e?.name !== 'RenderingCancelledException') { console.warn('PDF trang', j + 1, e); el.replaceChildren(h('span', { class: 'pdf-num' }, `Không vẽ được trang ${j + 1}`)); }
      this.rendered.delete(j);
    }
  }

  /** Giữ tối đa ~10 trang đã vẽ quanh vị trí đang đọc, giải phóng canvas ở xa. */
  evict(center) {
    if (!this.scrolled() || this.rendered.size <= 10) return;
    const far = [...this.rendered.keys()].sort((a, b) => Math.abs(b - center) - Math.abs(a - center));
    for (const j of far.slice(0, this.rendered.size - 10)) {
      this.rendered.get(j)?.task?.cancel?.();
      this.rendered.delete(j);
      const el = this.wrap.children[j];
      if (el) { el.classList.remove('done'); el.replaceChildren(h('span', { class: 'pdf-num' }, String(j + 1))); }
    }
  }

  trackScroll(force) {
    const s = this.r.stage, mid = s.getBoundingClientRect().top + s.clientHeight * 0.35;
    // Tìm nhị phân trang đầu tiên có mép dưới vượt qua vạch 35% khung nhìn (các trang xếp dọc).
    const kids = this.wrap.children;
    let lo = 0, hi = kids.length - 1, cur = kids.length - 1;
    while (lo <= hi) {
      const m = (lo + hi) >> 1;
      if (kids[m].getBoundingClientRect().bottom >= mid) { cur = m; hi = m - 1; } else lo = m + 1;
    }
    if (cur !== this.i || force) { this.i = cur; this.report(); }
  }

  showPage(j) {
    this.i = clamp(j, 0, this.count - 1);
    this.rendered.forEach(({ task }) => task?.cancel?.());
    this.rendered.clear();
    this.wrap.replaceChildren(this.slot(this.i));
    this.r.stage.scrollTop = 0;
    this.render(this.i);
    // Tải trước trang kế.
    if (this.i + 1 < this.count) this.doc.getPage(this.i + 2).catch(() => {});
    this.report();
  }

  report() {
    const f = this.count > 1 ? this.i / (this.count - 1) : 1;
    const ch = this.chapterOf(this.i);
    this.r.relocate({ fraction: f, locator: 'page:' + this.i, left: (ch ? ch + ' · ' : '') + `Trang ${this.i + 1} / ${this.count}`, right: pct(f), atEnd: this.i >= this.count - 1 });
  }

  next() {
    if (this.scrolled()) return this.r.stage.scrollBy({ top: this.r.stage.clientHeight * 0.9, behavior: reduceMotion() ? 'auto' : 'smooth' });
    if (!this.fitsVertically() && this.r.stage.scrollTop + this.r.stage.clientHeight < this.r.stage.scrollHeight - 4) return this.r.stage.scrollBy({ top: this.r.stage.clientHeight * 0.9 });
    if (this.i + 1 < this.count) this.showPage(this.i + 1);
  }
  prev() {
    if (this.scrolled()) return this.r.stage.scrollBy({ top: -this.r.stage.clientHeight * 0.9, behavior: reduceMotion() ? 'auto' : 'smooth' });
    if (this.i > 0) this.showPage(this.i - 1);
  }
  seek(v, commit) {
    const j = clamp(Math.round(v), 0, this.count - 1);
    if (!commit) return;
    if (this.scrolled()) { this.wrap.children[j]?.scrollIntoView({ block: 'start' }); this.i = j; this.report(); }
    else this.showPage(j);
  }
  sliderValue() { return this.i; }
  previewLabel(v) { const j = Math.round(v); const ch = this.chapterOf(j); return (ch ? ch + ' · ' : '') + `Trang ${j + 1}`; }
  paged() { return !this.scrolled(); }
  ownsKey(e) { return this.scrolled() && ['ArrowDown', 'ArrowUp'].includes(e.key); }

  async loadOutline() {
    const out = await this.doc.getOutline();
    if (!out?.length) return [];
    const resolve = async (dest) => {
      try {
        const d = typeof dest === 'string' ? await this.doc.getDestination(dest) : dest;
        if (!d) return null;
        return typeof d[0] === 'number' ? d[0] : await this.doc.getPageIndex(d[0]);
      } catch { return null; }
    };
    const walk = async (items, depth) => Promise.all(items.map(async (it) => {
      const page = it.dest ? await resolve(it.dest) : null;
      return { label: it.title, href: page, page: page != null ? page + 1 : null, subitems: depth < 4 && it.items?.length ? await walk(it.items, depth + 1) : [] };
    }));
    const tree = await walk(out, 0);
    const flat = [];
    const fl = (arr) => arr.forEach((x) => { if (x.href != null) flat.push(x); fl(x.subitems); });
    fl(tree);
    this.flat = flat.sort((a, b) => a.href - b.href);
    return tree;
  }
  chapterOf(j) { let c = null; for (const x of this.flat || []) { if (x.href <= j) c = x.label; else break; } return c; }
  hasToc() { return !!this.outline?.length; }
  toc() { return this.outline; }
  currentTocHref() { let c = null; for (const x of this.flat || []) { if (x.href <= this.i) c = x.href; else break; } return c; }
  goTo(target) { if (typeof target === 'number') this.seek(target, true); }

  async *search(q) {
    const fq = fold(q);
    for (let j = 0; j < this.count; j++) {
      const page = await this.doc.getPage(j + 1);
      const tc = await page.getTextContent();
      const text = tc.items.map((it) => it.str + (it.hasEOL ? '\n' : '')).join('');
      // fold giữ nguyên độ dài với tiếng Việt (bỏ dấu kết hợp sau NFD), nên ánh xạ vị trí qua NFD.
      const nfd = text.normalize('NFD');
      const map = [];
      let folded = '';
      for (let k = 0; k < nfd.length; k++) { const c = nfd[k]; if (/\p{M}/u.test(c)) continue; folded += fold(c); map.push(k); }
      const items = [];
      let at = folded.indexOf(fq);
      while (at >= 0 && items.length < 50) {
        const s = map[at], e = (map[at + fq.length - 1] ?? s) + 1;
        const pre = nfd.slice(Math.max(0, s - 40), s), post = nfd.slice(e, e + 60);
        items.push({ target: j, pre: '…' + pre.normalize('NFC').replace(/\s+/g, ' '), match: nfd.slice(s, e).normalize('NFC'), post: post.normalize('NFC').replace(/\s+/g, ' ') + '…' });
        at = folded.indexOf(fq, at + fq.length);
      }
      if (items.length) yield { label: `Trang ${j + 1}`, items };
      if (j % 10 === 9) yield { progress: (j + 1) / this.count };
    }
  }

  destroy() {
    this.io?.disconnect();
    this.rendered.forEach(({ task }) => task?.cancel?.());
    this.r.stage.removeEventListener('click', this.onClick);
    this.r.stage.removeEventListener('wheel', this.onWheel);
    if (this.onScroll) this.r.stage.removeEventListener('scroll', this.onScroll);
    removeEventListener('resize', this.onResize);
    this.doc?.destroy();
  }
}

// ------------------------------------------------------------------ TXT
class TextEngine {
  constructor(r) { this.r = r; }
  async open(locator, progress) {
    const res = await fetch(this.r.base + 'file');
    if (!res.ok) throw new Error(await res.text() || res.statusText);
    const buf = await res.arrayBuffer();
    let text;
    try { text = new TextDecoder('utf-8', { fatal: true }).decode(buf); } catch { text = new TextDecoder('windows-1258').decode(buf); }
    this.col = h('article', { class: 'txt-col' }, ...text.split(/\r?\n\s*\r?\n/).slice(0, 20000).map((para) => h('p', {}, para.trim())));
    this.r.stage.classList.add('text');
    this.r.stage.replaceChildren(this.col);
    this.apply(this.r.prefs);
    this.onScroll = () => this.report();
    this.r.stage.addEventListener('scroll', this.onScroll, { passive: true });
    this.onClick = (e) => { if (!getSelection().toString()) this.r.toggleChrome(); };
    this.r.stage.addEventListener('click', this.onClick);
    const f = locator?.startsWith('scroll:') ? +locator.slice(7) : progress || 0;
    setTimeout(() => { this.seek(f); this.report(); }, 0);
  }
  max() { return Math.max(1, this.r.stage.scrollHeight - this.r.stage.clientHeight); }
  report() { const f = this.r.stage.scrollTop / this.max(); this.r.relocate({ fraction: f, locator: 'scroll:' + f.toFixed(5), left: '', right: pct(f), atEnd: f >= 0.995 }); }
  apply(p) {
    const font = FONTS[p.font]?.css || FONTS.serif.css;
    Object.assign(this.col.style, { fontSize: (p.fontScale / 100) * 19 + 'px', fontFamily: font, lineHeight: p.lineHeight, textAlign: p.justify ? 'justify' : 'start', paddingInline: MARGINS[p.margin] });
  }
  paged() { return false; }
  next() { this.r.stage.scrollBy({ top: this.r.stage.clientHeight * 0.9, behavior: 'smooth' }); }
  prev() { this.r.stage.scrollBy({ top: -this.r.stage.clientHeight * 0.9, behavior: 'smooth' }); }
  seek(f) { this.r.stage.scrollTop = clamp(f, 0, 1) * this.max(); }
  previewLabel(f) { return pct(f); }
  ownsKey(e) { return ['ArrowDown', 'ArrowUp'].includes(e.key); }
  destroy() { this.r.stage.removeEventListener('scroll', this.onScroll); this.r.stage.removeEventListener('click', this.onClick); }
}
