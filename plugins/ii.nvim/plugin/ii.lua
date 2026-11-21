-- Single-file Neovim plugin for `ii`: async IXPC v1 client, /dev/tty KGP writer, U+10EEEE extmarks.

local uv = vim.uv
local bit = require("bit")

local M = {}

local MAGIC = 0x49585043
local VERSION = 1
local ERR_FLAG = 0x4000
local CMD_COMPOSE_EXTMARKS = 0x0003
local CMD_SHUTDOWN = 0x0006
local FLAG_LIGHT = 1
local FLAG_TMUX = 2
local FLAG_REMOTE = 4

local PLACEHOLDER = string.char(0xF4, 0x8E, 0xBB, 0xAE) -- U+10EEEE

-- Exact 1:1 mirror of `src/kitty.rs::DIACRITICS_UTF8` (64 canonical combining diacritics).
local DIACRITIC_BYTES = {
  0xCC,
  0x85,
  0xCC,
  0x8D,
  0xCC,
  0x8E,
  0xCC,
  0x90,
  0xCC,
  0x92,
  0xCC,
  0xBD,
  0xCC,
  0xBE,
  0xCC,
  0xBF,
  0xCD,
  0x86,
  0xCD,
  0x8A,
  0xCD,
  0x8B,
  0xCD,
  0x8C,
  0xCD,
  0x90,
  0xCD,
  0x91,
  0xCD,
  0x92,
  0xCD,
  0x97,
  0xCD,
  0x9B,
  0xCD,
  0xA3,
  0xCD,
  0xA4,
  0xCD,
  0xA5,
  0xCD,
  0xA6,
  0xCD,
  0xA7,
  0xCD,
  0xA8,
  0xCD,
  0xA9,
  0xCD,
  0xAA,
  0xCD,
  0xAB,
  0xCD,
  0xAC,
  0xCD,
  0xAD,
  0xCD,
  0xAE,
  0xCD,
  0xAF,
  0xD2,
  0x83,
  0xD2,
  0x84,
  0xD2,
  0x85,
  0xD2,
  0x86,
  0xD6,
  0x92,
  0xD6,
  0x93,
  0xD6,
  0x94,
  0xD6,
  0x95,
  0xD6,
  0x97,
  0xD6,
  0x98,
  0xD6,
  0x99,
  0xD6,
  0x9C,
  0xD6,
  0x9D,
  0xD6,
  0x9E,
  0xD6,
  0x9F,
  0xD6,
  0xA0,
  0xD6,
  0xA1,
  0xD6,
  0xA8,
  0xD6,
  0xA9,
  0xD6,
  0xAB,
  0xD6,
  0xAC,
  0xD6,
  0xAF,
  0xD7,
  0x84,
  0xD8,
  0x90,
  0xD8,
  0x91,
  0xD8,
  0x92,
  0xD8,
  0x93,
  0xD8,
  0x94,
  0xD8,
  0x95,
  0xD8,
  0x96,
  0xD8,
  0x97,
  0xD9,
  0x97,
  0xD9,
  0x98,
  0xD9,
  0x99,
}

local DIACRITICS = {}
for i = 1, 64 do
  local off = (i - 1) * 2 + 1
  DIACRITICS[i] = string.char(DIACRITIC_BYTES[off], DIACRITIC_BYTES[off + 1])
end
local FIRST_COL = DIACRITICS[1]

local NS = vim.api.nvim_create_namespace("ii")
local AUGROUP = vim.api.nvim_create_augroup("ii", { clear = true })

local DEFAULT_CFG = {
  bin = "ii",
  debounce_ms = 60,
  enabled = true,
  hide_on_cursor = true,
  filetypes = {
    markdown = true,
    quarto = true,
    mdx = true,
    rmd = true,
    typst = true,
    tex = true,
    plaintex = true,
  },
}

