-- ii.yazi: framed graphical preview adapter for Yazi.
-- Framed CLI output is committed on Yazi's main loop, never from a competing writer.

local M = {}

local emit = ya.mgr_emit or ya.emit or ya.manager_emit

-- Commit graphics on Yazi's main loop, after rejecting superseded jobs. Writing
-- from the CLI races Yazi's own terminal writer during rapid navigation.
local publish_graphics = ya.sync(function(_, url, skip, apc)
  local tab = cx.tab or cx.active
  local hovered = tab.current.hovered
  if not hovered or tostring(hovered.url) ~= url or tab.preview.skip ~= skip then return false end
  if #apc > 0 then
    local tty = io.open("/dev/tty", "wb")
    if not tty then return false end
    tty:write(apc); tty:flush(); tty:close()
  end
  return true
end)

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
    :arg("--machine")
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
    :stderr(Command.PIPED)
    :output()

  if not child or not child.status.success then
    ya.preview_widget(job, ui.Text("ii: " .. (child and child.stderr or "failed to start")):area(job.area))
    return
  end

  local raw = child.stdout
  if #raw < 4 then return end
  local a,b,c,d = raw:byte(1,4)
  local n = a+b*256+c*65536+d*16777216
  if n > #raw-4 then return end
  if not publish_graphics(tostring(job.file.url), job.skip or 0, raw:sub(5,4+n)) then return end
  local text = raw:sub(5+n)
  if text == "" and (job.skip or 0) > 0 then
    emit("peek", { math.max(0, job.skip - h), only_if = job.file.url, upper_bound = true })
    return
  end
  ya.preview_widget(job, ui.Text.parse(text):area(job.area))
end

function M:seek(job)
  local tab = cx.tab or cx.active
  local hovered = tab.current.hovered
  if not hovered or hovered.url ~= job.file.url then return end
  local step = math.floor(job.units * job.area.h / 10)
  if step == 0 then step = job.units > 0 and 1 or (job.units < 0 and -1 or 0) end
  emit("peek", { math.max(0, tab.preview.skip + step), only_if = job.file.url, scope = job.scope })
end

function M:preload(_) return true end

return M
