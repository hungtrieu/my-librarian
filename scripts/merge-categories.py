#!/usr/bin/env python3
"""Gộp thư mục thể loại trùng / lồng nhau trong kho sách.

Ví dụ: có "Bút Ký - Hồi Ký" và "Bút Ký - Hồi Ký - Thơ" → sách hồi ký/bút ký trong thư mục sau được
chuyển về "Bút Ký - Hồi Ký", phần còn lại (thơ) nằm trong thư mục mới tên "Thơ".
Thư mục cùng bộ thể loại ("Hồi Ký, Tuỳ Bút" và "Hồi Ký - Tùy Bút", "Dã sử - Quân sự 1/2/3") được gộp làm một.

Mỗi tác phẩm (mọi định dạng cùng tên) được phân loại theo: tên sách, metadata EPUB (chủ đề, mô tả),
từ khoá trong phần đầu nội dung, và hình dạng dòng (thơ = nhiều dòng ngắn không chấm câu).

Cách dùng (không ghi gì ra đĩa sách cho tới bước --apply):
    python -I scripts/merge-categories.py plan  D:\\books              → in kế hoạch + ghi merge-plan.csv
    python -I scripts/merge-categories.py apply D:\\books merge-plan.csv → di chuyển theo CSV (có thể đã sửa tay)
    python -I scripts/merge-categories.py undo  merge-undo-<thời gian>.jsonl
"""

from __future__ import annotations

import argparse
import csv
import html
import json
import os
import re
import struct
import sys
import time
import unicodedata
import zipfile
from dataclasses import dataclass, field
from pathlib import Path

for _s in (sys.stdout, sys.stderr):
    try:
        _s.reconfigure(encoding="utf-8")
    except Exception:
        pass

# ---------------------------------------------------------------- chuẩn hoá tên


def fold(s: str) -> str:
    s = unicodedata.normalize("NFD", s.replace("đ", "d").replace("Đ", "D"))
    s = "".join(c for c in s if unicodedata.category(c) != "Mn")
    return re.sub(r"\s+", " ", s.lower()).strip()


def nfc_lower(s: str) -> str:
    s = unicodedata.normalize("NFC", s).lower()
    # gõ dấu kiểu cũ/mới: "uỳ" ↔ "ùy", "oà" ↔ "òa"... đưa về một dạng bằng cách so khớp trên chuỗi đã bỏ dấu
    return re.sub(r"\s+", " ", s)


# Đồng nghĩa ở mức thể loại (khoá đã bỏ dấu).
SYNONYMS = {
    "hoi ki": "hoi ky",
    "but ki": "but ky",
    "du ki": "du ky",
    "tieu su": "tieu su",
    "tu truyen": "hoi ky",
    "tho ca": "tho",
    "thi ca": "tho",
    "sale": "ban hang",
    "sales": "ban hang",
    "truyen dai tron bo": "truyen dai",
    "tap truyen ngan": "truyen ngan",
    "kiem hiep": "kiem hiep",
    "vo hiep": "vo hiep",
}

ZIP_SUFFIX = re.compile(r"-\d{8}T\d{6}Z-\d+-\d+$")


def genre_parts(folder_name: str) -> list[tuple[str, str]]:
    """[(khoá, tên gốc)] theo thứ tự trong tên thư mục."""
    n = unicodedata.normalize("NFC", folder_name).strip()
    n = ZIP_SUFFIX.sub("", n)
    n = re.sub(r"^\s*\[[^\]]*\]\s*", "", n)
    n = re.sub(r"^\s*\d+\s*[.)]\s*", "", n)  # "2. Kinh tế…"
    out, seen = [], set()
    for raw in re.split(r"\s*[-–—,;&/+|]\s*|\s+và\s+", n):
        raw = raw.strip(" .")
        raw = re.sub(r"\s+\d+$", "", raw)  # "Quân Sự 2"
        key = fold(raw)
        if not key:
            continue
        key = SYNONYMS.get(key, key)
        if key not in seen:
            seen.add(key)
            out.append((key, raw))
    return out