local cfg = vim.deepcopy(DEFAULT_CFG)
local buf_state = {} -- [buf] = { timer, inflight, dirty, tick, marks }
local hl_cache = {} -- [image_id] = hl_group
local tty_fd = nil
local tty_queue = {}
local tty_flushing = false

local ok_ffi, ffi = pcall(require, "ffi")
if ok_ffi then
  pcall(
    ffi.cdef,
    [[
    typedef struct ii_winsize {
      unsigned short ws_row;
      unsigned short ws_col;
      unsigned short ws_xpixel;
      unsigned short ws_ypixel;
    } ii_winsize;
    int ioctl(int fd, unsigned long request, ...);
  ]]
  )
end

-- Binary IXPC v1 codec -------------------------------------------------------

local function u16_le(n) return string.char(bit.band(n, 0xff), bit.band(bit.rshift(n, 8), 0xff)) end

local function u32_le(n)
  return string.char(
    bit.band(n, 0xff),
    bit.band(bit.rshift(n, 8), 0xff),
    bit.band(bit.rshift(n, 16), 0xff),
    bit.band(bit.rshift(n, 24), 0xff)
  )
end

local function read_u16_le(s, i)
  local b0, b1 = string.byte(s, i, i + 1)
  return b0 + b1 * 256
end

local function read_u32_le(s, i)
  local b0, b1, b2, b3 = string.byte(s, i, i + 3)
  return b0 + b1 * 256 + b2 * 65536 + b3 * 16777216
end

local function socket_path()
  local xdg = os.getenv("XDG_RUNTIME_DIR")
  if xdg and xdg ~= "" then return xdg .. "/ii/ii.sock" end
  local uid = uv.getuid and uv.getuid() or 0
  return string.format("/tmp/ii-%d/ii.sock", uid)
end

local function encode_header(cmd, flags, cols, rows, cell_w, cell_h, payload_len)
  return table.concat {
    u32_le(MAGIC),
    u16_le(VERSION),
    u16_le(cmd),
    u32_le(flags),
    u16_le(cols),
    u16_le(rows),
    u16_le(cell_w),
    u16_le(cell_h),
    u32_le(payload_len),
    "\0\0\0\0\0\0\0\0",
  }
end

local function decode_extmarks(bytes)
  local out = {}
  local len = #bytes
  local i = 1
  while i + 15 <= len do
    out[#out + 1] = {
      image_id = read_u32_le(bytes, i),
      start_row = read_u16_le(bytes, i + 4),
      start_col = read_u16_le(bytes, i + 6),
      end_row = read_u16_le(bytes, i + 8),
      end_col = read_u16_le(bytes, i + 10),
      cols = read_u16_le(bytes, i + 12),
      rows = read_u16_le(bytes, i + 14),
    }
    i = i + 16
  end
  return out
end

-- Serialized /dev/tty KGP writer + TIOCGWINSZ --------------------------------

local function ensure_tty()
  if not tty_fd then tty_fd = uv.fs_open("/dev/tty", "w", 0) end
  return tty_fd
end

local function drain_tty()
  if tty_flushing then return end
  local item = table.remove(tty_queue, 1)
  if not item then return end
  local fd = ensure_tty()
  if not fd then
    tty_queue = {}
    return
  end
  tty_flushing = true

  local function write_slice(data)
    uv.fs_write(fd, data, -1, function(err, n)
      if err or not n or n <= 0 then
        tty_flushing = false
        drain_tty()
        return
      end
      if n < #data then
        write_slice(string.sub(data, n + 1))
      else
        tty_flushing = false
        drain_tty()
      end
    end)
  end

  write_slice(item)
end

