-- ii.nvim: Zero-overhead binary IPC coprocessor client for Neovim (0.10+)
-- Protocol: IXPC v1 (CMD_COMPOSE_EXTMARKS = 0x0003, 32-byte Header + Dual-Channel)
-- Transport: Persistent libuv pipe + Zero-Copy extmarks

local M = {}
local is_mac = vim.fn.has("mac") == 1
local cached_cell_w, cached_cell_h = is_mac and 18 or 9, is_mac and 36 or 18
local ns_id = vim.api.nvim_create_namespace("ii_renderer")
local hl_cache = {}

-- Kitty Graphics Protocol: 115 row combining diacritics for U=1 virtual placement
local PLACEHOLDER = "\244\142\187\174" -- U+10EEEE (UTF-8: F4 8E BB AE)
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

-- Little-endian packing helpers (LuaJIT / Lua 5.1 compatible, zero external dependencies)
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

-- Little-endian unpacking helpers
local function unpack_u16_le(str, offset)
  local b1, b2 = string.byte(str, offset, offset + 1)
  return b1 + b2 * 256
end

local function unpack_u32_le(str, offset)
  local b1, b2, b3, b4 = string.byte(str, offset, offset + 3)
  return b1 + b2 * 256 + b3 * 65536 + b4 * 16777216
end

-- Highlight group mapping image_id to foreground color
local function get_image_hl(image_id)
  local hl_name = string.format("IiImg_%06x", image_id)
  if not hl_cache[image_id] then
    vim.api.nvim_set_hl(0, hl_name, { fg = string.format("#%06x", image_id) })
    hl_cache[image_id] = hl_name
  end
  return hl_name
end

-- Resolve IXPC v1 socket path (main loop safe)
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

-- Terminal cell size probe with tmux split protection
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

-- Connect to daemon with transparent autospawn on failure
local function connect_daemon(sock_path, cb, retries)
  retries = retries or 0
  local pipe = vim.uv.new_pipe(false)

  pipe:connect(sock_path, function(err)
    if not err then
      cb(nil, pipe)
      return
    end

    pipe:close()
    if (err:match("ENOENT") or err:match("ECONNREFUSED")) and retries == 0 then
      vim.schedule(function()
        vim.system({ "ii", "--daemon", "--socket", sock_path }, { detach = true }, function(_) end)
      end)

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
          if not vim.api.nvim_buf_is_valid(bufnr) then return end
          if vim.api.nvim_buf_get_changedtick(bufnr) ~= tick then return end

          -- 1. Deliver KGP APC commands through terminal UI stream
          local apc_bytes = full_buf:sub(33, 32 + apc_len)
          if #apc_bytes > 0 then
            if vim.api.nvim_ui_send then
              vim.api.nvim_ui_send(apc_bytes)
            else
              io.stdout:write(apc_bytes)
            end
          end

          -- 2. Clear old extmarks in buffer
          vim.api.nvim_buf_clear_namespace(bufnr, ns_id, 0, -1)

          -- 3. Parse 16-byte ExtmarkRecord array
          local extmarks_offset = 33 + apc_len
          local extmarks_end = expected_len
          local offset = extmarks_offset

          while offset + 16 - 1 <= extmarks_end do
            local image_id = unpack_u32_le(full_buf, offset)
            local end_row = unpack_u16_le(full_buf, offset + 8)
            local m_cols = unpack_u16_le(full_buf, offset + 12)
            local m_rows = unpack_u16_le(full_buf, offset + 14)

            local hl_name = get_image_hl(image_id)

            -- Format rows of U+10EEEE virtual placeholders with KGP row combining diacritics
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

local buffer_enabled = {}

function M.clear(bufnr)
  bufnr = bufnr or vim.api.nvim_get_current_buf()
  if vim.api.nvim_buf_is_valid(bufnr) then
    vim.api.nvim_buf_clear_namespace(bufnr, ns_id, 0, -1)
  end
end

function M.toggle(bufnr)
  bufnr = bufnr or vim.api.nvim_get_current_buf()
  if buffer_enabled[bufnr] == false then
    buffer_enabled[bufnr] = true
    M.render(bufnr)
  else
    buffer_enabled[bufnr] = false
    M.clear(bufnr)
  end
end

function M.setup(opts)
  opts = opts or {}
  local debounce_ms = opts.debounce_ms or 20

  local timer = vim.uv.new_timer()
  local augroup = vim.api.nvim_create_augroup("IiAutoRender", { clear = true })
  vim.api.nvim_create_autocmd({ "BufEnter", "BufWritePost", "TextChanged", "TextChangedI" }, {
    group = augroup,
    callback = function(ev)
      if buffer_enabled[ev.buf] == false then return end
      timer:stop()
      timer:start(debounce_ms, 0, vim.schedule_wrap(function()
        if vim.api.nvim_buf_is_valid(ev.buf) then
          M.render(ev.buf)
        end
      end))
    end,
  })
end

-- Register standard commands and aliases
vim.api.nvim_create_user_command("IiRender", function() M.render() end, { desc = "Render buffer with ii coprocessor" })
vim.api.nvim_create_user_command("IiToggle", function() M.toggle() end, { desc = "Toggle ii KGP rendering" })
vim.api.nvim_create_user_command("IiClear", function() M.clear() end, { desc = "Clear ii extmarks" })
vim.api.nvim_create_user_command("PixxRender", function() M.render() end, { desc = "Render buffer with ii coprocessor" })
vim.api.nvim_create_user_command("PixxToggle", function() M.toggle() end, { desc = "Toggle ii KGP rendering" })
vim.api.nvim_create_user_command("PixxClear", function() M.clear() end, { desc = "Clear ii extmarks" })

return M