# ---------------------------------------------------------------- từ khoá thể loại

# Viết có dấu, chữ thường. Khớp theo ranh giới từ trên chuỗi NFC.
KEYWORDS: dict[str, list[str]] = {
    "tho": ["thơ", "tập thơ", "thi tập", "thi ca", "trường ca", "lục bát", "poetry", "poems", "poem"],
    "hoi ky": ["hồi ký", "hồi kí", "memoir", "memoirs", "autobiography", "tự truyện", "hồi ức", "ký ức", "kí ức"],
    "tieu su": ["tiểu sử", "biography", "biographies", "cuộc đời và sự nghiệp", "chân dung"],
    "but ky": ["bút ký", "bút kí", "ghi chép", "phóng sự"],
    "du ky": ["du ký", "du kí", "travel", "hành trình", "du lịch"],
    "tan van": ["tản văn", "tạp văn", "tạp bút", "essay", "essays", "prose"],
    "tuy but": ["tùy bút", "tuỳ bút", "tản mạn", "essays"],
    "truyen ngan": ["truyện ngắn", "tập truyện", "short stories", "short story"],
    "tieu thuyet": ["tiểu thuyết", "novel", "fiction"],
    "truyen dai": ["truyện dài", "tiểu thuyết", "novel"],
    "trinh tham": ["trinh thám", "detective", "mystery", "crime"],
    "hinh su": ["hình sự", "crime"],
    "kinh di": ["kinh dị", "horror"],
    "kiem hiep": ["kiếm hiệp", "võ hiệp", "wuxia"],
    "tien hiep": ["tiên hiệp", "tu tiên", "xianxia"],
    "lich su": ["lịch sử", "history"],
    "quan su": ["quân sự", "chiến tranh", "military", "war"],
    "ton giao": ["tôn giáo", "phật giáo", "thiền", "religion", "buddhism"],
    "tam linh": ["tâm linh", "spiritual"],
    "triet hoc": ["triết học", "philosophy"],
    "kinh te": ["kinh tế", "economics"],
    "ban hang": ["bán hàng", "sale", "sales", "selling"],
    "marketing": ["marketing", "tiếp thị"],
    "thieu nhi": ["thiếu nhi", "trẻ em", "children"],
}

# "thơ" trong các cụm này không phải thể loại thơ.
NOT_POETRY = ["tuổi thơ", "ấu thơ", "thơ ấu", "ngây thơ", "thơ ngây", "trẻ thơ", "con thơ", "thơ dại", "thơ thẩn", "nhà thơ"]


def kw_regex(words: list[str]) -> re.Pattern:
    alts = sorted({nfc_lower(w) for w in words}, key=len, reverse=True)
    return re.compile(r"(?<!\w)(?:" + "|".join(re.escape(a) for a in alts) + r")(?!\w)")


def strip_not_poetry(text: str) -> str:
    for p in NOT_POETRY:
        text = text.replace(p, " ")
    return text


# ---------------------------------------------------------------- đọc sách

BOOK_EXT = {".epub", ".mobi", ".azw", ".azw3", ".prc", ".pdf", ".docx", ".doc", ".txt", ".fb2", ".djvu", ".cbz", ".cbr", ".lit", ".rtf"}
TEXT_LIMIT = 300_000


def html_to_lines(s: str) -> str:
    s = re.sub(r"(?is)<(script|style|head)\b.*?</\1>", " ", s)
    s = re.sub(r"(?i)<br\s*/?>|</p>|</div>|</h\d>|</li>|</tr>", "\n", s)
    s = re.sub(r"<[^>]+>", " ", s)
    return html.unescape(s)


@dataclass
class Evidence:
    title: str = ""
    subjects: str = ""
    description: str = ""
    text: str = ""


