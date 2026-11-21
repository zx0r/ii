# АРХИТЕКТУРНАЯ СПЕЦИФИКАЦИЯ: `ii` PURE STATELESS STATE RELAY (PRINCIPAL UNIX STANDARD)
**Статус:** Канонический стандарт архитектуры и реализации  
**Уровень:** Principal Systems Architect  
**Стек:** Kitty Graphics Protocol (`U+10EEEE`, `a=p, U=1`), Tmux 3.4+ DCS Passthrough, Neovim, Yazi  
**Транспорт:** Полностью автономный (POSIX SHM `t=s` локально, чанкованный Base64 `t=d` при SSH/удаленном сеансе без ручных флагов)  
**Геометрия:** Строго парные флаги `-s, --size <WxH>` и `--cell-size <WxH>`  
**Философия:** Чистый Unix-инструмент. Zero UI State, Zero TTY I/O Side-Effects, Zero Subprocesses, Zero Bloat.

---

## 1. Архитектурный манифест: Pure Stateless State Relay

`ii` — это высокоскоростной графический сопроцессор, транслирующий входящий поток документов, формул, диаграмм, таблиц и графиков в квантованный по символьной сетке фрейм Kitty Graphics Protocol (KGP).

### 1.1. Инварианты разделения ответственности (SoC)

1. **Истинный Stateless Relay (Zero Terminal State Tracking):**
   - Сопроцессор **не отслеживает VRAM терминала** и не ведет спекулятивных таблиц присутствия текстур в GPU.
   - Терминальная VRAM принадлежит исключительно эмулятору и хост-клиенту (Neovim / Yazi). При смене сплита, смене сессии, рестарте клиента или `tmux detach/attach` VRAM терминала может быть сброшена в любой момент (`a=d, d=a`).
   - `ii` работает строго идемпотентно: на каждый запрос генерирует полную пару — команду загрузки текстуры (с детерминированным `image_id` на базе BLAKE3-хэша) и сетку плейсхолдеров `U+10EEEE`.
   - Внутренний кэш `ii` ограничен исключительно **L1 RAM Cache** (готовые декодированные RGBA-буферы в памяти процесса) и **L2 Disk Cache** (BLAKE3 + LZ4/PX03 на диске). Никакого VRAM-трекинга в демоне!

2. **Абсолютный запрет на запись в `/dev/tty` во всех режимах:**
   - `ii` — **строгий Unix-фильтр** (`stdin / file -> stdout`).
   - `ii` **никогда не открывает `/dev/tty` на запись** (`O_WRONLY` / `O_RDWR`).
   - Весь вывод направляется исключительно в дескриптор `stdout` (или в сокетное соединение IXPC v1).
   - Клиент (Yazi / Neovim) сам владеет своим TTY и сам решает, куда сбрасывать управляющие байты, а куда вставлять текст.

3. **Автономный транспорт данных (Zero Transport Flags):**
   - Никаких ручных флагов вроде `--no-shm` или `--shm`. Выбор метода передачи текстуры в GPU выполняется **100% автоматически**:
     - В CLI-режиме: `ii` проверяет переменные `$SSH_CLIENT`, `$SSH_CONNECTION`, `$TMUX` собственного процесса.
     - В IXPC v1 (демон): клиент передает биты состояния (`FLAG_TMUX`, `FLAG_REMOTE`) в 32-байтном заголовке запроса. Демон форматирует KGP строго под окружение вызывающего клиента:
       - Локальная сессия: `POSIX SHM` (`t=s`, `mmap` + RAII unlink).
       - SSH / удаленная сессия / отсутствие SHM: прозрачный fallback на чанкованный Base64 (`t=d`, 4096 B).
       - Если клиент в tmux (`FLAG_TMUX`): оборачивание в DCS passthrough (`\ePtmux;\e\e_G...\e\e\\\e\\`).

4. **Zero-Probe & Zero-Guesswork Policy (Вся геометрия — чистое входящее состояние):**
   - Ядро сопроцессора никогда не порождает дочерних процессов (`Command::new(...)` запрещены) и **не выполняет эвристического зондирования TTY через escape-последовательности (`CSI 16t`, `OSC 11`) во время пайп-сессий**.
   - Вся геометрия (`cols, rows`, `cell_w, cell_h`, `theme`) является **чистым входящим состоянием**, которое клиент (Yazi, Neovim) уже знает нативно в своей памяти.
   - Клиент передает состояние напрямую через флаги (`--size`, `--cell-size`, `--theme`) или бинарный заголовок IXPC v1. Сопроцессор работает как чистое функциональное ядро: `f(document, GeometryState) -> (TextureAPC, TextGrid)`.

5. **Zero Bloat & No Legacy Fallbacks:**
   - Полный отказ от растеризации в псевдографику / ANSI-полублоки (`▀▄`), Sixel или iTerm2. `ii` — бескомпромиссный сопроцессор Kitty Graphics Protocol (`U=1, U+10EEEE`).
   - Исключены любые встроенные инсталляторы (`init`), интерактивные панели (`doctor`), эвристические «залечиватели» Markdown (`heal.rs`) и ANSI-пейджеры.

---

## 2. Математика автоматического масштабирования и холста

### 2.1. Определения и входные переменные

- $intrinsic\_w, intrinsic\_h \in \mathbb{R}^+$: Натуральные габариты векторного объекта в CSS-пикселях (96 DPI), извлеченные из `viewBox` или нормализованных атрибутов `width`/`height` SVG (`pt \times 1.333`, `pc \times 16`, `in \times 96`, `mm \times 3.78`).
- $cols, rows \in \mathbb{N}^+$: Размеры области отображения в символьных ячейках, переданные через `-s, --size <COLSxROWS>`.
- $cell\_w, cell\_h \in \mathbb{N}^+$: Физические размеры одной символьной ячейки в пикселях, переданные через `--cell-size <WxH>`.
- Пределы вьюпорта в физических пикселях:
  $$max\_px\_w = cols \times cell\_w, \quad max\_px\_h = rows \times cell\_h$$

---

### 2.2. Масштабирование контента

#### А. Векторные диаграммы и графы (SVG, Mermaid, DOT, Vega-Lite):
Строгий `scale-down` ($scale \le 1.0$), исключающий раздувание и билинейное размытие мелких схем и иконок:
$$scale = \min\left(1.0,\; \frac{max\_px\_w}{intrinsic\_w},\; \frac{max\_px\_h}{intrinsic\_h}\right)$$
- Если схема меньше доступной области — она рендерится строго $1:1$ в нативном разрешении без апскейла.
- Если схема шире или выше вьюпорта — пропорционально сжимается с сохранением строгой изометрии.

