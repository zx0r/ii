# Фазы рефакторинга `ii` → Hourglass IR + DOD

> Каждая фаза — атомарная; после каждого шага обязаны проходить:
> `cargo test --all-features && cargo clippy --all-targets --all-features -- -D warnings`

---

## Фаза 0: Санитарная зачистка

### 0.0 — Fix `Cmd::from_u16` Result mismatch в `daemon.rs`
- `daemon.rs:522`: `let Some(cmd) = Cmd::from_u16(header.command)` — не компилируется, `from_u16` → `Result`, не `Option`.
- Исправить на `let Ok(cmd) = Cmd::from_u16(header.command)`.

### 0.1 — НЕ удалять DAG из `layout.rs`
- **Отменено.** `DagNode`/`DagEdge`/`assign_ranks`/`layout_dag_coordinates` остаются в `layout.rs`.
- В Фазе 2 графовый layout из `fmt/dot.rs` → переедет В `layout.rs` (переиспользование с mermaid).

### 0.2 — Удалить дубль `trim_ascii` из `fmt/md.rs`
- `md.rs:96-114` дублирует `fmt/mod.rs:23-39`.
- Привести `fmt/mod.rs::trim_ascii` к `is_ascii_whitespace()`, удалить дубль из `md.rs`.

### 0.3 — `ipc::encode_error` → `cmd: u16, msg: &[u8]`; удалить `daemon::encode_error`
- Изменить сигнатуру `ipc::encode_error` на `(cmd: u16, msg: &[u8], out: &mut Vec<u8>)`.
- Удалить `daemon.rs::encode_error` (549–565), заменить все вызовы на `ipc::encode_error`.

### 0.4 — `FORMAT_REGISTRY` — оставить, подключить в Фазе 2

---

## Фаза 1: Сжатие `fmt/md.rs` + `fmt/json.rs`

### 1.1 — Вынести `Span`/`Kind` из `fmt/md.rs` → `scene.rs`
- `Kind`, `Span`, `OFFSET_MASK`, `KIND_SHIFT` — общие IR-типы, нужные `tty.rs`, `daemon.rs`.
- Перенести в `scene.rs`, чтобы `tty`/`daemon` не зависели от `fmt`.

### 1.2 — Вынести тесты `md.rs` в `tests/`
- Тест `monotonic_non_overlapping_spans` (и будущие интеграционные) → `tests/scan.rs`.
- Unit-тесты вспомогательных функций оставить in-place.

### 1.3 — `fmt/json.rs` 340 → ≤250 SLOC
- Вынести тесты в `tests/json_nav.rs`.
- Оценить: `str_of()` unescape (40 SLOC) → можно ли сжать match-каскад.

---

## Фаза 2: Ingest ≠ Render — двухслойная архитектура

### 2.1 — `chart/csv/dot/font` → чистые `&[u8] → &mut Scene`
- **chart.rs**: парсинг Vega-Lite/Plotly JSON → серии `Datum` → `Scene::push_series_line`, `Scene::push_rect`, `Scene::push_text`. Убрать `&Palette`, `&mut String`, SVG-генерацию. Layout (оси, тики, координаты) → считаются в `layout.rs`, не в `fmt/`.
- **csv.rs**: CSV/TSV/pipe-table → `Scene::grids` + `Scene::cells` + `Scene::col_widths`.
- **dot.rs**: DOT парсинг → узлы + рёбра → передаёт в `layout.rs` (Sugiyama) → `Scene` rects + paths + glyphs. `dot.rs` остаётся чистым парсером.
- **font.rs**: Specimen → `Scene` rects + glyphs.

### 2.2 — Графовый layout из `dot.rs` → `layout.rs`
- Перенести локальные `Node`/`Edge`/rank assignment/coordinate computation из `dot.rs` в `layout.rs`.
- `layout.rs` уже содержит `DagNode`/`DagEdge`/`assign_ranks`/`layout_dag_coordinates` — унифицировать с dot.rs стековой реализацией.
- Результат: `dot.rs` — только парсинг DOT → вызов `layout::*`, `mermaid.rs` — тоже может использовать.

### 2.3 — Подключить парсеры в `FORMAT_REGISTRY`
- Заменить `parse_noop` на реальные `csv::parse`, `dot::parse`, etc.

