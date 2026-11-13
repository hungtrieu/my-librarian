// Tiện ích giao diện dùng chung.

const PROPS = new Set(['checked', 'selected', 'disabled', 'value', 'hidden']);

/** Tạo phần tử DOM: h('div', { class: 'x', onclick }, 'chữ', con, [mảng]). Không dùng innerHTML. */
export function h(tag, attrs = {}, ...children) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs || {})) {
    if (v == null || v === false && !PROPS.has(k)) continue;
    if (k.startsWith('on') && typeof v === 'function') el.addEventListener(k.slice(2), v);
    else if (PROPS.has(k)) el[k] = v;
    else el.setAttribute(k, v === true ? '' : v);
  }
  append(el, children);
  return el;
}

function append(el, children) {
  for (const c of children) {
    if (c == null || c === false) continue;
    if (Array.isArray(c)) append(el, c);
    else el.append(c instanceof Node ? c : document.createTextNode(String(c)));
  }
}

export function frag(...children) {
  const f = document.createDocumentFragment();
  append(f, children);
  return f;
}

/** Bỏ dấu tiếng Việt + chữ thường (khớp với `normalize::fold` phía Rust). */
export const fold = (s) => String(s ?? '').normalize('NFD').replace(/\p{M}/gu, '').replace(/[đĐ]/g, 'd').toLowerCase();

/** Tô sáng các từ bắt đầu bằng một token của truy vấn (không dấu) – E3-02. */
export function highlight(text, q) {
  text = String(text ?? '');
  const toks = fold(q).split(/[^\p{L}\p{N}]+/u).filter(Boolean);
  if (!toks.length) return document.createTextNode(text);
  const f = document.createDocumentFragment();
  let last = 0;
  for (const m of text.matchAll(/[\p{L}\p{N}]+/gu)) {
    const w = m[0], fw = fold(w);
    const t = toks.filter((t) => fw.startsWith(t)).sort((a, b) => b.length - a.length)[0];
    if (!t) continue;
    let k = 0;
    while (k < w.length && fold(w.slice(0, k)).length < t.length) k++;
    f.append(text.slice(last, m.index));
    const mk = document.createElement('mark');
    mk.textContent = w.slice(0, k);
    f.append(mk);
    last = m.index + k;
  }
  f.append(text.slice(last));
  return f;
}

export function fmtBytes(b) {
  if (!b) return '0 B';
  const u = ['B', 'KB', 'MB', 'GB', 'TB'];
  let i = 0;
  while (b >= 1024 && i < u.length - 1) { b /= 1024; i++; }
  return (i ? b.toFixed(b < 10 ? 1 : 0) : b) + ' ' + u[i];
}

export const fmtDate = (sec) => (sec ? new Date(sec * 1000).toLocaleString('vi-VN', { dateStyle: 'short', timeStyle: 'short' }) : '—');

function hash(s) {
  let x = 2166136261;
  for (const c of String(s)) x = Math.imul(x ^ c.codePointAt(0), 16777619);
  return x >>> 0;
}

/** Bìa mặc định khi không trích được ảnh (E2-09): màu theo tên, chữ tên sách + tác giả. */
export function genCover(title, author, format) {
  const x = hash(title), hue = x % 360, hue2 = (hue + 25 + (x >> 9) % 40) % 360;
  return h('div', { class: 'gen-cover', style: `background:linear-gradient(155deg,hsl(${hue} 46% 46%),hsl(${hue2} 52% 30%))`, 'aria-hidden': 'true' },
    h('div', { class: 't' }, title || 'Không tên'), h('div', { class: 'a' }, author || ''), format ? h('div', { class: 'f' }, format) : null);
}

export function debounce(fn, ms) {
  let t;
  return (...a) => { clearTimeout(t); t = setTimeout(() => fn(...a), ms); };
}