#### Б. Математические формулы (LaTeX / Typst):
Масштабирование относительно высоты строчного моноширинного шрифта терминала (Font-Relative Baseline Scaling):
Высота строчного символа терминального моноширинного шрифта составляет ровно $0.75 \times cell\_h$. Стандартный кегль Computer Modern 10pt в `ratex` строится на базе $16.0\text{ px}$.

Единый канонический масштаб математики:
$$scale_{math} = \frac{cell\_h \text{ as } f32}{16.0}$$
С учетом ограничений по ширине строки:
$$scale = \min\left(scale_{math},\; \frac{max\_px\_w}{intrinsic\_w}\right)$$

> [!NOTE]
> На Retina / HiDPI дисплеях (где физическая высота ячейки $cell\_h = 32..48\text{ px}$), $scale_{math} > 1.0$ (например, $36.0 / 16.0 = 2.25$). Формулы масштабируются пропорционально физическому кеглю окружающего текста ячейки, а не фиксируются на 1.0. Ограничение $scale \le 1.0$ относится исключительно к векторным диаграммам (п. А).

---

### 2.3. Математика Canvas Snapped Allocation (Инвариант защиты от искажений KGP GPU)

Kitty Graphics Protocol масштабирует переданную текстуру силами GPU на весь прямоугольник ячеек $cols \times rows$. Чтобы исключить анизотропное растяжение кругов в эллипсы:

1. Вычисляются физические размеры содержимого:
   $$content\_w = \min(max\_px\_w,\; \lceil intrinsic\_w \times scale \rceil)$$
   $$content\_h = \min(max\_px\_h,\; \lceil intrinsic\_h \times scale \rceil)$$

2. Вычисляется квантованная сетка символьных ячеек:
   $$grid\_cols = \left\lceil \frac{content\_w}{cell\_w} \right\rceil, \quad grid\_rows = \left\lceil \frac{content\_h}{cell\_h} \right\rceil$$

3. Физический размер холста (Canvas Frame):
   $$snap\_w = grid\_cols \times cell\_w, \quad snap\_h = grid\_rows \times cell\_h$$

4. Смещение контента внутри холста (центрирование):
   $$off\_x = \left\lfloor \frac{snap\_w - content\_w}{2} \right\rfloor, \quad off\_y = \left\lfloor \frac{snap\_h - content\_h}{2} \right\rfloor$$

5. **Фон холста:**
   - Холст **безусловно инициализируется прозрачным цветом `RGBA(0, 0, 0, 0)`** (unpremultiplied).
   - Это гарантирует, что нативный фон терминала (включая прозрачность, блюр и темы) просвечивает сквозь поля без артефактов и рамок.
   - Отрисовка векторного содержимого выполняется со сдвигом:
     $$\text{Transform} = \text{Transform::from\_scale}(scale, scale).\text{post\_translate}(off\_x, off\_y)$$

---

## 3. Нативное состояние геометрии: Native Client State Relay (Zero-Probe)

В архитектуре **Pure State Relay** сопроцессор `ii` полностью освобожден от логики зондирования терминала, гонок за TTY и таймаутов `poll(2)`.
Вся геометрия является **чистым состоянием**, которым уже владеет клиент (хост-приложение):