def read_epub(p: Path) -> Evidence:
    ev = Evidence()
    with zipfile.ZipFile(p) as z:
        names = z.namelist()
        opf = next((n for n in names if n.lower().endswith(".opf")), None)
        if opf:
            o = z.read(opf).decode("utf-8", "ignore")

            def tags(t):
                return [html.unescape(re.sub(r"<[^>]+>", " ", m)).strip() for m in re.findall(rf"(?is)<dc:{t}[^>]*>(.*?)</dc:{t}>", o)]

            ev.title = " ".join(tags("title"))
            ev.subjects = "; ".join(tags("subject"))
            ev.description = " ".join(tags("description"))[:4000]
        parts, total = [], 0
        for n in sorted(x for x in names if x.lower().endswith((".xhtml", ".html", ".htm"))):
            s = html_to_lines(z.read(n).decode("utf-8", "ignore"))
            parts.append(s)
            total += len(s)
            if total > TEXT_LIMIT:
                break
        ev.text = "\n".join(parts)
    return ev


def palmdoc_decompress(data: bytes) -> bytes:
    out = bytearray()
    i = 0
    while i < len(data):
        c = data[i]
        i += 1
        if c == 0 or 9 <= c <= 0x7F:
            out.append(c)
        elif 1 <= c <= 8:
            out += data[i : i + c]
            i += c
        elif c >= 0xC0:
            out += b" " + bytes([c ^ 0x80])
        else:
            if i >= len(data):
                break
            v = ((c << 8) | data[i]) & 0x3FFF
            i += 1
            dist, ln = v >> 3, (v & 7) + 3
            for _ in range(ln):
                if dist > len(out) or dist == 0:
                    break
                out.append(out[-dist])
    return bytes(out)


def read_mobi(p: Path) -> Evidence:
    ev = Evidence()
    b = p.read_bytes()
    if len(b) < 78 + 8:
        return ev
    n = struct.unpack(">H", b[76:78])[0]
    offs = [struct.unpack(">I", b[78 + 8 * k : 82 + 8 * k])[0] for k in range(n)] + [len(b)]
    r0 = b[offs[0] : offs[1]]
    comp, _, _, recs = struct.unpack(">HHIH", r0[:10])
    enc = "utf-8"
    if r0[16:20] == b"MOBI":
        if struct.unpack(">I", r0[28:32])[0] == 1252:
            enc = "cp1252"
        hl = struct.unpack(">I", r0[20:24])[0]
        exth = r0[16 + hl :]
        if exth[:4] == b"EXTH":
            cnt = struct.unpack(">I", exth[8:12])[0]
            j = 12
            for _ in range(cnt):
                t, l = struct.unpack(">II", exth[j : j + 8])
                v = exth[j + 8 : j + l].decode(enc, "ignore")
                if t == 503:
                    ev.title = v
                elif t == 105:
                    ev.subjects += ("; " if ev.subjects else "") + v
                elif t == 103:
                    ev.description = v[:4000]
                j += l
        if not ev.title:
            to, tl = struct.unpack(">II", r0[84:92])
            ev.title = r0[to : to + tl].decode(enc, "ignore")
    if comp not in (1, 2):  # HUFF/CDIC – bỏ qua nội dung
        return ev
    trail = struct.unpack(">H", r0[0xF2:0xF4])[0] if len(r0) >= 0xF4 and r0[16:20] == b"MOBI" else 0
    chunks, total = [], 0
    for k in range(1, min(recs, n - 1) + 1):
        rec = b[offs[k] : offs[k + 1]]
        # bỏ phần "trailing entries" (multibyte/TBS) ở cuối bản ghi
        for bit in range(15, 0, -1):
            if trail & (1 << bit):
                sz, m = 0, 0
                for q in range(1, 5):
                    if q > len(rec):
                        break
                    v = rec[-q]
                    sz |= (v & 0x7F) << (7 * (q - 1))
                    m = q
                    if v & 0x80:
                        break
                sz = sz or m
                rec = rec[: len(rec) - sz] if sz <= len(rec) else rec
        if trail & 1 and rec:
            rec = rec[: len(rec) - ((rec[-1] & 3) + 1)]
        chunk = palmdoc_decompress(rec) if comp == 2 else rec
        chunks.append(chunk)
        total += len(chunk)
        if total > TEXT_LIMIT:
            break
    ev.text = html_to_lines(b"".join(chunks).decode(enc, "ignore"))
    return ev


