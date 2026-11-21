-- ii.yazi: Zero-overhead previewer/preloader plugin for Yazi.
-- Automated Dual-Channel decoding over piped stdout [u32 apc_len (LE)][apc][text].

local M = {}

local emit = ya.mgr_emit or ya.emit or ya.manager_emit

local function unpack_u32_le(raw, offset)
  local b1, b2, b3, b4 = string.byte(raw, offset, offset + 3)
  return b1 + b2 * 256 + b3 * 65536 + b4 * 16777216
end

local function path_of(job)
  return (tostring(job.file.path or job.file.url):gsub("^file://", ""))
end

function M:peek(job)
  local path = path_of(job)
  local w = (job.area and job.area.w and job.area.w > 0) and job.area.w or 80
  local h = (job.area and job.area.h and job.area.h > 0) and job.area.h or 24

  local cw, ch
  if rt and rt.term and rt.term.cell_size then
    cw, ch = rt.term.cell_size()
  end
  local cell_size_arg = (cw and ch) and string.format("%dx%d", math.floor(cw), math.floor(ch)) or "18x36"
  local is_light = (rt and rt.term and rt.term.light and rt.term.light()) or false
  local theme_arg = is_light and "light" or "dark"

  local child, _ = Command("ii")
    :arg("--size")
    :arg(string.format("%dx%d", w, h))
    :arg("--cell-size")
    :arg(cell_size_arg)
    :arg("--skip")
    :arg(tostring(job.skip or 0))
    :arg("--theme")
    :arg(theme_arg)
    :arg("--")
    :arg(path)
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

  if #apc > 0 and emit then
    emit("term_write", { apc })
  end
  ya.preview_widget(job, ui.Text.parse(text):area(job.area))
end

function M:seek(job)
  self:peek(job)
end

return M