```text
┌─────────────────────────────────────────────────────────────────────────────┐
│                    КЛИЕНТСКИЙ СЛОЙ (Yazi / Neovim)                          │
│                                                                             │
│  • Знает границы окон, сплитов, отступы (textoff, border, margins)         │
│  • Владеет жизненным циклом TTY и перерисовкой UI                           │
│  • Кэширует cell_w / cell_h на уровне своего runtime / VimResized           │
│                                                                             │
│  Yazi:   job.area.w, job.area.h, rt.term.cell_size(), rt.term.light()       │
│  Neovim: nvim_win_get_width - textoff, nvim_win_get_height,                 │
│          cached ioctl(1, TIOCGWINSZ), vim.o.background                      │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │
                  Чистое состояние     │  --size <cols>x<rows>
                  без побочных         │  --cell-size <cell_w>x<cell_h>
                  эффектов             │  --theme <dark|light>
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                       `ii` PURE STATELESS RELAY                             │
│                                                                             │
│  • ZERO TTY I/O (никаких открытий /dev/tty, никаких CSI 16t в пайпе)       │
│  • ZERO Subprocesses (никаких дочерних процессов tmux/sh)                   │
│  • Чистая математика: snap_w = cols * cell_w, content_w = min(max_w, ...)   │
│  • Идемпотентный результат: [APC Texture Load] + [U+10EEEE Grid]            │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 3.1. Нативное извлечение состояния в Yazi (Zero Logic)
Yazi в версиях 0.3+ / 2024+ реализует нативное определение параметров терминала в ядре (`yazi-emulator::Dimension`) и транслирует их в Lua через пространство `rt`:
1. **Сетка вьюпорта (`cols`, `rows`):**
   `job.area.w` и `job.area.h` — точные размеры области предпросмотра в символах.
2. **Физический размер ячейки (`cell_w`, `cell_h`):**
   `rt.term.cell_size()` возвращает пару `(cw, ch)` (например, `9.0, 18.0`). Значение инициализируется ядром Yazi при старте и доступно синхронно в ОЗУ.
3. **Полярность палитры (`theme`):**
   `rt.term.light()` возвращает `boolean` (`true` для светлой темы, `false` для темной).
4. **Смещение скролла (`skip`):**
   `job.skip` — вертикальное смещение строк для пейджинга.

```lua
-- Yazi: извлечение чистого состояния за 0 syscalls в плагине:
local cols, rows = job.area.w, job.area.h
local cw, ch = rt.term.cell_size()
local cell_size_str = (cw and ch) and string.format("%dx%d", math.floor(cw), math.floor(ch)) or "9x18"
local theme_str = rt.term.light() and "light" or "dark"
```

### 3.2. Нативное извлечение состояния в Neovim (Zero Logic)
В Neovim 0.10+ / 0.11+ геометрия извлекается из нативных структур редактора:
1. **Сетка буфера с учетом интерфейса (`cols`, `rows`):**
   ```lua
   local win = vim.api.nvim_get_current_win()
   local wininfo = vim.fn.getwininfo(win)[1]
   local textoff = wininfo and wininfo.textoff or 0 -- отступ на номера строк, signcolumn, foldcolumn
   local cols = vim.api.nvim_win_get_width(win) - textoff
   local rows = vim.api.nvim_win_get_height(win)
   ```
2. **Физический размер ячейки (`cell_w`, `cell_h`):**
   Снимается клиентом один раз при старте через `ioctl(1, TIOCGWINSZ)` на дескрипторе TTY Neovim (стандарт `folke/snacks.nvim` и `3rd/image.nvim`), кэшируется в Lua и обновляется только по событию `VimResized`:
   ```lua
   local ffi = require("ffi")
   ffi.cdef([[
     typedef struct { unsigned short row, col, xpixel, ypixel; } winsize;
     int ioctl(int, int, ...);
   ]])
   local is_mac = vim.fn.has("mac") == 1
   local TIOCGWINSZ = (is_mac or vim.fn.has("bsd") == 1) and 0x40087468 or 0x5413

   local function get_cell_size()
     -- Защита от ловушки сплитов tmux (горизонтальных и вертикальных):
     -- Внутри tmux ws_xpixel и ws_ypixel возвращают размер всего окна терминала,
     -- а ws_col / ws_row — сплит панели. Деление дает искажения на любых сплитах!
     if vim.env.TMUX then
       return is_mac and 18 or 9, is_mac and 36 or 18
     end

     local sz = ffi.new("winsize")
     if ffi.C.ioctl(1, TIOCGWINSZ, sz) == 0 and sz.col > 0 and sz.row > 0 and sz.xpixel > 0 and sz.ypixel > 0 then
       return math.max(4, math.floor(sz.xpixel / sz.col)), math.max(8, math.floor(sz.ypixel / sz.row))
     end
     return is_mac and 18 or 9, is_mac and 36 or 18
   end
   ```
3. **Полярность темы:**
   `vim.o.background` (`"dark"` или `"light"`). Полностью нативное состояние Neovim.
4. **Вывод APC без открытия `/dev/tty`:**
   В Neovim 0.10+ управляющие APC отправляются в эмулятор через `vim.api.nvim_ui_send(apc)` (нативный канал TUI), исключая прямые манипуляции с файловыми дескрипторами.

### 3.3. Автономный запуск в интерактивном TTY (Standalone CLI)
Если утилита запущена пользователем напрямую в интерактивном терминале (`isatty(stdout) == true`) без передачи флагов геометрии:
- `ii` выполняет ровно один быстрый неблокирующий `ioctl(stdout_fd, TIOCGWINSZ)` (занимает $< 1\text{ мкс}$).
- **Защита от ловушки сплитов `tmux`:** если обнаружена переменная `$TMUX`, поля `ws_xpixel` и `ws_ypixel` **полностью игнорируются** (так как они возвращают геометрию всего окна терминала, а не сплита панели, что ломает деление как в вертикальных, так и в горизонтальных сплитах). Применяется базовый fallback ОС:
  - macOS (`target_os = "macos"`): `(18, 36)` (нативная Retina 2x).
  - Linux / BSD: `(9, 18)` (стандартный 1:2 DPI).
- **Вне `$TMUX`:** если `ws.ws_xpixel > 0` и `ws.ws_ypixel > 0`: `cell_w = (ws.ws_xpixel / ws.ws_col).max(4)`, `cell_h = (ws.ws_ypixel / ws.ws_row).max(8)`.
- Если пиксели не переданы (SSH без pixel-forwarding): применяется эталонное соотношение ОС (`18x36` на macOS, `9x18` на Linux/BSD).
- **Никаких зависаний на чтении escape-ответов при пайпах!** В режиме пайпа (`isatty(stdout) == false`) опрос TTY аппаратно запрещен.

---

## 4. Контракт потоков: Автономный Dual-Channel

Поток вывода в `stdout` определяется **100% автоматически** на основе системного дескриптора `isatty(stdout)` без каких-либо ручных флагов:

```text
┌─────────────────────────────────────────────────────────────────────────────┐
│ 1. ДЕФОЛТ ДЛЯ ПАЙПА / ДОЧЕРНЕГО ПРОЦЕССА (!isatty(stdout) == true):         │
│    Бинарный Dual-Channel пакет с 4-байтным LE префиксом длины:              │
│    [u32: apc_len (LE)] [apc_bytes (len=apc_len)] [text_bytes (rest of buf)] │
│                                                                             │
│    • apc_bytes: \e_Ga=T,U=1,t=s,i=<id>,c=<cols>,r=<rows>;<shm_name>\e\\    │
│    • text_bytes: монолитная сетка строк U+10EEEE с TrueColor SGR           │
│    • Применение: пейджеры предварительного просмотра (Yazi / fzf),          │
│      где весь документ выводится в единую прямоугольную область.            │
├─────────────────────────────────────────────────────────────────────────────┤
│ 2. ДЕФОЛТ ДЛЯ ИНТЕРАКТИВНОГО ШЕЛЛА (isatty(stdout) == true):               │
│    Прямой последовательный KGP поток в stdout:                              │
│    [apc_bytes][text_bytes]                                                  │
│    Терминал последовательно принимает загрузку текстуры в VRAM и сразу же   │
│    отображает плейсхолдеры на текущей позиции курсора.                      │
└─────────────────────────────────────────────────────────────────────────────┘
```

> [!IMPORTANT]
> **Редакторы буферов (Neovim):** Neovim никогда не парсит плоский текстовый пайп. Редактор работает со структурированным документом через бинарный сокет IXPC v1 (`CMD_COMPOSE_EXTMARKS = 0x0003`), получая KGP APC и массив 16-байтных структур `ixpc_extmark_t` с точной привязкой каждого блока к строке буфера (`end_row`) и независимым `image_id`.

---

## 5. Канонический Unix CLI Контракт

Никаких ручных флагов для очевидных вещей. Минимальный, ортогональный набор (ровно 6 функциональных флагов):

```text
ii [OPTIONS] [FILE]
cat document.md | ii [OPTIONS]
```

### Реестр флагов:

```text
ВХОДНАЯ ГЕОМЕТРИЯ:
  -s, --size <WxH>        Размер видимой области в ячейках (cols x rows)
      --cell-size <WxH>   Физический размер ячейки в пикселях (cell_w x cell_h)
      --skip <N>          Вертикальное смещение строк для пейджинга (seek)

ТЕМА:
  -t, --theme <THEME>     Полярность палитры: [dark | light]
                          (default: $COLORFGBG или 'dark', Zero-Probe TTY)

ДЕМОН И СОКЕТ:
      --daemon            Запуск фонового сопроцессора (reactor)
      --socket <PATH>     Путь к UNIX-сокету (default: $XDG_RUNTIME_DIR/ii/ii.sock)

ИНФО:
  -h, --help              Справка
  -V, --version           Версия