### ~~2.3 (old)~~ — ~~render.rs Scene→SVG~~ ОТМЕНЕНО
- Нет промежуточного SVG. Pipeline: `Scene` → `layout.rs` → `raster::rasterize_scene` (tiny-skia) напрямую.

---

## Фаза 3: Консолидация хеширования и кеша

- `cache.rs` (BLAKE3 L2) + `vram.rs` (wymix L0) — разные алгоритмы для разных уровней. **Оставить как есть.**
- ~~3.2 единый HEX LUT~~ ОТМЕНЕНО — не ломать DAG ради одной строки.

---

## Фаза 4: Wire-protocol cleanup

### 4.1 → перенесён в 0.0

### 4.2 — `daemon.rs:dispatch_and_respond` вызывает `scratch.reset()` дважды
- `daemon.rs:521` и `daemon.rs:506`. Убрать один.

---

## Фаза 5: Полный pipeline в `daemon.rs`

### 5.1 — `handle_render_file` → Ingest→Layout→Raster→KGP
1. `dispatch_into_scene(buf, ext, scene)` ✓
2. `raster::rasterize_scene(scene, cw, ch, max_w, rgba_buf)` → KGP staging
3. `kitty::stage_shm/stage_base64(rgba, params, apc)` → APC
4. `tty::render_ansi(src, spans, dark, text)` → ANSI text
5. `ipc::encode_dual_response(cmd, apc, text, out)` → response

### 5.2 — `daemon.rs` принять `tty::Metrics` при подключении клиента

---

## Фаза 6: Оптимизация горячего пути

### 6.1 — `Scene::text_pool`: `Vec<u8>` → `String`
- Вместо `unsafe from_utf8_unchecked` — сменить тип `text_pool` на `String`.
- `push_text` → `self.text_pool.push_str(text)`.
- `text()` / `cell_text()` → `&self.text_pool[start..end]` без `from_utf8`.

### 6.3 — `rasterize_scene` без `checked_mul`
- `raster.rs:126` — добавить `checked_mul` для consistency с SVG rasterizer.

### 6.4 — Стековый `b64_name` в `kitty.rs`
- `kitty.rs:249` — `Vec::with_capacity(32)` → `[u8; 32]` стековый буфер.

---

## Фаза 7: Финальная гигиена

### 7.1 — Удалить `theme::DARK`/`theme::LIGHT` alias-дубли
- Оставить только `DARK_PALETTE`/`LIGHT_PALETTE`.

### 7.2 — `Palette::hex_str` — проверить необходимость
- После удаления SVG-генерации из fmt/* (Фаза 2), проверить остались ли вызовы `hex_str`.
- Если нет — удалить `hex_str`, `series_hex_str`, `DARK_HEX`, `SERIES_HEX` целиком.
- Если да — добавить `LIGHT_HEX` для корректности.

### 7.3 — `tty.rs:165` — `query_fg` dead code

### 7.4 — `b64_buf` в `RenderScratch`

---

## Порядок исполнения

```
Phase 0 (Sanitize)
  ├── 0.0  Fix Cmd::from_u16 Result in daemon.rs
  ├── 0.2  trim_ascii dedup
  └── 0.3  ipc::encode_error → u16, delete daemon::encode_error
       │
Phase 1 (Compress md.rs + json.rs)
  ├── 1.1  Span/Kind → scene.rs
  ├── 1.2  md.rs tests → tests/
  └── 1.3  json.rs compress
       │
Phase 2 (Ingest ≠ Render)
  ├── 2.1  chart/csv/dot/font → Scene-only
  ├── 2.2  dot.rs graph layout → layout.rs
  └── 2.3  FORMAT_REGISTRY подключение
       │
Phase 4 (Wire cleanup)
  └── 4.2  двойной scratch.reset()
       │
Phase 5 (Pipeline)
  ├── 5.1  полный render pipeline в daemon
  └── 5.2  прокидывание Metrics от клиента
       │
Phase 6 (Hot path)
  ├── 6.1  text_pool: Vec<u8> → String
  ├── 6.3  checked_mul в rasterize_scene
  └── 6.4  стековый b64_name в kitty
       │
Phase 7 (Final polish)
  ├── 7.1  DARK/LIGHT alias cleanup
  ├── 7.2  hex_str audit
  ├── 7.3  query_fg dead code
  └── 7.4  b64_buf в RenderScratch
```
