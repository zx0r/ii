vim.opt.rtp:append(vim.env.II_ROOT..'/plugins/ii.nvim')
vim.cmd('runtime! ftdetect/ii.lua')
vim.wo.number=true;vim.wo.relativenumber=true;vim.wo.signcolumn='yes:2';vim.wo.foldcolumn='2';vim.wo.winbar='source view';vim.wo.wrap=true
vim.cmd('edit '..vim.env.II_ROOT..'/showcase/scenarios/incident.md')
vim.o.mouse='a'
local ii=require('ii');ii.setup({socket_path=vim.env.II_SOCKET})
do
 local buf=vim.api.nvim_get_current_buf()
 vim.api.nvim_buf_set_lines(buf,0,-1,false,{
  'Before formula', '', '```math', [[\frac{1}{2}]], '```', '', 'After formula',
 })
 vim.treesitter.start(buf,'markdown')
 ii.render(buf,nil,true)
 assert(vim.wait(10000,function()return ii.status().graphics_bytes>0 and not ii.status().pending end,10))
 vim.cmd('normal! ggzt');vim.api.nvim__redraw({valid=false,flush=true})
 local images=0
 for row=1,15 do
  if vim.fn.screenstring(row,1):find('􎻮',1,true) then images=images+1 end
 end
 assert(images>=2,'Tree-sitter hid a row of the fenced formula: '..images)
 vim.cmd('normal! 4Gzt');vim.api.nvim__redraw({valid=false,flush=true})
 local first=vim.fn.screenstring(1,1)
 assert(first:find('􎻮̅̅',1,true),
  'scroll started at a continuation row of the formula')
 local source_highlight=false
 vim.api.nvim_create_autocmd('ModeChanged',{once=true,pattern='n:i',callback=function()
  source_highlight=vim.b[buf].ts_highlight==true
 end})
 vim.api.nvim_feedkeys('i'..string.char(27),'xt',false)
 assert(source_highlight,'Insert mode did not restore Tree-sitter')
 ii.clear(buf)
 assert(vim.b[buf].ts_highlight,'clearing preview did not restore Tree-sitter')
 vim.treesitter.stop(buf)
 vim.cmd('edit!')