```

### Детерминированные правила исполнения:
1. **Интерактивный TTY (`isatty(stdout) == true`):**
   - Если `-s, --size` не задан: замер через быстрый `tcgetwinsize(1)`.
   - Если `--cell-size` не задан:
     - **Защита от ловушек сплитов `tmux` (горизонтальных и вертикальных):** внутри `$TMUX` поля `ws_xpixel` и `ws_ypixel` из `TIOCGWINSZ` **полностью игнорируются**, так как они возвращают пиксели всего окна терминала, а не текущей панели. Вместо ошибочного деления применяется нативный базовый fallback ОС:
       - macOS (`target_os = "macos"`): `(18, 36)` (нативная Retina 2x для терминалов 13–16pt).
       - Linux / BSD: `(9, 18)` (стандартный 1:2 DPI).
     - Вне `$TMUX`: если `ws_xpixel > 0` и `ws_ypixel > 0`, вычисляется `ws_xpixel / ws_col` и `ws_ypixel / ws_row`. Иначе fallback `(18, 36)` на macOS или `(9, 18)` на Linux.
   - Если `-t, --theme` не задан: проверка переменной `$COLORFGBG` (например, `COLORFGBG="15;0"` $\to$ dark, `"0;15"` $\to$ light), fallback `dark`. Никаких блокирующих escape-опросов `OSC 11`!
   - Вывод: поток KGP пишется прямо в `stdout`. Никаких открытий `/dev/tty` на запись.
2. **Пайп / Дочерний процесс (`isatty(stdout) == false`):**
   - **Zero TTY I/O:** строгий запрет любых опросов и записей в `/dev/tty`.
   - Геометрия считывается строго из входящего состояния: `-s, --size` и `--cell-size` (fallback: `18x36` на macOS, `9x18` на Linux).
   - Тема считывается из входящего состояния: `-t, --theme` (fallback: `dark`).
   - Вывод: бинарный Dual-Channel пакет (`[u32 apc_len][apc][text]`) пишется в `stdout`.
3. **Прозрачный IPC:**
   - Если существует сокет `--socket`, выполняется неблокирующий `connect(2)` с таймаутом 2 мс. При успехе запрос обрабатывается демоном за $< 50\text{ мкс}$.
   - При отказе сокета (`ECONNREFUSED`/`ENOENT`) управление без ошибок передается локальному in-process растрированию.
4. **Управление демоном:**
   - Остановка демона: стандартный `kill -TERM <pid>`.
   - Проверка живости: стандартный `connect(2)` к сокету.

---

## 6. Интеграция с клиентами: Архитектурное разделение

1. **Yazi (`plugins/ii.yazi`): Монолитный пейджер предпросмотра**
   - Считывает нативное состояние рантайма Yazi (`job.area.w`, `job.area.h`, `rt.term.cell_size()`, `rt.term.light()`).
   - Запускает `ii` через пайп. Автоматически получает Dual-Channel поток `[apc_len][apc_bytes][text_bytes]`.
   - Сбрасывает `apc_bytes` в терминал через `ya.mgr_emit("term_write", { apc })`.
   - Монтирует монолитную сетку плейсхолдеров `U+10EEEE` через `ya.preview_widget(job, ui.Text.parse(text):area(job.area))`.
   - *Канонический компилируемый код плагина приведен в **Разделе 7.5**.*

2. **Neovim (`plugins/ii.nvim`): Структурный композитор буфера**
   - Подключается к демону через постоянный бинарный сокет IXPC v1 (`$XDG_RUNTIME_DIR/ii/ii.sock`) с использованием `vim.uv.new_pipe(false)`.
   - **Автостарт демона (Zero-Configuration):** при ошибках подключения `ENOENT` (файл сокета отсутствует) или `ECONNREFUSED` (демон не запущен) плагин прозрачно для пользователя фоново запускает `ii --daemon` через `vim.system` и производит повторное подключение.
   - Отправляет 32-байтный заголовок с `CMD_COMPOSE_EXTMARKS = 0x0003` и срез строк активного буфера.
   - Принимает KGP APC и бинарный массив 16-байтных структур `ixpc_extmark_t`.
   - Для каждого блока:
     - Регистрирует персональную группу подсветки `nvim_set_hl(0, "IiImg_" .. id, { fg = hex })`.
     - Генерирует чистые плейсхолдеры `U+10EEEE` **без единого ANSI escape-кода**.
     - Монтирует виртуальные строки ровно под нужной строкой буфера: `nvim_buf_set_extmark(bufnr, ns_id, end_row, 0, { virt_lines = ... })`.
   - *Канонический компилируемый код плагина приведен в **Разделе 7.6**.*

---

## 7. Эталонная реализация компонентов ядра (Bit-Exact Production Blueprint)

Ниже приводится законченная, компилируемая спецификация ключевых компонентов, устраняющая любые абстракции.

### 7.1. Zero-Allocation CLI Parser & Dual-Channel Dispatch (`src/main.rs`)

Парсер аргументов выполняется за один проход по итератору `std::env::args_os()` без использования тяжелых внешних библиотек (`clap`), без аллокаций в куче и без регулярных выражений. Набор опций строго ограничен 6 функциональными флагами:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeChoice {
    Dark,
    Light,
}

pub struct CliArgs<'a> {
    pub size: Option<(u16, u16)>,         // cols, rows
    pub cell_size: Option<(u16, u16)>,    // cell_w, cell_h (px)
    pub skip: usize,
    pub theme: Option<ThemeChoice>,
    pub is_daemon: bool,
    pub socket_path: Option<&'a Path>,
    pub file_path: Option<&'a Path>,
}

impl<'a> CliArgs<'a> {
    pub fn parse(args: &'a [std::ffi::OsString]) -> Result<Self, Error> {
        let mut parsed = Self {
            size: None,
            cell_size: None,
            skip: 0,
            theme: None,
            is_daemon: false,
            socket_path: None,
            file_path: None,
        };

        let mut it = args.iter().peekable();
        while let Some(arg) = it.next() {
            let Some(s) = arg.to_str() else { continue };
            match s {
                "-s" | "--size" => {
                    let val = it.next().and_then(|v| v.to_str()).ok_or(Error::InvalidCli)?;
                    parsed.size = Some(parse_wxh(val)?);
                }
                "--cell-size" => {
                    let val = it.next().and_then(|v| v.to_str()).ok_or(Error::InvalidCli)?;
                    parsed.cell_size = Some(parse_wxh(val)?);
                }
                "--skip" => {
                    let val = it.next().and_then(|v| v.to_str()).ok_or(Error::InvalidCli)?;
                    parsed.skip = val.parse::<usize>().map_err(|_| Error::InvalidCli)?;
                }
                "-t" | "--theme" => {
                    let val = it.next().and_then(|v| v.to_str()).ok_or(Error::InvalidCli)?;
                    parsed.theme = match val {
                        "dark" => Some(ThemeChoice::Dark),
                        "light" => Some(ThemeChoice::Light),
                        _ => return Err(Error::InvalidCli),
                    };
                }
                "--daemon" => parsed.is_daemon = true,
                "--socket" => {
                    let val = it.next().ok_or(Error::InvalidCli)?;
                    parsed.socket_path = Some(Path::new(val));
                }
                "-h" | "--help" => {
                    let _ = write_all(std::io::stdout().as_fd(), USAGE);
                    std::process::exit(0);
                }
                "-V" | "--version" => {
                    let _ = write_all(std::io::stdout().as_fd(), VERSION_LINE);
                    std::process::exit(0);
                }
                pos if !pos.starts_with('-') => {
                    parsed.file_path = Some(Path::new(arg));
                }
                _ => return Err(Error::InvalidCli),
            }
        }
        Ok(parsed)
    }
}

// Парсер геометрии "WxH" без String-аллокаций:
fn parse_wxh(s: &str) -> Result<(u16, u16), Error> {
    let mut parts = s.splitn(2, 'x');
    let w = parts.next().and_then(|v| v.parse::<u16>().ok()).ok_or(Error::InvalidCli)?;
    let h = parts.next().and_then(|v| v.parse::<u16>().ok()).ok_or(Error::InvalidCli)?;
    if w == 0 || h == 0 { return Err(Error::InvalidCli); }
    Ok((w, h))
}
```