def read_docx(p: Path) -> Evidence:
    with zipfile.ZipFile(p) as z:
        x = z.read("word/document.xml").decode("utf-8", "ignore")
    x = re.sub(r"</w:p>", "\n", x)
    return Evidence(text=html.unescape(re.sub(r"<[^>]+>", "", x))[:TEXT_LIMIT])


def read_evidence(files: list[Path]) -> Evidence:
    order = {".epub": 0, ".docx": 1, ".azw3": 2, ".mobi": 2, ".prc": 2, ".azw": 2, ".txt": 3}
    best = Evidence()
    for f in sorted(files, key=lambda f: order.get(f.suffix.lower(), 9)):
        ext = f.suffix.lower()
        try:
            if ext == ".epub":
                ev = read_epub(f)
            elif ext in (".mobi", ".azw3", ".prc", ".azw"):
                ev = read_mobi(f)
            elif ext == ".docx":
                ev = read_docx(f)
            elif ext == ".txt":
                ev = Evidence(text=f.read_bytes()[:TEXT_LIMIT].decode("utf-8", "ignore"))
            else:
                continue
        except Exception:
            continue
        for a in ("title", "subjects", "description", "text"):
            if not getattr(best, a) and getattr(ev, a):
                setattr(best, a, getattr(ev, a))
        if best.text and best.title:
            break
    return best


def verse_ratio(text: str) -> float | None:
    """Tỉ lệ dòng 'giống câu thơ': 3–12 từ, không phải lời thoại, không kết bằng dấu câu."""
    lines = [re.sub(r"\s+", " ", l).strip() for l in text.split("\n")]
    lines = [l for l in lines if l]
    if len(lines) < 80:
        return None
    v = 0
    for l in lines:
        w = len(l.split())
        if 3 <= w <= 12 and not l.startswith(("-", "–", "—")) and not l.endswith((".", "?", "!", ":", '"', "”", "…")):
            v += 1
    return v / len(lines)


# ---------------------------------------------------------------- mô hình kho


@dataclass
class Item:
    """Một tác phẩm: mọi file cùng tên gốc (kể cả ảnh bìa / .opf) hoặc một thư mục con."""

    name: str
    paths: list[Path]
    is_dir: bool = False
    _ev: Evidence | None = None

    def evidence(self) -> Evidence:
        if self._ev is None:
            if self.is_dir:
                inner = [p for p in self.paths[0].rglob("*") if p.is_file() and p.suffix.lower() in BOOK_EXT][:6]
                self._ev = read_evidence(inner)
            else:
                self._ev = read_evidence(self.paths)
            if not self._ev.title:
                self._ev.title = self.name
        return self._ev


@dataclass
class Folder:
    name: str
    path: Path  # nơi sẽ nằm trên đĩa (có thể chưa tồn tại)
    parts: list[tuple[str, str]]
    items: list[Item] = field(default_factory=list)
    is_main_root: bool = True
    exists: bool = True

    @property
    def genres(self) -> frozenset[str]:
        return frozenset(k for k, _ in self.parts)


def list_items(d: Path) -> list[Item]:
    groups: dict[str, list[Path]] = {}
    dirs = []
    for e in sorted(d.iterdir(), key=lambda p: p.name.lower()):
        if e.is_dir():
            dirs.append(Item(e.name, [e], is_dir=True))
        elif e.is_file():
            stem = e.name
            while True:
                s, ext = os.path.splitext(stem)
                if ext and len(ext) <= 6 and s:
                    stem = s
                    if ext.lower() in BOOK_EXT:
                        break
                else:
                    break
            groups.setdefault(stem.lower(), []).append(e)
    files = [Item(v[0].stem if len(v) == 1 else os.path.commonprefix([p.name for p in v]).rstrip(". ") or v[0].stem, v) for v in groups.values()]
    return files + dirs


