// Ghép giao diện iPad vào target/ipad-ui (I1-02): file dùng chung của ui/ + ui/ipad/ trải phẳng, bỏ phần chỉ của app Windows.
import { cp, rm, mkdir, readFile, writeFile, readdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';

const ui = fileURLToPath(new URL('../ui/', import.meta.url));
const out = fileURLToPath(new URL('../target/ipad-ui/', import.meta.url));
const skip = new Set(['app.js', 'mock.js', 'index.html', 'ipad']);
await rm(out, { recursive: true, force: true });
await mkdir(out, { recursive: true });
for (const n of await readdir(ui)) if (!skip.has(n)) await cp(join(ui, n), join(out, n), { recursive: true });
for (const n of await readdir(join(ui, 'ipad'))) {
  if (n === 'mock.js') continue;
  let t = await readFile(join(ui, 'ipad', n), 'utf8');
  if (/\.(html|js|css)$/.test(n)) t = t.replaceAll('../', './');
  await writeFile(join(out, n), t);
}
console.log('đã ghép giao diện iPad →', out);