#### Dual-Channel сериализатор и автоматическая маршрутизация в `stdout`:
```rust
fn emit_dual_channel(
    stdout_fd: BorrowedFd<'_>,
    apc_bytes: &[u8],
    text_bytes: &[u8],
) -> Result<(), Error> {
    let apc_len = u32::try_from(apc_bytes.len()).map_err(|_| Error::PayloadTooLarge(0))?;
    let prefix = apc_len.to_le_bytes(); // 4 байта LE
    
    // Последовательная гарантированная запись частей без риска повреждения буфера при частичном I/O:
    write_all(stdout_fd, &prefix)?;
    write_all(stdout_fd, apc_bytes)?;
    write_all(stdout_fd, text_bytes)?;
    Ok(())
}

// Диспетчеризация потока вывода: 100% автоматически по дескриптору stdout
fn output_results(
    stdout_fd: BorrowedFd<'_>,
    apc_bytes: &[u8],
    text_bytes: &[u8],
) -> Result<(), Error> {
    if rustix::io::isatty(stdout_fd) {
        // Интерактивный TTY: KGP поток прямо на текущую позицию курсора
        write_all(stdout_fd, apc_bytes)?;
        write_all(stdout_fd, text_bytes)?;
    } else {
        // Пайп / дочерний процесс (Yazi / пейджеры): гарантированный Dual-Channel пакет
        emit_dual_channel(stdout_fd, apc_bytes, text_bytes)?;
    }
    Ok(())
}
```

---

### 7.2. Sans-I/O TTY & Geometry Solver (`src/tty.rs`)

Строгое соблюдение Zero-TTY инварианта, защита от переполнения `u16` и защита от ловушки сплитов `tmux`:

```rust
pub fn resolve_metrics(
    size_arg: Option<(u16, u16)>,
    cell_size_arg: Option<(u16, u16)>,
    is_interactive: bool,
) -> Metrics {
    #[cfg(target_os = "macos")]
    let default_cell = (18, 36);
    #[cfg(not(target_os = "macos"))]
    let default_cell = (9, 18);

    let (cols, rows, cell_w, cell_h) = if is_interactive {
        // Standalone запуск в интерактивном эмуляторе:
        let mut ws = rustix::termios::Winsize { ws_row: 0, ws_col: 0, ws_xpixel: 0, ws_ypixel: 0 };
        let stdout = std::io::stdout();
        let _ = rustix::termios::tcgetwinsize(stdout.as_fd(), &mut ws);

        let cols = size_arg.map(|(w, _)| w).unwrap_or(if ws.ws_col > 0 { ws.ws_col } else { 80 });
        let rows = size_arg.map(|(_, h)| h).unwrap_or(if ws.ws_row > 0 { ws.ws_row } else { 24 });

        let is_tmux = std::env::var_os("TMUX").is_some();
        let (cell_w, cell_h) = if let Some(cs) = cell_size_arg {
            cs
        } else if !is_tmux && ws.ws_col > 0 && ws.ws_xpixel > 0 && ws.ws_row > 0 && ws.ws_ypixel > 0 {
            (
                (ws.ws_xpixel / ws.ws_col).max(4),
                (ws.ws_ypixel / ws.ws_row).max(8),
            )
        } else {
            default_cell // Защита от сплитов tmux и SSH без xpixel: OS-aware fallback
        };
        (cols, rows, cell_w, cell_h)
    } else {
        // Чистый пайп: опрос TTY запрещен! Берем исключительно входящее состояние:
        let (cols, rows) = size_arg.unwrap_or((80, 24));
        let (cell_w, cell_h) = cell_size_arg.unwrap_or(default_cell);
        (cols, rows, cell_w, cell_h)
    };

    // Защита от переполнения u16 при умножении cols * cell_w и rows * cell_h:
    let px_w = (u32::from(cols) * u32::from(cell_w)).min(u32::from(u16::MAX)) as u16;
    let px_h = (u32::from(rows) * u32::from(cell_h)).min(u32::from(u16::MAX)) as u16;

    Metrics { cols, rows, cell_w, cell_h, px_w, px_h, ..Metrics::FALLBACK }
}
```

---

### 7.3. Canvas Snapped Allocation & Strict Scale-Down (`src/raster.rs`)

Алгоритм растеризации с защитой от искажения пропорций KGP GPU, защитой от деления на ноль и поддержкой нативной прозрачности терминала:

