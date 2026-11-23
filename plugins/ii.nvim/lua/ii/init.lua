-- ii.nvim: bounded asynchronous IXPC client for Neovim (0.12+)
-- Protocol: IXPC v1 (CMD_COMPOSE_EXTMARKS = 0x0003, 32-byte Header + Dual-Channel)
-- One request per window. Source data stays owned by Neovim.

local M = {}
local is_mac = vim.fn.has("mac") == 1
local cached_cell_w, cached_cell_h = is_mac and 18 or 9, is_mac and 36 or 18
local ns_id = vim.api.nvim_create_namespace("ii_renderer")
local style_ns = vim.api.nvim_create_namespace("ii_presentation")
local hl_cache = {}
local frames = {}
local deadlines = {}
local options = { executable = "ii", timeout_ms = 5000 }
local buffer_enabled = {}
local generations = {}
local requests = {}
local timers = {}

local function style_buffer(bufnr)
  if buffer_enabled[bufnr] == false then return end
  vim.api.nvim_buf_clear_namespace(bufnr, style_ns, 0, -1)
  local ft = vim.bo[bufnr].filetype
  if ft ~= "markdown" and ft ~= "md" and ft ~= "mdx" and ft ~= "quarto" and ft ~= "qmd" and ft ~= "rmd" then return end
  local win = vim.fn.bufwinid(bufnr)
  local cursor = win ~= -1 and vim.api.nvim_win_get_cursor(win)[1] - 1 or -1
  local lines = vim.api.nvim_buf_get_lines(bufnr, 0, -1, false)
  local calls = {
    NOTE = { "󰋽 Note", "IiInfo" }, TIP = { "󰌶 Tip", "IiSuccess" }, IMPORTANT = { "󰅾 Important", "IiAccent" },
    WARNING = { "󰀪 Warning", "IiWarn" }, CAUTION = { "󰳦 Caution", "IiDanger" }, INFO = { "󰋽 Info", "IiInfo" },
    ABSTRACT = { "󰨸 Abstract", "IiInfo" }, SUMMARY = { "󰨸 Summary", "IiInfo" }, TLDR = { "󰨸 Tldr", "IiInfo" },
    SUCCESS = { "󰄬 Success", "IiSuccess" }, TODO = { "󰗡 Todo", "IiWarn" }, QUESTION = { "󰘥 Question", "IiWarn" },
    FAILURE = { "󰅖 Failure", "IiDanger" }, DANGER = { "󱐌 Danger", "IiDanger" }, BUG = { "󰨰 Bug", "IiDanger" },
    EXAMPLE = { "󰉹 Example", "IiAccent" }, QUOTE = { "󱆨 Quote", "IiMuted" },
  }
  local in_fence = false
  for row, line in ipairs(lines) do
    if line:match("^%s*```") or line:match("^%s*~~~") then in_fence = not in_fence end
    local r = row - 1
    if r ~= cursor and not in_fence then
      local marks, body = line:match("^(#+)%s+(.*)$")
      if marks and #marks <= 6 then
        vim.api.nvim_buf_set_extmark(bufnr, style_ns, r, 0, { end_col = #marks + 1, virt_text = { { ({"󰲡 ","󰲣 ","󰲥 ","󰲧 ","󰲩 ","󰲫 "})[#marks], "IiH" .. #marks } }, virt_text_pos = "overlay" })
      else
        local indent, tag = line:match("^(%s*)>%s*%[!([%w]+)%]")
        local call = tag and calls[tag:upper()] or nil
        if indent and call then
          vim.api.nvim_buf_set_extmark(bufnr, style_ns, r, #indent, { end_col = #line, virt_text = { { "│ " .. call[1], call[2] } }, virt_text_pos = "overlay" })
        else
          local prefix, state = line:match("^(%s*[-*]%s+)%[([ xX%!~%-])%]")
          if prefix and state then
            local icon, group = "󰄱 ", "IiMuted"
            if state == "x" or state == "X" then icon, group = "󰄵 ", "IiSuccess"
            elseif state == "-" then icon, group = "󰥔 ", "IiWarn"
            elseif state == "!" then icon, group = "󰀨 ", "IiDanger"
            elseif state == "~" then icon = "󰰱 " end
            vim.api.nvim_buf_set_extmark(bufnr, style_ns, r, #prefix, { end_col = #prefix + 3, virt_text = { { icon, group } }, virt_text_pos = "overlay" })
          end
        end
      end
    end
  end
end

local function init_highlights()
  -- Match ii_old's default Catppuccin palettes. No group sets a background.
  local dark = vim.o.background ~= "light"
  local c = dark and { text="#cdd6f4", muted="#a1a7c2", info="#89b4fa", mauve="#cba6f7", peach="#fab387", teal="#94e2d5", success="#a6e3a1", warn="#f9e2af", danger="#f38ba8" }
    or { text="#4c4f69", muted="#74778c", info="#1e66f5", mauve="#8839ef", peach="#fe640b", teal="#179299", success="#40a02b", warn="#df8e1d", danger="#d20f39" }
  local colors = { IiH1=c.info, IiH2=c.mauve, IiH3=c.peach, IiH4=c.teal, IiH5=c.muted, IiH6=c.muted,
    IiInfo=c.info, IiAccent=c.teal, IiSuccess=c.success, IiWarn=c.warn, IiDanger=c.danger, IiMuted=c.muted }
  for name, fg in pairs(colors) do vim.api.nvim_set_hl(0, name, { fg=fg, bold=name:match("^IiH") ~= nil }) end
  hl_cache = {}
end

-- Kitty Graphics Protocol: 297 row combining diacritics for U=1 virtual placement
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
  0x0818, 0x0819, 0x081B, 0x081C, 0x081D, 0x081E, 0x081F, 0x0820, 0x0821, 0x0822,
  0x0823, 0x0825, 0x0826, 0x0827, 0x0829, 0x082A, 0x082B, 0x082C, 0x082D, 0x0951,
  0x0953, 0x0954, 0x0F82, 0x0F83, 0x0F86, 0x0F87, 0x135D, 0x135E, 0x135F, 0x17DD,
  0x193A, 0x1A17, 0x1A75, 0x1A76, 0x1A77, 0x1A78, 0x1A79, 0x1A7A, 0x1A7B, 0x1A7C,
  0x1B6B, 0x1B6D, 0x1B6E, 0x1B6F, 0x1B70, 0x1B71, 0x1B72, 0x1B73, 0x1CD0, 0x1CD1,
  0x1CD2, 0x1CDA, 0x1CDB, 0x1CE0, 0x1DC0, 0x1DC1, 0x1DC3, 0x1DC4, 0x1DC5, 0x1DC6,
  0x1DC7, 0x1DC8, 0x1DC9, 0x1DCB, 0x1DCC, 0x1DD1, 0x1DD2, 0x1DD3, 0x1DD4, 0x1DD5,
  0x1DD6, 0x1DD7, 0x1DD8, 0x1DD9, 0x1DDA, 0x1DDB, 0x1DDC, 0x1DDD, 0x1DDE, 0x1DDF,
  0x1DE0, 0x1DE1, 0x1DE2, 0x1DE3, 0x1DE4, 0x1DE5, 0x1DE6, 0x1DFE, 0x20D0, 0x20D1,
  0x20D4, 0x20D5, 0x20D6, 0x20D7, 0x20DB, 0x20DC, 0x20E1, 0x20E7, 0x20E9, 0x20F0,
  0x2CEF, 0x2CF0, 0x2CF1, 0x2DE0, 0x2DE1, 0x2DE2, 0x2DE3, 0x2DE4, 0x2DE5, 0x2DE6,
  0x2DE7, 0x2DE8, 0x2DE9, 0x2DEA, 0x2DEB, 0x2DEC, 0x2DED, 0x2DEE, 0x2DEF, 0x2DF0,
  0x2DF1, 0x2DF2, 0x2DF3, 0x2DF4, 0x2DF5, 0x2DF6, 0x2DF7, 0x2DF8, 0x2DF9, 0x2DFA,
  0x2DFB, 0x2DFC, 0x2DFD, 0x2DFE, 0x2DFF, 0xA66F, 0xA67C, 0xA67D, 0xA6F0, 0xA6F1,
  0xA8E0, 0xA8E1, 0xA8E2, 0xA8E3, 0xA8E4, 0xA8E5, 0xA8E6, 0xA8E7, 0xA8E8, 0xA8E9,
  0xA8EA, 0xA8EB, 0xA8EC, 0xA8ED, 0xA8EE, 0xA8EF, 0xA8F0, 0xA8F1, 0xAAB0, 0xAAB2,
  0xAAB3, 0xAAB7, 0xAAB8, 0xAABE, 0xAABF, 0xAAC1, 0xFE20, 0xFE21, 0xFE22, 0xFE23,
  0xFE24, 0xFE25, 0xFE26, 0x10A0F, 0x10A38, 0x1D185, 0x1D186, 0x1D187, 0x1D188, 0x1D189,
  0x1D1AA, 0x1D1AB, 0x1D1AC, 0x1D1AD, 0x1D242, 0x1D243, 0x1D244,
}

local function utf8_encode(cp)
  if cp < 128 then
    return string.char(cp)
  elseif cp < 2048 then
    return string.char(192 + math.floor(cp / 64), 128 + (cp % 64))
  elseif cp < 65536 then
    return string.char(224 + math.floor(cp / 4096), 128 + math.floor((cp % 4096) / 64), 128 + (cp % 64))
  else
    return string.char(
      240 + math.floor(cp / 262144),
      128 + math.floor((cp % 262144) / 4096),
      128 + math.floor((cp % 4096) / 64),
      128 + (cp % 64)
    )
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
local function get_image_hl(image_id, win, index)
  local hl_name = string.format("IiImg_%d_%d", win, index)
  if hl_cache[hl_name] ~= image_id then
    vim.api.nvim_set_hl(0, hl_name, { fg = string.format("#%06x", image_id) })
    hl_cache[hl_name] = image_id
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
  local uid = vim.uv.getuid and vim.uv.getuid() or 1000
  return string.format("/tmp/ii-%d/ii.sock", uid)
end

-- Terminal cell size probe with tmux split protection
local function update_cell_metrics()
  if vim.env.TMUX then
    cached_cell_w, cached_cell_h = is_mac and 18 or 9, is_mac and 36 or 18
    return
  end

  local ffi = require("ffi")
  pcall(ffi.cdef, [[
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
    if (err:match("ENOENT") or err:match("ECONNREFUSED")) and retries < 5 then
      if retries == 0 then vim.schedule(function()
        vim.system({ options.executable, "--daemon", "--socket", sock_path }, { detach = true }, function(_) end)
      end) end

      local timer = vim.uv.new_timer()
      timer:start(40 * (retries + 1), 0, function()
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

  if not vim.api.nvim_buf_is_valid(bufnr) or not vim.api.nvim_win_is_valid(win) or buffer_enabled[bufnr] == false then return end
  generations[win] = (generations[win] or 0) + 1
  local generation = generations[win]
  if requests[win] and not requests[win]:is_closing() then requests[win]:close() end
  requests[win] = nil
  if deadlines[win] then deadlines[win]:stop(); deadlines[win]:close(); deadlines[win] = nil end
  local deadline = vim.uv.new_timer()
  deadlines[win] = deadline
  deadline:start(options.timeout_ms, 0, vim.schedule_wrap(function()
    if generations[win] ~= generation then return end
    generations[win] = generation + 1
    if requests[win] and not requests[win]:is_closing() then requests[win]:close() end
    requests[win] = nil
    frames[win] = nil
    deadline:stop(); deadline:close(); deadlines[win] = nil
    vim.notify("ii: render request timed out", vim.log.levels.WARN)
  end))
  local tick = vim.api.nvim_buf_get_changedtick(bufnr)

  local wininfo = vim.fn.getwininfo(win)[1]
  local textoff = wininfo and wininfo.textoff or 0
  local cols = math.max(1, vim.api.nvim_win_get_width(win) - textoff)
  local rows = math.max(1, vim.api.nvim_win_get_height(win))
  local sock_path = get_socket_path()

  -- Replies can be cancelled before delivery: Base64 has no orphaned SHM lifetime.
  local flags = 4
  if vim.o.background == "light" then flags = flags + 1 end            -- FLAG_THEME_LIGHT
  if vim.env.TMUX then flags = flags + 2 end                           -- FLAG_TMUX

  local lines = vim.api.nvim_buf_get_lines(bufnr, 0, -1, false)
  local payload = table.concat(lines, "\n")
  local payload_len = #payload
  if payload_len > 67108864 then
    deadline:stop(); deadline:close(); deadlines[win] = nil
    vim.notify("ii: buffer exceeds request limit", vim.log.levels.WARN)
    return
  end

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
    if err or not pipe then
      if deadlines[win] == deadline then deadline:stop(); deadline:close(); deadlines[win] = nil end
      vim.schedule(function() vim.notify("ii: cannot connect to daemon: " .. tostring(err), vim.log.levels.WARN) end)
      return
    end
    if generations[win] ~= generation then pipe:close(); return end
    requests[win] = pipe
    local rx_chunks = {}
    local total_read = 0
    local expected_len = nil
    local apc_len = nil

    local function failed(message)
      if not pipe:is_closing() then pipe:close() end
      if generations[win] ~= generation then return end
      requests[win] = nil
      if deadlines[win] == deadline then deadline:stop(); deadline:close(); deadlines[win] = nil end
      vim.schedule(function() vim.notify("ii: " .. message, vim.log.levels.WARN) end)
    end
    pipe:read_start(function(read_err, chunk)
      if read_err or not chunk then
        failed(tostring(read_err or "incomplete response"))
        return
      end

      table.insert(rx_chunks, chunk)
      total_read = total_read + #chunk
      if total_read > 67108896 then failed("response exceeds limit"); return end

      if not expected_len and total_read >= 32 then
        local full_prefix = table.concat(rx_chunks)
        local resp_payload_len = unpack_u32_le(full_prefix, 21)
        apc_len = unpack_u32_le(full_prefix, 17)
        local magic = unpack_u32_le(full_prefix, 1)
        local version = unpack_u16_le(full_prefix, 5)
        local command = unpack_u16_le(full_prefix, 7)
        if unpack_u32_le(full_prefix, 9) ~= bufnr or unpack_u32_le(full_prefix, 13) ~= tick or magic ~= 0x49585043 or version ~= 1 or command ~= 0x8003 or resp_payload_len > 67108864 or apc_len > resp_payload_len or (resp_payload_len - apc_len) % 16 ~= 0 then
          failed("invalid render response"); return
        end
        expected_len = 32 + resp_payload_len
      end

      if expected_len and total_read > expected_len then failed("trailing response bytes"); return end
      if expected_len and total_read == expected_len then
        pipe:read_stop()
        pipe:close()

        requests[win] = nil
        local full_buf = table.concat(rx_chunks)
        vim.schedule(function()
          if deadlines[win] == deadline then deadline:stop(); deadline:close(); deadlines[win] = nil end
          if not vim.api.nvim_buf_is_valid(bufnr) then return end
          if vim.api.nvim_buf_get_changedtick(bufnr) ~= tick or generations[win] ~= generation or buffer_enabled[bufnr] == false then return end
          if not vim.api.nvim_win_is_valid(win) or vim.api.nvim_win_get_buf(win) ~= bufnr then return end
          if math.max(1, vim.api.nvim_win_get_width(win) - textoff) ~= cols or vim.api.nvim_win_get_height(win) ~= rows then return end

          -- 1. Deliver KGP APC commands through terminal UI stream
          local apc_bytes = full_buf:sub(33, 32 + apc_len)
          if #apc_bytes > 0 and #vim.api.nvim_list_uis() > 0 then
            vim.api.nvim_ui_send(apc_bytes)
          end

          -- 2. Clear old extmarks in buffer
          local marks = {}
          style_buffer(bufnr)

          -- 3. Parse 16-byte ExtmarkRecord array
          local extmarks_offset = 33 + apc_len
          local extmarks_end = expected_len
          local offset = extmarks_offset

          while offset + 16 - 1 <= extmarks_end do
            local image_id = unpack_u32_le(full_buf, offset)
            local start_row = unpack_u16_le(full_buf, offset + 4)
            local end_row = unpack_u16_le(full_buf, offset + 8)
            local end_col = unpack_u16_le(full_buf, offset + 10)
            -- The wire endpoint is exclusive; a consumed newline belongs to the
            -- closing source line, not the next paragraph's decoration anchor.
            if end_col == 0 and end_row > start_row then end_row = end_row - 1 end
            local m_cols = unpack_u16_le(full_buf, offset + 12)
            local m_rows = unpack_u16_le(full_buf, offset + 14)

            local hl_name = get_image_hl(image_id, win, #marks + 1)

            -- Format rows of U+10EEEE virtual placeholders with KGP row combining diacritics
            local virt_lines = {}
            for r = 0, m_rows - 1 do
              local diacritic = DIACRITICS[math.min(r + 1, #DIACRITICS)] or ""
              local first_cell = PLACEHOLDER .. diacritic .. DIACRITICS[1]
              local tail = m_cols > 1 and string.rep(PLACEHOLDER, m_cols - 1) or ""
              table.insert(virt_lines, { { first_cell .. tail, hl_name } })
            end

            if #virt_lines > 0 then
              end_row = math.min(end_row, vim.api.nvim_buf_line_count(bufnr) - 1)
              marks[#marks + 1] = { row = end_row, lines = virt_lines }
            end

            offset = offset + 16
          end
          frames[win] = { bufnr = bufnr, tick = tick, cols = cols, marks = marks }
          vim.cmd("redraw")
        end)
      end
    end)

    pipe:write(header .. payload, function(write_err) if write_err then failed(tostring(write_err)) end end)
  end)
end

function M.clear(bufnr)
  bufnr = bufnr or vim.api.nvim_get_current_buf()
  for win, frame in pairs(frames) do
    if frame.bufnr == bufnr then frames[win] = nil end
  end
  for win, pipe in pairs(requests) do
    if not vim.api.nvim_win_is_valid(win) or vim.api.nvim_win_get_buf(win) == bufnr then
      generations[win] = (generations[win] or 0) + 1
      if not pipe:is_closing() then pipe:close() end
      requests[win] = nil
    end
  end
  for win, deadline in pairs(deadlines) do
    if not vim.api.nvim_win_is_valid(win) or vim.api.nvim_win_get_buf(win) == bufnr then
      generations[win] = (generations[win] or 0) + 1
      deadline:stop(); deadline:close(); deadlines[win] = nil
    end
  end
  if vim.api.nvim_buf_is_valid(bufnr) then
    vim.api.nvim_buf_clear_namespace(bufnr, ns_id, 0, -1)
    vim.api.nvim_buf_clear_namespace(bufnr, style_ns, 0, -1)
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
  assert(vim.api.nvim_ui_send, "ii: Neovim 0.12+ is required for ordered terminal graphics output")
  opts = opts or {}
  local debounce_ms = opts.debounce_ms or 20
  options.executable = opts.executable or options.executable
  options.timeout_ms = opts.timeout_ms or options.timeout_ms
  if opts.socket_path then vim.g.ii_socket_path = opts.socket_path end
  assert(options.timeout_ms > 0 and debounce_ms >= 0, "ii: invalid timer settings")
  init_highlights()

  local augroup = vim.api.nvim_create_augroup("IiAutoRender", { clear = true })
  vim.api.nvim_create_autocmd({ "BufEnter", "BufWritePost", "TextChanged", "TextChangedI", "WinResized", "ColorScheme" }, {
    group = augroup,
    callback = function(ev)
      local bufnr = vim.api.nvim_get_current_buf()
      local ft = vim.bo[bufnr].filetype
      if not ({ markdown=true, md=true, mdx=true, quarto=true, qmd=true, rmd=true })[ft] then return end
      if buffer_enabled[bufnr] == false then return end
      if ev.event == "ColorScheme" then init_highlights() end
      ev.buf = bufnr
      local timer = timers[ev.buf] or vim.uv.new_timer()
      timers[ev.buf] = timer
      timer:stop()
      timer:start(debounce_ms, 0, vim.schedule_wrap(function()
        if vim.api.nvim_buf_is_valid(ev.buf) then
          for _, win in ipairs(vim.fn.win_findbuf(ev.buf)) do M.render(ev.buf, win) end
        end
      end))
    end,
  })
  vim.api.nvim_create_autocmd("BufWipeout", {
    group = augroup,
    callback = function(ev)
      M.clear(ev.buf)
      if timers[ev.buf] then timers[ev.buf]:stop(); timers[ev.buf]:close(); timers[ev.buf] = nil end
      buffer_enabled[ev.buf] = nil
    end,
  })
  vim.api.nvim_create_autocmd("WinClosed", {
    group = augroup,
    callback = function(ev)
      local win = tonumber(ev.match)
      generations[win] = (generations[win] or 0) + 1
      frames[win] = nil
      if requests[win] and not requests[win]:is_closing() then requests[win]:close() end
      requests[win] = nil
      if deadlines[win] then deadlines[win]:stop(); deadlines[win]:close(); deadlines[win] = nil end
    end,
  })
  vim.api.nvim_set_decoration_provider(ns_id, {
    on_win = function(_, win, bufnr)
      local frame = frames[win]
      if not frame or frame.bufnr ~= bufnr or buffer_enabled[bufnr] == false or frame.tick ~= vim.api.nvim_buf_get_changedtick(bufnr) then return false end
      for _, mark in ipairs(frame.marks) do
        vim.api.nvim_buf_set_extmark(bufnr, ns_id, mark.row, 0, {
          ephemeral = true, virt_lines = mark.lines, virt_lines_above = false,
        })
      end
      return false
    end,
  })
  vim.api.nvim_create_autocmd("CursorMoved", {
    group = augroup,
    callback = function(ev) if buffer_enabled[ev.buf] ~= false then style_buffer(ev.buf) end end,
  })
end

-- Register standard commands and aliases
vim.api.nvim_create_user_command("IiRender", function() M.render() end, { desc = "Render buffer with ii coprocessor" })
vim.api.nvim_create_user_command("IiToggle", function() M.toggle() end, { desc = "Toggle ii KGP rendering" })
vim.api.nvim_create_user_command("IiClear", function() M.clear() end, { desc = "Clear ii extmarks" })
vim.api.nvim_create_user_command("PixxRender", function() M.render() end, { desc = "Render buffer with ii coprocessor" })
vim.api.nvim_create_user_command("PixxToggle", function() M.toggle() end, { desc = "Toggle ii KGP rendering" })
vim.api.nvim_create_user_command("PixxClear", function() M.clear() end, { desc = "Clear ii extmarks" })

--- Current window render state, suitable for diagnostics and health checks.
function M.status(win)
  win = win or vim.api.nvim_get_current_win()
  local frame = frames[win]
  return { pending = deadlines[win] ~= nil, placements = frame and #frame.marks or 0,
    source_tick = frame and frame.tick or nil, cols = frame and frame.cols or nil }
end

return M