DEFAULT_EXCLUDE = ["calibre library", "chua phan loai", "$recycle.bin", "system volume information", ".trashes"]


def load_folders(root: Path, also: list[Path], exclude: list[str]) -> list[Folder]:
    ex = {fold(x) for x in DEFAULT_EXCLUDE + exclude}
    out = []
    for base, is_main in [(root, True)] + [(a, False) for a in also]:
        for e in sorted(base.iterdir()):
            if not e.is_dir() or fold(e.name) in ex or e.name.startswith((".", "$")):
                continue
            parts = genre_parts(e.name)
            if not parts:
                continue
            out.append(Folder(e.name, e, parts, list_items(e), is_main_root=is_main))
    return out


# ---------------------------------------------------------------- chấm điểm


def genre_keywords(folders: list[Folder]) -> dict[str, re.Pattern]:
    words: dict[str, set[str]] = {}
    for f in folders:
        for k, raw in f.parts:
            words.setdefault(k, set()).update(KEYWORDS.get(k, []))
            words[k].add(raw)
    return {k: kw_regex(sorted(v)) for k, v in words.items()}


def score_item(item: Item, genre: str, rx: re.Pattern) -> tuple[float, str]:
    ev = item.evidence()
    title = nfc_lower(ev.title + " " + item.name)
    subj = nfc_lower(ev.subjects)
    desc = nfc_lower(ev.description)
    text = nfc_lower(ev.text[:40_000])
    if genre == "tho":
        title, subj, desc, text = map(strip_not_poetry, (title, subj, desc, text))
    best, why = 0.0, ""

    def bump(v, w):
        nonlocal best, why
        if v > best:
            best, why = v, w

    if rx.search(title):
        bump(1.0, "tên sách")
    elif genre in ("tho",) and re.search(r"(?<!\w)(tap tho|thi tap|truong ca)(?!\w)", fold(item.name)):
        bump(1.0, "tên file")
    elif genre != "tho" and kw_regex([fold(w) for w in KEYWORDS.get(genre, [])] or [genre]).search(fold(item.name)):
        bump(0.9, "tên file")
    if rx.search(subj):
        bump(0.9, "chủ đề metadata")
    if rx.search(desc):
        bump(0.6, "mô tả")
    hits = len(rx.findall(text))
    if genre == "tho":
        vr = verse_ratio(ev.text)
        if vr is not None:
            if vr >= 0.45:
                bump(1.0, f"nội dung dạng thơ ({vr:.0%} dòng)")
            elif vr >= 0.35:
                bump(0.55, f"nhiều dòng ngắn ({vr:.0%})")
            elif vr >= 0.3 and hits >= 4:
                bump(0.55, f"dòng ngắn ({vr:.0%}) + nhắc 'thơ' {hits} lần")
            elif vr < 0.2 and best < 1.0:
                best, why = min(best, 0.3), why  # văn xuôi rõ ràng
        if hits >= 4 and best < 0.5:
            bump(0.4, f"nội dung nhắc 'thơ' {hits} lần")
    elif hits >= 2:
        bump(min(0.75, 0.35 + 0.1 * hits), f"nội dung nhắc {hits} lần")
    return best, why


# ---------------------------------------------------------------- lập kế hoạch


@dataclass
class Move:
    item: Item
    src_folder: str
    dst_folder: Path
    confidence: str
    reason: str