```rust
pub fn scale_down_and_snap(
    intrinsic_w: f32,
    intrinsic_h: f32,
    cols: u16,
    rows: u16,
    cell_w: u16,
    cell_h: u16,
) -> Result<CanvasLayout, Error> {
    // Защита от битых/пустых входных SVG (viewBox="0 0 0 0" или NaN):
    if !(intrinsic_w.is_finite() && intrinsic_h.is_finite() && intrinsic_w > 0.0 && intrinsic_h > 0.0) {
        return Err(Error::SvgParse);
    }

    let max_px_w = (cols as f32) * (cell_w as f32);
    let max_px_h = (rows as f32) * (cell_h as f32);

    // Строгий scale-down (scale <= 1.0):
    let scale_x = max_px_w / intrinsic_w;
    let scale_y = max_px_h / intrinsic_h;
    let scale = 1.0_f32.min(scale_x).min(scale_y);

    let content_w = ((intrinsic_w * scale).ceil() as u32).min(max_px_w as u32).max(1);
    let content_h = ((intrinsic_h * scale).ceil() as u32).min(max_px_h as u32).max(1);

    // Квантование символьной сетки:
    let grid_cols = content_w.div_ceil(cell_w as u32).max(1) as u16;
    let grid_rows = content_h.div_ceil(cell_h as u32).max(1) as u16;

    // Физический размер холста (Canvas Frame):
    let snap_w = (grid_cols as u32) * (cell_w as u32);
    let snap_h = (grid_rows as u32) * (cell_h as u32);

    if snap_w > MAX_DIM || snap_h > MAX_DIM {
        return Err(Error::ImageTooLarge { w: snap_w, h: snap_h });
    }

    // Центрирование контента:
    let off_x = ((snap_w - content_w) / 2) as f32;
    let off_y = ((snap_h - content_h) / 2) as f32;

    Ok(CanvasLayout {
        scale,
        grid_cols,
        grid_rows,
        snap_w,
        snap_h,
        off_x,
        off_y,
    })
}

pub struct CanvasLayout {
    pub scale: f32,
    pub grid_cols: u16,
    pub grid_rows: u16,
    pub snap_w: u32,
    pub snap_h: u32,
    pub off_x: f32,
    pub off_y: f32,
}

pub fn rasterize_svg_snapped(
    svg_data: &[u8],
    cols: u16,
    rows: u16,
    cell_w: u16,
    cell_h: u16,
    rgba_out: &mut Vec<u8>,
) -> Result<(u16, u16), Error> {
    let tree = resvg::usvg::Tree::from_data(svg_data, &resvg::usvg::Options::default())
        .map_err(|_| Error::SvgParse)?;
    let (src_w, src_h) = (tree.size().width(), tree.size().height());
    
    let layout = scale_down_and_snap(src_w, src_h, cols, rows, cell_w, cell_h)?;
    let byte_len = (layout.snap_w * layout.snap_h * 4) as usize;

    // Аллокация буфера и очистка в безусловно прозрачный RGBA(0, 0, 0, 0):
    rgba_out.clear();
    rgba_out.resize(byte_len, 0);

    let mut pixmap = tiny_skia::PixmapMut::from_bytes(rgba_out, layout.snap_w, layout.snap_h)
        .ok_or(Error::SvgParse)?;

    // Отрисовка со сдвигом в центр квантованного холста:
    let ts = tiny_skia::Transform::from_scale(layout.scale, layout.scale)
        .post_translate(layout.off_x, layout.off_y);
    resvg::render(&tree, ts, &mut pixmap);

    // Q24 Unpremultiply:
    unpremultiply(rgba_out);

    Ok((layout.grid_cols, layout.grid_rows))
}
```

---

### 7.4. IXPC v1 Wire Protocol, Commands & ExtmarkRecord (`src/ipc.rs`)

Бинарный протокол межпроцессного взаимодействия с явной передачей признаков окружения клиента в демон и структурной привязкой графических блоков:

```rust
pub const MAGIC: u32 = 0x4958_5043; // ASCII 'IXPC'
pub const VERSION: u16 = 1;

// Реестр команд протокола IXPC v1:
pub const CMD_PING: u16             = 0x0001; // Проверка доступности демона
pub const CMD_COMPOSE_ANSI: u16     = 0x0002; // Монолитный Dual-Channel вывод
pub const CMD_COMPOSE_EXTMARKS: u16 = 0x0003; // Структурный вывод для редакторов (Neovim)
pub const CMD_RENDER_FILE: u16      = 0x0004; // Рендеринг файла по пути
pub const CMD_SHUTDOWN: u16         = 0x0006; // Корректное завершение демона

// Биты флагов окружения клиента (Header.flags на входе):
pub const FLAG_THEME_LIGHT: u32 = 1 << 0;  // 0 = dark, 1 = light
pub const FLAG_TMUX: u32        = 1 << 1;  // Клиент внутри tmux (оборачивать KGP в DCS passthrough)
pub const FLAG_REMOTE: u32      = 1 << 2;  // Клиент удаленный/SSH (принудительно Base64, без SHM)

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub magic: u32,       // MAGIC (0x4958_5043)
    pub version: u16,     // 1
    pub command: u16,     // CMD_*
    pub req_id_lo: u32,   // bufnr / job_id
    pub req_id_hi: u32,   // changedtick
    pub flags: u32,       // in: биты FLAG_*; out: apc_len (LE)
    pub payload_len: u32, // длина последующего буфера
    pub cols: u16,        // вьюпорт cols
    pub rows: u16,        // вьюпорт rows
    pub cell_w: u16,      // физическая ширина ячейки (px)
    pub cell_h: u16,      // физическая высота ячейки (px)
}
const _: () = assert!(std::mem::size_of::<Header>() == 32);

// Структурная запись привязки графического блока к буферу (для CMD_COMPOSE_EXTMARKS):
#[repr(C, packed)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExtmarkRecord {
    pub image_id: u32,   // 24-битный NonZeroU32 идентификатор текстуры (LE)
    pub start_row: u16,  // 0-индексированная строка буфера для virt_lines
    pub start_col: u16,  // 0-индексированная колонка начала блока
    pub end_row: u16,    // Строка окончания блока разметки
    pub end_col: u16,    // Колонка окончания блока разметки
    pub cols: u16,       // Ширина блока в ячейках терминала
    pub rows: u16,       // Высота блока в ячейках терминала
}
const _: () = assert!(std::mem::size_of::<ExtmarkRecord>() == 16);
```

---

### 7.5. Production Client: Yazi (`plugins/ii.yazi/main.lua`)

Полный, готовый к установке плагин без костылей и без ручных флагов формата (Dual-Channel активируется автоматически по `!isatty(stdout)`):