local function write_apc(apc)
  if apc and #apc > 0 then
    tty_queue[#tty_queue + 1] = apc
    drain_tty()
  end
end

local function cell_metrics()
  local cw, ch = 8, 16
  local fd = ensure_tty()
  if ok_ffi and fd then
    local req = ffi.os == "OSX" and 0x40087468 or 0x5413
    local ws = ffi.new("ii_winsize")
    if
      ffi.C.ioctl(fd, req, ws) == 0
      and ws.ws_col > 0
      and ws.ws_row > 0
      and ws.ws_xpixel > 0
      and ws.ws_ypixel > 0
    then
      cw = math.max(1, math.floor(ws.ws_xpixel / ws.ws_col))
      ch = math.max(1, math.floor(ws.ws_ypixel / ws.ws_row))
    end
  end
  return cw, ch
end

local function env_flags()
  local f = 0
  if vim.o.background == "light" then f = f + FLAG_LIGHT end
  if os.getenv("TMUX") then f = f + FLAG_TMUX end
  if os.getenv("SSH_CONNECTION") or os.getenv("SSH_CLIENT") or os.getenv("SSH_TTY") then
    f = f + FLAG_REMOTE
  end
  return f
end

-- Async Unix socket + lazy daemon bootstrap ----------------------------------

local function close_safe(h)
  if h and not h:is_closing() then h:close() end
end

local function spawn_daemon(on_ready)
  local handle
  handle = uv.spawn(cfg.bin, {
    args = { "--daemon" },
    detached = true,
    stdio = { nil, nil, nil },
  }, function() close_safe(handle) end)
  if not handle then
    on_ready("spawn failed")
    return
  end
  handle:unref()

  local sock = socket_path()
  local timer = uv.new_timer()
  local tries = 0
  timer:start(15, 25, function()
    tries = tries + 1
    if uv.fs_stat(sock) then
      timer:stop()
      close_safe(timer)
      on_ready(nil)
    elseif tries >= 40 then
      timer:stop()
      close_safe(timer)
      on_ready("socket timeout")
    end
  end)
end

local function send_once(sock, frame, cb)
  local pipe = uv.new_pipe(false)
  if not pipe then
    cb("new_pipe failed")
    return
  end
  local done = false
  local function finish(err, apc, text)
    if done then return end
    done = true
    pipe:read_stop()
    close_safe(pipe)
    cb(err, apc, text)
  end

  pipe:connect(sock, function(conn_err)
    if conn_err then
      finish(conn_err)
      return
    end
    local chunks, total, hdr = {}, 0, nil
    pipe:read_start(function(rerr, data)
      if rerr or not data then
        finish(rerr or "eof")
        return
      end
      chunks[#chunks + 1] = data
      total = total + #data
      if not hdr and total >= 32 then
        local flat = #chunks == 1 and chunks[1] or table.concat(chunks)
        chunks = { flat }
        local magic = read_u32_le(flat, 1)
        local ver = read_u16_le(flat, 5)
        if magic ~= MAGIC or ver ~= VERSION then
          finish("bad header")
          return
        end
        hdr = { command = read_u16_le(flat, 7), payload_len = read_u32_le(flat, 21) }
      end
      if hdr and total >= 32 + hdr.payload_len then
        local flat = #chunks == 1 and chunks[1] or table.concat(chunks)
        local body = string.sub(flat, 33, 32 + hdr.payload_len)
        if bit.band(hdr.command, ERR_FLAG) ~= 0 then
          finish(body)
          return
        end
        if #body < 4 then
          finish(nil, "", "")
          return
        end
        local apc_len = read_u32_le(body, 1)
        if 4 + apc_len > #body then
          finish("truncated dual response")
          return
        end
        finish(nil, string.sub(body, 5, 4 + apc_len), string.sub(body, 5 + apc_len))
      end
    end)
    pipe:write(frame, function(werr)
      if werr then finish(werr) end
    end)
  end)
end

local function ipc_request(cmd, opts, cb)
  local sock = socket_path()
  local payload = opts.payload or ""
  local hdr = encode_header(
    cmd,
    opts.flags or 0,
    opts.cols or 80,
    opts.rows or 24,
    opts.cell_w or 8,
    opts.cell_h or 16,
    #payload
  )
  local frame = payload ~= "" and { hdr, payload } or hdr

  send_once(sock, frame, function(err, apc, text)
    if not err then
      cb(nil, apc, text)
      return
    end
    if
      opts.auto_spawn == false
      or cmd == CMD_SHUTDOWN
      or (err ~= "ENOENT" and err ~= "ECONNREFUSED")
    then
      cb(err)
      return
    end
    spawn_daemon(function(serr)
      if serr then
        cb(serr)
      else
        send_once(sock, frame, cb)
      end
    end)
  end)
end

-- Extmark rendering & buffer lifecycle ---------------------------------------

local function ensure_hl(image_id)
  local rgb = image_id % 0x1000000
  local name = hl_cache[rgb]
  if not name then
    name = string.format("IiImg%06x", rgb)
    vim.api.nvim_set_hl(0, name, { fg = rgb })
    hl_cache[rgb] = name
  end
  return name
end

local function build_virt_lines(cols, rows, hl)
  local c = math.max(1, cols)
  local r = math.max(1, math.min(rows, #DIACRITICS))
  local tail = c > 1 and string.rep(PLACEHOLDER, c - 1) or ""
  local out = {}
  for row = 1, r do
    out[row] = { { PLACEHOLDER .. DIACRITICS[row] .. FIRST_COL .. tail, hl } }
  end
  return out
end

local function close_buf_state(buf)
  local st = buf_state[buf]
  if st then
    if st.timer then
      st.timer:stop()
      close_safe(st.timer)
    end
    buf_state[buf] = nil
  end
end

local function apply_marks(buf, records)
  if not vim.api.nvim_buf_is_valid(buf) then return end
  vim.api.nvim_buf_clear_namespace(buf, NS, 0, -1)
  if not cfg.enabled or #records == 0 then return end

  local max_row = vim.api.nvim_buf_line_count(buf) - 1
  local cur_row = (cfg.hide_on_cursor and vim.api.nvim_get_current_buf() == buf)
      and (vim.api.nvim_win_get_cursor(0)[1] - 1)
    or nil

  for _, em in ipairs(records) do
    local anchor = (em.end_col == 0 and em.end_row > em.start_row) and (em.end_row - 1)
      or em.end_row
    anchor = math.max(0, math.min(anchor, max_row))
    local hidden = cur_row ~= nil and cur_row >= em.start_row and cur_row <= anchor
    if not hidden and em.cols > 0 and em.rows > 0 then
      local hl = ensure_hl(em.image_id)
      vim.api.nvim_buf_set_extmark(buf, NS, anchor, 0, {
        virt_lines = build_virt_lines(em.cols, em.rows, hl),
        virt_lines_above = false,
      })
    end
  end
end

local function dispatch_render(buf)
  if not cfg.enabled or not vim.api.nvim_buf_is_valid(buf) then return end
  local st = buf_state[buf]
  if not st then return end
  if st.inflight then
    st.dirty = true
    return
  end
  local win = vim.fn.bufwinid(buf)
  if win == -1 then return end

  local payload = table.concat(vim.api.nvim_buf_get_lines(buf, 0, -1, false), "\n")
  local tick = vim.api.nvim_buf_get_changedtick(buf)
  local cw, ch = cell_metrics()

  st.inflight = true
  st.dirty = false
  st.tick = tick

  ipc_request(CMD_COMPOSE_EXTMARKS, {
    flags = env_flags(),
    cols = math.max(1, vim.api.nvim_win_get_width(win) - 4),
    rows = math.max(1, vim.api.nvim_win_get_height(win)),
    cell_w = cw,
    cell_h = ch,
    payload = payload,
  }, function(err, apc, text)
    vim.schedule(function()
      local cur = buf_state[buf]
      if not cur then return end
      cur.inflight = false
      if not vim.api.nvim_buf_is_valid(buf) then
        close_buf_state(buf)
        return
      end
      if err then return end
      if vim.api.nvim_buf_get_changedtick(buf) ~= tick then
        if cur.dirty then dispatch_render(buf) end
        return
      end
      write_apc(apc)
      cur.marks = decode_extmarks(text or "")
      apply_marks(buf, cur.marks)
      if cur.dirty then dispatch_render(buf) end
    end)
  end)
end

local function schedule_render(buf, immediate)
  if not cfg.enabled or not vim.api.nvim_buf_is_valid(buf) then return end
  if not cfg.filetypes[vim.bo[buf].filetype] then return end
  local st = buf_state[buf]
  if not st then
    st = { timer = uv.new_timer(), inflight = false, dirty = false, tick = -1, marks = {} }
    buf_state[buf] = st
  end
  st.timer:stop()
  st.timer:start(
    immediate and 0 or cfg.debounce_ms,
    0,
    vim.schedule_wrap(function() dispatch_render(buf) end)
  )
end

-- Public API -----------------------------------------------------------------

function M.refresh(buf) schedule_render(buf or vim.api.nvim_get_current_buf(), true) end

function M.toggle()
  cfg.enabled = not cfg.enabled
  if cfg.enabled then
    M.refresh(vim.api.nvim_get_current_buf())
  else
    for buf in pairs(buf_state) do
      if vim.api.nvim_buf_is_valid(buf) then vim.api.nvim_buf_clear_namespace(buf, NS, 0, -1) end
    end
  end
end

function M.shutdown()
  ipc_request(CMD_SHUTDOWN, { auto_spawn = false }, function() end)
end

function M.setup(user_opts)
  cfg = vim.tbl_deep_extend("force", DEFAULT_CFG, user_opts or {})
  vim.api.nvim_clear_autocmds { group = AUGROUP }

  vim.api.nvim_create_user_command("IiToggle", M.toggle, { desc = "Toggle ii inline graphics" })
  vim.api.nvim_create_user_command(
    "IiRefresh",
    function() M.refresh() end,
    { desc = "Re-render ii graphics" }
  )
  vim.api.nvim_create_user_command("IiShutdown", M.shutdown, { desc = "Stop ii daemon" })

  vim.api.nvim_create_autocmd({ "BufEnter", "BufWritePost", "InsertLeave", "VimResized" }, {
    group = AUGROUP,
    callback = function(ev) schedule_render(ev.buf, true) end,
  })
  vim.api.nvim_create_autocmd({ "TextChanged", "TextChangedI" }, {
    group = AUGROUP,
    callback = function(ev) schedule_render(ev.buf, false) end,
  })
  vim.api.nvim_create_autocmd("ColorScheme", {
    group = AUGROUP,
    callback = function()
      hl_cache = {}
      M.refresh(vim.api.nvim_get_current_buf())
    end,
  })
  if cfg.hide_on_cursor then
    vim.api.nvim_create_autocmd({ "CursorMoved", "CursorMovedI" }, {
      group = AUGROUP,
      callback = function(ev)
        local st = buf_state[ev.buf]
        if st and st.marks then apply_marks(ev.buf, st.marks) end
      end,
    })
  end
  vim.api.nvim_create_autocmd("BufWipeout", {
    group = AUGROUP,
    callback = function(ev)
      if vim.api.nvim_buf_is_valid(ev.buf) then
        vim.api.nvim_buf_clear_namespace(ev.buf, NS, 0, -1)
      end
      close_buf_state(ev.buf)
    end,
  })
  vim.api.nvim_create_autocmd("VimLeavePre", {
    group = AUGROUP,
    callback = function()
      for buf in pairs(buf_state) do
        close_buf_state(buf)
      end
      tty_queue = {}
      tty_flushing = false
      if tty_fd then
        pcall(uv.fs_close, tty_fd)
        tty_fd = nil
      end
    end,
  })

  -- Handle cold lazy-load on `ft` where `BufEnter` already fired.
  schedule_render(vim.api.nvim_get_current_buf(), true)
end

return M