end
for _,file in ipairs({'../01.md','../02.qmd','../03.ipynb','../04.tex','../11.mdx','../12.rmd','../13.latex','../14.typ','incident.md','incident.markdown','incident.mdx','incident.qmd','incident.rmd',
 'latency.plotly','latency.vl','latency.vega-lite','analysis.ipynb','telemetry.csv','telemetry.tsv',
 'kinetics.chem','equilibrium.mhchem','evidence.zip','evidence.tar','evidence.jar','evidence.tgz','evidence.tbz2','evidence.txz','evidence.tzst',
 'replicas.svg','incident-heatmap.png','rollout.dot','rollout.gv','checkout.mmd','checkout.mermaid','queueing.tex','queueing.latex','queueing.typ'}) do
 vim.cmd('edit '..vim.env.II_ROOT..'/showcase/scenarios/'..file)
 vim.wo.winbar='source view'
 local buf=vim.api.nvim_get_current_buf();local src=vim.api.nvim_buf_get_lines(buf,0,-1,false)
 local maps=vim.api.nvim_buf_get_keymap(buf,'n')
 local buffers=vim.api.nvim_list_bufs()
 assert(vim.wait(10000,function()return ii.status().source==buf and ii.status().blocks>0 and not ii.status().pending end,10),file)
 assert(vim.api.nvim_get_current_buf()==buf,'buffer was swapped')
 assert(vim.deep_equal(buffers,vim.api.nvim_list_bufs()),'renderer created a buffer')
 assert(vim.deep_equal(src,vim.api.nvim_buf_get_lines(buf,0,-1,false)),'source mutated')
 assert(vim.deep_equal(maps,vim.api.nvim_buf_get_keymap(buf,'n')),'keymaps changed')
 if file=='../03.ipynb' then
  local pieces={};local colors={}
  local ns=vim.api.nvim_get_namespaces()['ii_window_'..vim.api.nvim_get_current_win()]
  for _,mark in ipairs(vim.api.nvim_buf_get_extmarks(buf,ns,0,-1,{details=true})) do
   local lines={mark[4].virt_text or {}}
   for _,line in ipairs(mark[4].virt_lines or {}) do lines[#lines+1]=line end
   for _,line in ipairs(lines) do for _,chunk in ipairs(line) do
    pieces[#pieces+1]=chunk[1]
    if chunk[1]:find('􎻮',1,true) then colors[chunk[2]]=true end
   end end
  end
  local text=table.concat(pieces,'\n');local images=0;for _ in pairs(colors) do images=images+1 end
  assert(not text:find('"colorscale"',1,true),'Plotly heatmap fell back to raw JSON')
  assert(images>=8,'notebook lost graphical outputs: '..images)
 end
 local native=file:match('%.csv$') or file:match('%.tsv$') or file:match('evidence%.')
 if not native then assert(ii.status().graphics_bytes>0,'no graphics for '..file) end
 if file=='../01.md' or file=='../02.qmd' or file=='../12.rmd' then
  vim.cmd('normal! ggzt');vim.api.nvim__redraw({valid=false,flush=true})
  local top={}
  local graphic_cells=0
  for row=1,vim.o.lines-2 do
   local line={}
   for col=1,vim.o.columns do
    local cell=vim.fn.screenstring(row,col)
    if cell:find('􎻮',1,true) then
     graphic_cells=graphic_cells+1
     assert(vim.fn.strchars(cell)>=3,'graphic cell lost its coordinates during redraw')
    end
    line[#line+1]=cell
   end
   top[#top+1]=table.concat(line)
  end
  assert(graphic_cells>0,'no formula cells visible: '..file)
  local text=table.concat(top,'\n')
  assert(not text:find('\\frac',1,true),'raw math leaked into Normal: '..file..'\n'..text)
  if vim.env.II_SCREEN_CAPTURE then vim.fn.writefile(top,vim.env.II_SCREEN_CAPTURE..'/'..file:gsub('[^%w._-]','_')..'.top.txt') end
 end
 if file=='../02.qmd' then
  vim.cmd('vsplit')
  local split=vim.api.nvim_get_current_win()
  assert(vim.wait(10000,function()return ii.status(split).cols==vim.api.nvim_win_get_width(split) and not ii.status(split).pending end,10),'split width did not update: '..vim.inspect(ii.status(split))..' width='..vim.api.nvim_win_get_width(split))
  vim.cmd('normal! ggzt');vim.api.nvim__redraw({valid=false,flush=true})
  local origin=vim.api.nvim_win_get_position(split)
  for col=1,vim.api.nvim_win_get_width(split) do
   assert(vim.fn.screenstring(origin[1]+1,origin[2]+col)=='─','frontmatter rule does not fit split')
  end
  local source_profile=false
  vim.api.nvim_create_autocmd('ModeChanged',{once=true,pattern='n:i',callback=function() source_profile=vim.wo.number and vim.wo.relativenumber and vim.wo.winbar=='source view' end})
  vim.api.nvim_feedkeys('i'..string.char(27),'xt',false)
  assert(source_profile,'new split lost the source window options')
  vim.cmd('close')
  assert(vim.wait(10000,function()return ii.status().cols==vim.api.nvim_win_get_width(0) and not ii.status().pending end,10),'width was not restored after closing split')
  vim.cmd('normal! 33Gzt')
 end
 vim.api.nvim__redraw({valid=false,flush=true})
 local screen={}
 for row=1,vim.o.lines-2 do
  local line={};for col=1,vim.o.columns do line[#line+1]=vim.fn.screenstring(row,col) end
  screen[#screen+1]=table.concat(line)
 end
 local visible=table.concat(screen,'\n')
 assert(vim.api.nvim_get_hl(0,{name="Ii_"..vim.api.nvim_get_current_win().."_1",link=false}).bg==nil,"renderer added a highlight background")
 if file=='../02.qmd' then
  for _,value in ipairs({'Parameter','0.452','1.91','3.18','Reporting rate','0.31'}) do
   assert(visible:find(value,1,true),'QMD table body missing: '..value)
  end
 elseif file=='../11.mdx' then
  assert(visible:find('Exoplanet Transit',1,true),'MDX body missing')
  assert(not visible:find('import { LightCurve }',1,true),'MDX module import leaked into Normal')
  assert(table.concat(src,'\n'):find('import { LightCurve }',1,true),'MDX source was changed')
 elseif file:match('^incident%.m') or file:match('^incident%.[qr]md$') then
  assert(visible:find('󰲡 Checkout',1,true),'Markdown heading is not visible')
  assert(not visible:find('# Checkout',1,true),'raw Markdown leaked into Normal mode')
 elseif file=='telemetry.csv' or file=='telemetry.tsv' then
  assert(vim.fn.screenstring(1,1)=='┌','table has a left or top gutter')
  assert(vim.fn.screenstring(1,vim.o.columns)=='┐','table does not fill the window width')
 elseif file=='queueing.tex' or file=='queueing.latex' then
  assert(visible:find('Queue saturation',1,true),'LaTeX document heading missing')
  assert(not visible:find('documentclass',1,true),'LaTeX preamble leaked into rendered view')
 elseif file=='queueing.typ' then
  assert(visible:find('Queue saturation',1,true),'Typst heading missing')
 elseif file=='analysis.ipynb' then
  assert(visible:find('Checkpoint 00',1,true),'notebook content is not visible')
  assert(not visible:find('"nbformat"',1,true),'notebook JSON leaked into Normal mode')
 end
 if vim.env.II_SCREEN_CAPTURE then vim.fn.writefile(screen,vim.env.II_SCREEN_CAPTURE..'/'..file:gsub('[^%w._-]','_')..'.txt') end
 if file=='analysis.ipynb' then
  local trace={}
  for step=1,80 do
   vim.api.nvim_feedkeys(vim.api.nvim_replace_termcodes('<C-e>',true,false,true),'xt',false);vim.wait(1)
   vim.api.nvim__redraw({valid=false,flush=true})
   trace[#trace+1]={cursor=vim.api.nvim_win_get_cursor(0),view=vim.fn.winsaveview()}
  end
  if vim.env.II_SCREEN_CAPTURE then vim.fn.writefile({vim.json.encode(trace)},vim.env.II_SCREEN_CAPTURE..'/scroll.json') end
  assert(trace[2].cursor[1]==3,'notebook cursor jumped across hidden JSON')
  assert(trace[80].cursor[1]<vim.api.nvim_buf_line_count(buf)-30,'notebook skipped to EOF')
  assert(not ii.status().pending,'scroll triggered IPC')
  vim.cmd('normal! gg')
 end
 ii.render(buf)
 assert(not ii.status().pending,'unchanged frame was rendered again')
 for step=1,40 do
  vim.api.nvim_input_mouse('move','','',0,3+step%15,3+step%30)
  vim.wait(1)
  vim.api.nvim__redraw({valid=false,flush=true})
 end
 for _,direction in ipairs({'down','up'}) do
  vim.api.nvim_input_mouse('wheel',direction,'',0,5,5)
  vim.api.nvim_feedkeys('','x',false)
  vim.wait(10)
 end
 assert(not ii.status().pending,'redraw scheduled rendering')
 vim.api.nvim_feedkeys('iTEST'..string.char(27),'xt',false)
 assert(vim.fn.mode()=='n','Escape did not restore Normal')
 assert(table.concat(vim.api.nvim_buf_get_lines(buf,0,-1,false),'\n'):find('TEST'),'native insert failed')
 ii.clear(buf)
 assert(vim.wo.number and vim.wo.relativenumber and vim.wo.signcolumn=="yes:2" and vim.wo.winbar=="source view" and vim.wo.wrap,"source window options were not restored: "..vim.inspect({number=vim.wo.number,relativenumber=vim.wo.relativenumber,signcolumn=vim.wo.signcolumn,winbar=vim.wo.winbar,wrap=vim.wo.wrap}))
end
vim.cmd('qa!')