```lua
local M = {}

local function unpack_u32_le(raw, offset)
  local b1, b2, b3, b4 = string.byte(raw, offset, offset + 3)
  return b1 + b2 * 256 + b3 * 65536 + b4 * 16777216
end

function M:peek(job)
  local w, h = job.area.w, job.area.h
  local cw, ch = rt.term.cell_size()
  local cell_size_arg = (cw and ch) and string.format("%dx%d", math.floor(cw), math.floor(ch)) or "9x18"
  local theme_arg = rt.term.light() and "light" or "dark"

  local child, err = Command("ii")
    :args({
      "--size", string.format("%dx%d", w, h),
      "--cell-size", cell_size_arg,
      "--skip", tostring(job.skip or 0),
      "--theme", theme_arg,
      tostring(job.file.url)
    })
    :stdout(Command.PIPED)
    :stderr(Command.NULL)
    :output()

  if not child or not child.status.success then
    ya.preview_widget(job, ui.Text("ii: render failed"):area(job.area))
    return
  end

  local raw = child.stdout
  if #raw < 4 then return end

  local apc_len = unpack_u32_le(raw, 1)
  local apc = raw:sub(5, 4 + apc_len)
  local text = raw:sub(5 + apc_len)

  if #apc > 0 then
    ya.mgr_emit("term_write", { apc })
  end
  ya.preview_widget(job, ui.Text.parse(text):area(job.area))
end

function M:seek(job)
  -- Аппаратный скроллинг: обновление skip и перерисовка
  self:peek(job)
end

return M
```

---

### 7.6. Production Client: Neovim (`plugins/ii.nvim/lua/ii/init.lua`)

Полный, готовый к установке плагин для Neovim 0.10+ (совместим с LuaJIT, валидацией changedtick, сокетом IXPC v1, автозапуском демона, корректными диакритиками строк KGP и нативными подсветками `U+10EEEE`):

