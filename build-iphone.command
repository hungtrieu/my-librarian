#!/usr/bin/env bash
# Build và cài app My Librarian lên iPhone – bấm đúp file này trong Finder (máy Mac), hoặc ./build-iphone.command
# Cùng app với iPad (cùng Bundle ID); trên iPhone app chỉ tải/đọc sách chữ (EPUB, MOBI/AZW3, FB2, TXT – không PDF,
# không truyện tranh) và dồn thanh nút nhanh của trình đọc (đánh dấu trang, mục lục, tìm, Aa) về góc dưới trái.
# Mọi tuỳ chọn như build-ipad.command (--force, LIBRARIAN_IPAD_DEVICE=<tên|UDID>, …). Hướng dẫn: docs/IPHONE.md.
cd "$(dirname "$0")"
LIBRARIAN_DEVICE_KIND=iPhone exec ./build-ipad.command "$@"
