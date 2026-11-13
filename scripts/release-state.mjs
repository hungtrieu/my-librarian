// Theo dõi bản build của app Windows / iPad / iPhone / Android: code có đổi kể từ lần build trước không, phiên bản tiếp theo là gì.
// Dùng bởi build-windows.cmd, build-ipad.command và build-android.command – không cần gọi tay.
//
//   node scripts/release-state.mjs check  <windows|ipad|iphone|android> [--force] [--shell]
//   node scripts/release-state.mjs record <windows|ipad|iphone|android> <phiên bản> <thư mục bản build> <dấu vân tay>
//
// "Dấu vân tay" = băm nội dung (git hash-object) mọi file nguồn ảnh hưởng tới app đó, kể cả thay đổi chưa commit.
// Trạng thái lưu ở dist/<windows|ipad|iphone|android>-release.json (dist/ không commit).
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('..', import.meta.url));
const git = (args, input) => execFileSync('git', args, { cwd: root, input, maxBuffer: 256 << 20 }).toString();

// File nguồn ảnh hưởng tới từng app. Tài liệu (.md) và file sao lưu không tính.
const TARGETS = {
  windows: {
    conf: 'crates/app/tauri.conf.json',
    paths: ['Cargo.toml', 'Cargo.lock', 'crates/core', 'crates/scanner-cli', 'crates/app', 'ui'],
    skip: (p) => p.startsWith('ui/ipad/'),
  },
  ipad: {
    conf: 'crates/ipad/tauri.conf.json',
    paths: ['Cargo.toml', 'Cargo.lock', 'crates/core', 'crates/ipad', 'ui', 'scripts/build-ipad-ui.mjs', 'scripts/ipad-install.sh'],
    // build-ipad-ui.mjs bỏ các file chỉ của app Windows.
    skip: (p) => ['ui/app.js', 'ui/index.html', 'ui/mock.js', 'ui/ipad/mock.js'].includes(p),
  },
};
// iPhone dùng chung app (và mã nguồn) với iPad; theo dõi riêng để biết bản trên mỗi máy cài lúc nào.
TARGETS.iphone = TARGETS.ipad;
// Android cũng dùng chung crate và giao diện với iPad/iPhone, thêm script build riêng. APK không hết hạn nên không cần ký lại.
TARGETS.android = { ...TARGETS.ipad, paths: [...TARGETS.ipad.paths.filter((p) => p !== 'scripts/ipad-install.sh'), 'build-android.command'] };
// App iPad tự ký bằng Apple ID miễn phí hết hạn sau 7 ngày: quá 6 ngày thì build/cài lại (không tăng phiên bản).
const RESIGN_DAYS = 6;

const [cmd, target, ...rest] = process.argv.slice(2);
const T = TARGETS[target];
if (!T || !['check', 'record'].includes(cmd)) {
  console.error('dùng: release-state.mjs check|record windows|ipad|iphone|android …');
  process.exit(2);
}
const stateFile = join(root, 'dist', `${target}-release.json`);
const readStateOf = (t) => { try { return JSON.parse(readFileSync(join(root, 'dist', `${t}-release.json`), 'utf8')); } catch { return null; } };
const readState = () => readStateOf(target);

function fingerprint() {
  const files = git(['ls-files', '-z', '--cached', '--others', '--exclude-standard', '--', ...T.paths])
    .split('\0')
    .filter((p) => p && !/\.(md|bak)$/i.test(p) && !T.skip(p) && existsSync(join(root, p)));
  const unique = [...new Set(files)].sort();
  const hashes = git(['hash-object', '--stdin-paths'], unique.join('\n') + '\n').trim().split('\n');
  const h = createHash('sha256');
  unique.forEach((p, i) => h.update(`${p}\0${hashes[i]}\n`));
  return h.digest('hex').slice(0, 16);
}

const parseVer = (v) => (/^(\d+)\.(\d+)\.(\d+)$/.exec(v || '') || []).slice(1).map(Number);
const cmpVer = (a, b) => { const x = parseVer(a), y = parseVer(b); for (let i = 0; i < 3; i++) if ((x[i] ?? 0) !== (y[i] ?? 0)) return (x[i] ?? 0) - (y[i] ?? 0); return 0; };
const bump = (v) => { const [a = 0, b = 1, c = 0] = parseVer(v); return `${a}.${b}.${c + 1}`; };

if (cmd === 'check') {
  const force = rest.includes('--force');
  const state = readState();
  const confVersion = JSON.parse(readFileSync(join(root, T.conf), 'utf8')).version || '0.1.0';
  const fp = fingerprint();
  // iPhone đánh số tiếp theo bản lớn nhất của cả iPad lẫn iPhone (cùng một app).
  const peers = [state, target === 'iphone' ? readStateOf('ipad') : null].filter(Boolean).map((x) => x.version);
  const base = [confVersion, ...peers].reduce((m, v) => (cmpVer(v, m) > 0 ? v : m));
  const ageDays = state?.builtAt ? (Date.now() - Date.parse(state.builtAt)) / 86400e3 : Infinity;
  let action, reason;
  if (!state) [action, reason] = ['build', 'chưa có bản build nào được ghi lại'];
  else if (force) [action, reason] = ['build', 'yêu cầu build lại (--force)'];
  else if (state.fingerprint !== fp) [action, reason] = ['build', 'mã nguồn đã thay đổi'];
  else if ((target === 'ipad' || target === 'iphone') && ageDays > RESIGN_DAYS) [action, reason] = ['resign', `bản cài trên ${target === 'iphone' ? 'iPhone' : 'iPad'} đã ${Math.floor(ageDays)} ngày (Apple ID miễn phí hết hạn sau 7 ngày)`];
  else [action, reason] = ['none', 'mã nguồn không đổi'];
  const out = {
    action,
    reason,
    fingerprint: fp,
    version: action === 'build' ? bump(base) : state.version,
    lastVersion: state?.version ?? '',
    lastFolder: state?.folder ?? '',
    lastBuiltAt: state?.builtAt ?? '',
  };
  if (rest.includes('--shell')) {
    for (const [k, v] of Object.entries(out)) console.log(`${k}='${String(v).replaceAll("'", "'\\''")}'`);
  } else console.log(JSON.stringify(out));
} else {
  const [version, folder, fp] = rest;
  if (!parseVer(version).length || !folder || !fp) { console.error('record: thiếu phiên bản/thư mục/dấu vân tay'); process.exit(2); }
  mkdirSync(join(root, 'dist'), { recursive: true });
  writeFileSync(stateFile, JSON.stringify({ version, folder, fingerprint: fp, builtAt: new Date().toISOString() }, null, 2) + '\n');
}