```lua
local M = {}
local is_mac = vim.fn.has("mac") == 1
local cached_cell_w, cached_cell_h = is_mac and 18 or 9, is_mac and 36 or 18
local ns_id = vim.api.nvim_create_namespace("ii_renderer")
local hl_cache = {}

-- Kitty Graphics Protocol: 115 диакритик для кодирования индекса строки (U=1):
local PLACEHOLDER = "\244\142\187\174" -- U+10EEEE
local DIACRITIC_CODEPOINTS = {
  0x0305, 0x030D, 0x030E, 0x0310, 0x0312, 0x033D, 0x033E, 0x033F, 0x0346, 0x034A,
  0x034B, 0x034C, 0x0350, 0x0351, 0x0352, 0x0357, 0x035B, 0x0363, 0x0364, 0x0365,
  0x0366, 0x0367, 0x0368, 0x0369, 0x036A, 0x036B, 0x036C, 0x036D, 0x036E, 0x036F,
  0x0483, 0x0484, 0x0485, 0x0486, 0x0487, 0x0592, 0x0593, 0x0594, 0x0595, 0x0597,
  0x0598, 0x0599, 0x059C, 0x059D, 0x059E, 0x059F, 0x05A0, 0x05A1, 0x05A8, 0x05A9,
  0x05AB, 0x05AC, 0x05AF, 0x05C4, 0x0610, 0x0611, 0x0612, 0x0613, 0x0614, 0x0615,
  0x0616, 0x0617, 0x0657, 0x0658, 0x0659, 0x065A, 0x065B, 0x065D, 0x065E, 0x06D6,
  0x06D7, 0x06D8, 0x06D9, 0x06DA, 0x06DB, 0x06DC, 0x06DF, 0x06E0, 0x06E1, 0x06E2,
  0x06E4, 0x06E7, 0x06E8, 0x06EB, 0x06EC, 0x0730, 0x0732, 0x0733, 0x0735, 0x0736,
  0x073A, 0x073D, 0x073F, 0x0740, 0x0741, 0x0743, 0x0745, 0x0747, 0x0749, 0x074A,
  0x07EB, 0x07EC, 0x07ED, 0x07EE, 0x07EF, 0x07F0, 0x07F1, 0x07F3, 0x0816, 0x0817,
  0x0818, 0x0819, 0x081B, 0x081C, 0x081D,
}

local function utf8_encode(cp)
  if cp < 128 then
    return string.char(cp)
  elseif cp < 2048 then
    return string.char(192 + math.floor(cp / 64), 128 + (cp % 64))
  else
    return string.char(224 + math.floor(cp / 4096), 128 + math.floor((cp % 4096) / 64), 128 + (cp % 64))
  end
end

local DIACRITICS = {}
for i, cp in ipairs(DIACRITIC_CODEPOINTS) do
  DIACRITICS[i] = utf8_encode(cp)
end

-- Упаковка LE чисел без внешних зависимостей (LuaJIT / Lua 5.1 compatible):
local function pack_u16_le(val)
  local v = math.floor(val) % 65536
  return string.char(v % 256, math.floor(v / 256))
end

local function pack_u32_le(val)
  local v = math.floor(val) % 4294967296
  local b1 = v % 256
  v = math.floor(v / 256)
  local b2 = v % 256
  v = math.floor(v / 256)
  local b3 = v % 256
  local b4 = math.floor(v / 256)
  return string.char(b1, b2, b3, b4)
end

-- Распаковка LE чисел без string.unpack (LuaJIT / Lua 5.1 compatible):
local function unpack_u16_le(str, offset)
  local b1, b2 = string.byte(str, offset, offset + 1)
  return b1 + b2 * 256
end

local function unpack_u32_le(str, offset)
  local b1, b2, b3, b4 = string.byte(str, offset, offset + 3)
  return b1 + b2 * 256 + b3 * 65536 + b4 * 16777216
end

-- Динамическое создание hl_group для кодирования image_id в цвете текста U+10EEEE:
local function get_image_hl(image_id)
  local hl_name = string.format("IiImg_%06x", image_id)
  if not hl_cache[image_id] then
    vim.api.nvim_set_hl(0, hl_name, { fg = string.format("#%06x", image_id) })
    hl_cache[image_id] = hl_name
  end
  return hl_name
end

-- Определение пути к сокету IXPC v1:
local function get_socket_path()
  if vim.g.ii_socket_path and #vim.g.ii_socket_path > 0 then
    return vim.g.ii_socket_path
  end
  local xdg = vim.env.XDG_RUNTIME_DIR
  if xdg and #xdg > 0 and vim.uv.fs_stat(xdg) then
    return xdg .. "/ii/ii.sock"
  end
  local tmp = (vim.env.TMPDIR or "/tmp"):gsub("/+$", "")
  local uid = vim.uv.getuid and vim.uv.getuid() or 1000
  return string.format("%s/ii-%d/ii.sock", tmp, uid)
end

-- Определение размеров ячейки через ioctl(1, TIOCGWINSZ) с защитой от ловушки сплитов tmux:
local function update_cell_metrics()
  if vim.env.TMUX then
    cached_cell_w, cached_cell_h = is_mac and 18 or 9, is_mac and 36 or 18
    return
  end

  local ffi = require("ffi")
  ffi.cdef([[
    typedef struct { unsigned short row, col, xpixel, ypixel; } winsize;
    int ioctl(int, int, ...);
  ]])
  local TIOCGWINSZ = (is_mac or vim.fn.has("bsd") == 1) and 0x40087468 or 0x5413
  local sz = ffi.new("winsize")
  if ffi.C.ioctl(1, TIOCGWINSZ, sz) == 0 and sz.col > 0 and sz.row > 0 and sz.xpixel > 0 and sz.ypixel > 0 then
    cached_cell_w = math.max(4, math.floor(sz.xpixel / sz.col))
    cached_cell_h = math.max(8, math.floor(sz.ypixel / sz.row))
  else
    cached_cell_w, cached_cell_h = is_mac and 18 or 9, is_mac and 36 or 18
  end
end

update_cell_metrics()
vim.api.nvim_create_autocmd("VimResized", { callback = update_cell_metrics })

-- Подключение к демону с прозрачным автозапуском (Zero-Configuration):
local function connect_daemon(sock_path, cb, retries)
  retries = retries or 0
  local pipe = vim.uv.new_pipe(false)

  pipe:connect(sock_path, function(err)
    if not err then
      cb(nil, pipe)
      return
    end

    pipe:close()
    -- При отсутствии сокета (ENOENT) или отказе (ECONNREFUSED) — фоновый старт:
    if (err:match("ENOENT") or err:match("ECONNREFUSED")) and retries == 0 then
      vim.schedule(function()
        vim.system({ "ii", "--daemon", "--socket", sock_path }, { detach = true }, function(_) end)
      end)

      -- Повторное подключение через 40 мс:
      local timer = vim.uv.new_timer()
      timer:start(40, 0, function()
        timer:stop()
        timer:close()
        connect_daemon(sock_path, cb, retries + 1)
      end)
    else
      cb(err, nil)
    end
  end)
end

function M.render(bufnr, win)
  win = win or vim.api.nvim_get_current_win()
  bufnr = bufnr or vim.api.nvim_win_get_buf(win)

  if not vim.api.nvim_buf_is_valid(bufnr) then return end
  local tick = vim.api.nvim_buf_get_changedtick(bufnr)

  local wininfo = vim.fn.getwininfo(win)[1]
  local textoff = wininfo and wininfo.textoff or 0
  local cols = math.max(1, vim.api.nvim_win_get_width(win) - textoff)
  local rows = math.max(1, vim.api.nvim_win_get_height(win))
  local sock_path = get_socket_path()

  local flags = 0
  if vim.o.background == "light" then flags = flags + 1 end            -- FLAG_THEME_LIGHT
  if vim.env.TMUX then flags = flags + 2 end                           -- FLAG_TMUX
  if vim.env.SSH_CLIENT or vim.env.SSH_TTY then flags = flags + 4 end   -- FLAG_REMOTE

  local lines = vim.api.nvim_buf_get_lines(bufnr, 0, -1, false)
  local payload = table.concat(lines, "\n")
  local payload_len = #payload

  local header = pack_u32_le(0x49585043) ..    -- MAGIC
                 pack_u16_le(1) ..             -- VERSION
                 pack_u16_le(3) ..             -- CMD_COMPOSE_EXTMARKS
                 pack_u32_le(bufnr) ..         -- req_id_lo
                 pack_u32_le(tick) ..          -- req_id_hi
                 pack_u32_le(flags) ..         -- flags
                 pack_u32_le(payload_len) ..   -- payload_len
                 pack_u16_le(cols) ..          -- cols
                 pack_u16_le(rows) ..          -- rows
                 pack_u16_le(cached_cell_w) .. -- cell_w
                 pack_u16_le(cached_cell_h)    -- cell_h

  connect_daemon(sock_path, function(err, pipe)
    if err or not pipe then return end

    local rx_chunks = {}
    local total_read = 0
    local expected_len = nil
    local apc_len = nil

    pipe:read_start(function(read_err, chunk)
      if read_err or not chunk then
        pipe:close()
        return
      end

      table.insert(rx_chunks, chunk)
      total_read = total_read + #chunk

      if not expected_len and total_read >= 32 then
        local full_prefix = table.concat(rx_chunks)
        local resp_payload_len = unpack_u32_le(full_prefix, 21)
        apc_len = unpack_u32_le(full_prefix, 17)
        expected_len = 32 + resp_payload_len
      end

      if expected_len and total_read >= expected_len then
        pipe:read_stop()
        pipe:close()

        local full_buf = table.concat(rx_chunks)
        vim.schedule(function()
          -- Валидация буфера и актуальности редакторского тика:
          if not vim.api.nvim_buf_is_valid(bufnr) then return end
          if vim.api.nvim_buf_get_changedtick(bufnr) ~= tick then return end

          -- 1. Сброс KGP APC в UI-канал терминала (без прямого открытия /dev/tty):
          local apc_bytes = full_buf:sub(33, 32 + apc_len)
          if #apc_bytes > 0 then
            if vim.api.nvim_ui_send then
              vim.api.nvim_ui_send(apc_bytes)
            else
              io.stdout:write(apc_bytes)
            end
          end

          -- 2. Очистка старых экстмарков:
          vim.api.nvim_buf_clear_namespace(bufnr, ns_id, 0, -1)

          -- 3. Разбор массива 16-байтных структур ixpc_extmark_t:
          local extmarks_offset = 33 + apc_len
          local extmarks_end = expected_len
          local offset = extmarks_offset

          while offset + 16 - 1 <= extmarks_end do
            local image_id = unpack_u32_le(full_buf, offset)
            local end_row = unpack_u16_le(full_buf, offset + 8)
            local m_cols = unpack_u16_le(full_buf, offset + 12)
            local m_rows = unpack_u16_le(full_buf, offset + 14)

            local hl_name = get_image_hl(image_id)

            -- Формирование строк U+10EEEE с диакритиками строк Kitty Graphics Protocol (U=1):
            local virt_lines = {}
            for r = 0, m_rows - 1 do
              local diacritic = DIACRITICS[math.min(r + 1, #DIACRITICS)] or ""
              local first_cell = PLACEHOLDER .. diacritic
              local tail = m_cols > 1 and string.rep(PLACEHOLDER, m_cols - 1) or ""
              table.insert(virt_lines, { { first_cell .. tail, hl_name } })
            end

            if #virt_lines > 0 then
              vim.api.nvim_buf_set_extmark(bufnr, ns_id, end_row, 0, {
                virt_lines = virt_lines,
                virt_lines_above = false,
              })
            end

            offset = offset + 16
          end
        end)
      end
    end)

    pipe:write(header .. payload)
  end)
end

return M
```