def plan(root: Path, also: list[Path], exclude: list[str], log=print) -> tuple[list[Move], list[str]]:
    folders = load_folders(root, also, exclude)
    rx = genre_keywords(folders)
    notes: list[str] = []
    final: dict[int, tuple[Folder, str, str]] = {}  # id(item) -> (đích, độ tin, lý do)
    origin: dict[int, tuple[Item, Folder]] = {}
    for f in folders:
        for it in f.items:
            origin[id(it)] = (it, f)
            final[id(it)] = (f, "", "")

    def assign(it, dst, conf, why):
        final[id(it)] = (dst, conf, why)
        dst.items.append(it)

    changed = True
    while changed:
        changed = False
        live = [f for f in folders if f.items or f.exists]
        # 1) cùng bộ thể loại → gộp vào thư mục ở gốc chính / nhiều sách hơn
        by_set: dict[frozenset, list[Folder]] = {}
        for f in live:
            by_set.setdefault(f.genres, []).append(f)
        for fs in by_set.values():
            if len(fs) < 2:
                continue
            keep = max(fs, key=lambda f: (f.is_main_root, f.exists, len(f.items), -len(f.name)))
            for f in fs:
                if f is keep:
                    continue
                notes.append(f"Gộp “{f.path}” → “{keep.name}” (cùng thể loại)")
                for it in f.items:
                    assign(it, keep, "cao", "thư mục cùng thể loại")
                f.items = []
                folders.remove(f)
                changed = True
        if changed:
            continue
        # 2) thư mục gộp nhiều thể loại mà một phần đã có thư mục riêng → tách
        for f in sorted(folders, key=lambda f: (len(f.genres), f.name)):
            if len(f.genres) < 2 or not f.items:
                continue
            subs = [g for g in folders if g is not f and g.genres < f.genres]
            subs = [g for g in subs if not any(g.genres < h.genres for h in subs)]  # chỉ lấy tập con lớn nhất
            if not subs:
                continue
            covered = frozenset().union(*(g.genres for g in subs))
            left = [(k, raw) for k, raw in f.parts if k not in covered]
            targets: list[Folder] = list(subs)
            rest: Folder | None = None
            if left:
                rest_name = " - ".join(raw for _, raw in left)
                parent = root if not f.is_main_root else f.path.parent
                rest = Folder(rest_name, parent / rest_name, left, [], True, exists=(parent / rest_name).exists())
                targets.append(rest)
            # mặc định: nhóm có nhiều thể loại nhất (thường chứa nhiều sách nhất); hoà → thư mục đã có
            default = max(targets, key=lambda t: (len(t.genres), t is not rest))
            # thư mục có sẵn giao một phần thể loại (vd "Bút Ký - Hồi Ký" với "Bút Ký - Truyện Ngắn…"):
            # chỉ nhận sách khi tên/metadata nói rõ thể loại riêng của nó
            overlaps = [g for g in folders if g is not f and g not in targets and g.genres & f.genres and g.exists]
            notes.append(
                f"Tách “{f.name}” → " + ", ".join(f"“{t.name}”" for t in targets)
                + (f"  (đổi tên phần còn lại thành “{rest.name}”)" if rest else "  (giải thể)")
            )
            log(f"  đang đọc {len(f.items)} tác phẩm trong “{f.name}”…")
            items, f.items = f.items, []
            for it in items:
                scored = []
                for t in targets:
                    s, why = max((score_item(it, g, rx[g]) for g in t.genres), key=lambda x: x[0])
                    scored.append((s, why, t))
                scored.sort(key=lambda x: (-x[0], x[2] is not default))
                s1, why1, t1 = scored[0]
                s2 = scored[1][0] if len(scored) > 1 else 0.0
                strong = []
                for g in overlaps:
                    own = g.genres - f.genres
                    if own:
                        s, why = max((score_item(it, k, rx[k]) for k in own), key=lambda x: x[0])
                        if s >= 0.9 and s > s1:
                            strong.append((s, why, g))
                if strong:
                    s, why, g = max(strong, key=lambda x: x[0])
                    assign(it, g, "vừa", f"{why} → thư mục có sẵn “{g.name}”")
                elif s1 >= 0.5 and s1 > s2:
                    conf = "cao" if s1 >= 0.9 and s2 < 0.6 else "vừa"
                    assign(it, t1, conf, why1 + (f" (cạnh tranh: {s2:.1f})" if s2 >= 0.5 else ""))
                else:
                    assign(it, default, "thấp", "không đủ dấu hiệu → nhóm mặc định" + (f" ({why1})" if why1 else ""))
            folders.remove(f)
            if rest:
                folders.append(rest)
            changed = True
            break

    moves = []
    for iid, (it, src) in origin.items():
        dst, conf, why = final[iid]
        if dst.path != src.path:
            moves.append(Move(it, src.name if src.is_main_root else str(src.path), dst.path, conf, why))
    return moves, notes


