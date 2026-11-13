#!/usr/bin/env node
// Ký APK Android bằng MỘT khoá dùng chung cho mọi máy build (Mac, Windows…), để cài đè không báo
// INSTALL_FAILED_UPDATE_INCOMPATIBLE ("chữ ký không khớp") khi đổi máy build.
//
// Khoá: crates/ipad/android/debug.keystore (KHÔNG commit – khoá riêng, nằm trong .gitignore;
// build trên máy khác thì chép tay file này sang). Chưa có thì lấy khoá debug của máy đang build
// (~/.android/debug.keystore) làm khoá chung.
// Sửa crates/ipad/gen/android/app/build.gradle.kts (thư mục gen bị gitignore, sinh lại khi init)
// để bản debug ký bằng khoá đó. Chạy nhiều lần không sao.
//
// Dùng: node scripts/android-signing.mjs   (gọi từ build-android.ps1 / build-android.command)
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const keystore = path.join(root, 'crates', 'ipad', 'android', 'debug.keystore');
const gradle = path.join(root, 'crates', 'ipad', 'gen', 'android', 'app', 'build.gradle.kts');
const MARK = '// librarian: shared debug signing';

function fail(msg) {
  console.error(msg);
  process.exit(1);
}

if (!fs.existsSync(keystore)) {
  const local = path.join(os.homedir(), '.android', 'debug.keystore');
  if (!fs.existsSync(local)) fail(`Chưa có khoá ký chung ${keystore} và máy này cũng chưa có ${local}.`);
  fs.mkdirSync(path.dirname(keystore), { recursive: true });
  fs.copyFileSync(local, keystore);
  console.log(`Lấy khoá debug của máy này làm khoá ký chung: ${keystore} – chép file này sang máy build khác để dùng cùng khoá (không commit).`);
}

if (!fs.existsSync(gradle)) fail(`Không thấy ${gradle} (chưa chạy cargo tauri android init?).`);
let src = fs.readFileSync(gradle, 'utf8');

// Bỏ khối cũ (nếu có) rồi chèn lại, để đường dẫn khoá luôn đúng với thư mục repo hiện tại.
src = src.replace(new RegExp(`\\n?[ \\t]*${MARK} start[\\s\\S]*?${MARK} end\\n`, 'g'), '\n');
src = src.replace(/\n[ \t]*signingConfig = signingConfigs\.getByName\("librarianShared"\)\n/g, '\n');

const storePath = keystore.replace(/\\/g, '/');
const block = `    ${MARK} start
    signingConfigs {
        create("librarianShared") {
            storeFile = file("${storePath}")
            storePassword = "android"
            keyAlias = "androiddebugkey"
            keyPassword = "android"
        }
    }
    ${MARK} end
`;

const androidOpen = /\nandroid \{\n/;
if (!androidOpen.test(src)) fail(`Không tìm thấy khối "android {" trong ${gradle}.`);
src = src.replace(androidOpen, (m) => m + block);

const debugOpen = /(getByName\("debug"\) \{\n)/;
if (!debugOpen.test(src)) fail(`Không tìm thấy buildTypes debug trong ${gradle}.`);
src = src.replace(debugOpen, `$1            signingConfig = signingConfigs.getByName("librarianShared")\n`);

fs.writeFileSync(gradle, src);
console.log(`Ký APK bằng khoá chung: ${keystore}`);
