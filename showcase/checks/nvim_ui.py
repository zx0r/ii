#!/usr/bin/env python3
"""Run the real Neovim TUI, renderer and native editing checks in a PTY."""
import argparse
import fcntl
import importlib.util
import os
from pathlib import Path
import pty
import select
import struct
import subprocess
import tempfile
import termios
import time

ROOT=Path(__file__).resolve().parents[2]
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--binary',type=Path,default=ROOT/'target/debug/ii')
args=parser.parse_args()
spec=importlib.util.spec_from_file_location('terminal_peer',Path(__file__).with_name('yazi.py'))
peer_module=importlib.util.module_from_spec(spec)
spec.loader.exec_module(peer_module)

with tempfile.TemporaryDirectory(prefix='ii-nvim-ui-') as tmp:
    sock=str(Path(tmp)/'ii.sock')
    daemon=subprocess.Popen([str(args.binary.resolve()),'--daemon','--socket',sock],stdout=subprocess.DEVNULL,stderr=subprocess.PIPE,env=dict(os.environ,XDG_CACHE_HOME=str(Path(tmp)/'cache')))
    master,slave=pty.openpty()
    fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',40,120,1080,720))
    def session():
        os.setsid();fcntl.ioctl(slave,termios.TIOCSCTTY,0)
    env=dict(os.environ,TERM='xterm-kitty',II_ROOT=str(ROOT),II_BINARY=str(args.binary.resolve()),II_SOCKET=sock,II_FAILURE=str(Path(tmp)/'failure.txt'),XDG_STATE_HOME=str(Path(tmp)/'state'),XDG_CACHE_HOME=str(Path(tmp)/'cache'))
    env.pop('TMUX',None)
    child=None
    try:
        for _ in range(100):
            if Path(sock).exists():break
            time.sleep(.02)
        child=subprocess.Popen(['nvim','-n','-u','NONE','-i','NONE','-c',"lua local ok,err=pcall(dofile,vim.env.II_ROOT..'/showcase/checks/nvim.lua'); if not ok then vim.fn.writefile({tostring(err)},vim.env.II_FAILURE); vim.cmd('cquit 1') end"],stdin=slave,stdout=slave,stderr=slave,env=env,preexec_fn=session)
        peer=peer_module.Peer(master)
        stop=time.monotonic()+45
        while time.monotonic()<stop:
            if select.select([master],[],[],.02)[0]:
                try: data=os.read(master,1048576)
                except OSError:break
                peer.feed(data)
            if child.poll() is not None:break
        assert child.poll()==0,(Path(env['II_FAILURE']).read_text() if Path(env['II_FAILURE']).exists() else peer.visible.decode('utf8','replace')[-3000:])
        text=peer.visible.decode('utf8','replace')
        assert 'Error executing' not in text and 'stack traceback' not in text,text[-3000:]
        assert peer.graphics>0,'real UI did not publish graphics'
        assert peer.unquiet==0 and peer.broken==0,'graphics corrupted TUI stream'
        print('PASS real Neovim TUI: all showcase extensions, full-width tables, source options restored, 80 notebook scroll steps;',peer.graphics,'graphic packets')
    finally:
        if child and child.poll() is None:child.kill();child.wait()
        os.close(master);os.close(slave)
        daemon.terminate();daemon.wait(timeout=5)