# ---------------------------------------------------------------- CSV / thực thi

CSV_COLS = ["di_chuyen", "do_tin_cay", "tac_pham", "tu_thu_muc", "den_thu_muc", "ly_do", "files"]


def write_plan(moves: list[Move], out: Path):
    with out.open("w", newline="", encoding="utf-8-sig") as fh:
        w = csv.writer(fh)
        w.writerow(CSV_COLS)
        order = {"thấp": 0, "vừa": 1, "cao": 2}
        for m in sorted(moves, key=lambda m: (m.src_folder, order.get(m.confidence, 3), m.item.name.lower())):
            w.writerow(["x", m.confidence, m.item.name, m.src_folder, str(m.dst_folder), m.reason, " | ".join(str(p) for p in m.item.paths)])


def unique_dest(dst: Path) -> Path:
    if not dst.exists():
        return dst
    stem, ext = (dst.name, "") if dst.is_dir() else (dst.stem, dst.suffix)
    k = 2
    while True:
        c = dst.with_name(f"{stem} ({k}){ext}")
        if not c.exists():
            return c
        k += 1


def same_file(a: Path, b: Path) -> bool:
    if not (a.is_file() and b.is_file()) or a.stat().st_size != b.stat().st_size:
        return False
    with a.open("rb") as x, b.open("rb") as y:
        while True:
            u, v = x.read(1 << 20), y.read(1 << 20)
            if u != v:
                return False
            if not u:
                return True


def apply_plan(root: Path, plan_csv: Path):
    rows = list(csv.DictReader(plan_csv.open(encoding="utf-8-sig")))
    stamp = time.strftime("%Y%m%d-%H%M%S")
    undo_path = plan_csv.with_name(f"merge-undo-{stamp}.jsonl")
    moved = skipped = dup = 0
    touched: set[Path] = set()
    with undo_path.open("w", encoding="utf-8") as undo:
        for r in rows:
            if (r.get("di_chuyen") or "").strip().lower() not in ("x", "1", "y", "yes", "co", "có"):
                skipped += 1
                continue
            dst_dir = Path(r["den_thu_muc"].strip())
            if not dst_dir.is_absolute():
                dst_dir = root / dst_dir
            if not dst_dir.exists():
                dst_dir.mkdir(parents=True)
                undo.write(json.dumps({"mkdir": str(dst_dir)}, ensure_ascii=False) + "\n")
            for s in [Path(p.strip()) for p in r["files"].split(" | ") if p.strip()]:
                if not s.exists():
                    print(f"  ! không còn: {s}")
                    continue
                target = dst_dir / s.name
                if target.exists() and same_file(s, target):
                    print(f"  = đã có bản giống hệt ở đích, để nguyên: {s}")
                    dup += 1
                    continue
                target = unique_dest(target)
                os.replace(s, target) if s.drive == target.drive else __import__("shutil").move(str(s), str(target))
                undo.write(json.dumps({"src": str(s), "dst": str(target)}, ensure_ascii=False) + "\n")
                undo.flush()
                touched.add(s.parent)
                moved += 1
        for d in sorted(touched, key=lambda p: -len(str(p))):
            try:
                if d.exists() and not any(d.iterdir()) and d != root:
                    d.rmdir()
                    undo.write(json.dumps({"rmdir": str(d)}, ensure_ascii=False) + "\n")
                    print(f"  − xoá thư mục rỗng: {d}")
            except OSError:
                pass
    print(f"\nĐã chuyển {moved} file, bỏ qua {skipped} dòng không đánh dấu, {dup} file trùng để nguyên.")
    print(f"Hoàn tác: python -I scripts/merge-categories.py undo \"{undo_path}\"")
    print("Nhớ bấm Quét lại trong My Librarian để cập nhật thư viện.")


def undo(log: Path):
    ops = [json.loads(l) for l in log.open(encoding="utf-8") if l.strip()]
    import shutil

    n = 0
    for op in reversed(ops):
        if "rmdir" in op:
            Path(op["rmdir"]).mkdir(parents=True, exist_ok=True)
        elif "mkdir" in op:
            p = Path(op["mkdir"])
            try:
                p.rmdir()
            except OSError:
                print(f"  ! không xoá được (không rỗng): {p}")
        else:
            s, d = Path(op["src"]), Path(op["dst"])
            if d.exists() and not s.exists():
                s.parent.mkdir(parents=True, exist_ok=True)
                shutil.move(str(d), str(s))
                n += 1
            else:
                print(f"  ! bỏ qua: {d}")
    print(f"Đã trả {n} file về chỗ cũ.")


# ---------------------------------------------------------------- CLI


def main():
    ap = argparse.ArgumentParser(description="Gộp thư mục thể loại trùng/lồng nhau")
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("plan", help="lập kế hoạch, không di chuyển gì")
    p.add_argument("root", type=Path)
    p.add_argument("--also", type=Path, action="append", default=[], help="thư mục nguồn khác (vd D:\\books\\dtv books) có thư mục thể loại con cần gộp vào kho")
    p.add_argument("--exclude", action="append", default=[], help="tên thư mục bỏ qua")
    p.add_argument("-o", "--out", type=Path, default=None, help="file CSV kế hoạch (mặc định merge-plan.csv cạnh script)")
    a = sub.add_parser("apply", help="thực hiện theo CSV")
    a.add_argument("root", type=Path)
    a.add_argument("plan", type=Path)
    u = sub.add_parser("undo", help="hoàn tác")
    u.add_argument("log", type=Path)
    args = ap.parse_args()

    if args.cmd == "plan":
        out = args.out or Path.cwd() / "merge-plan.csv"
        moves, notes = plan(args.root, args.also, args.exclude)
        print("\nKế hoạch:")
        for n in notes:
            print("  •", n)
        if not moves:
            print("Không có gì cần gộp.")
            return
        by: dict[tuple[str, str], list[Move]] = {}
        for m in moves:
            by.setdefault((m.src_folder, m.dst_folder.name), []).append(m)
        print()
        for (s, d), ms in sorted(by.items()):
            c = {k: sum(1 for m in ms if m.confidence == k) for k in ("cao", "vừa", "thấp")}
            print(f"  {s}  →  {d}: {len(ms)} tác phẩm (tin cậy cao {c['cao']}, vừa {c['vừa']}, thấp {c['thấp']})")
            for m in sorted(ms, key=lambda m: m.item.name.lower()):
                print(f"      [{m.confidence:>4}] {m.item.name}  — {m.reason}")
        write_plan(moves, out)
        print(f"\nĐã ghi kế hoạch: {out}")
        print("Mở bằng Excel, sửa cột den_thu_muc nếu cần (xoá chữ x ở cột di_chuyen để bỏ qua), rồi chạy:")
        print(f'  python -I scripts/merge-categories.py apply "{args.root}" "{out}"')
    elif args.cmd == "apply":
        apply_plan(args.root, args.plan)
    else:
        undo(args.log)


if __name__ == "__main__":
    main()
