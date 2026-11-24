#!/usr/bin/env python3
"""Exercise actual Yazi and ii under arrow/scroll input in a controlling PTY.
The minimal terminal peer answers capability queries and parses graphic packets;
it does not certify pixel appearance in a specific emulator.
"""
import argparse
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import struct
import subprocess
import tempfile
import termios
import time
import unicodedata

ROOT = Path(__file__).resolve().parents[2]


class Peer:
    def __init__(self, fd):
        self.fd, self.pending, self.visible = fd, bytearray(), bytearray()
        self.graphics = 0
        self.unquiet = 0
        self.broken = 0
        self.fragments = []

    def feed(self, data):
        self.pending.extend(data)
        while self.pending:
            if self.pending[0] != 27:
                self.visible.append(self.pending.pop(0))
                continue
            if len(self.pending) < 2: return
            prefix = bytes(self.pending[:3])
            if self.pending[1] in (ord('_'), ord('P'), ord(']')):
                end = self.pending.find(b'\x1b\\', 2)
                bell = self.pending.find(b'\x07', 2) if self.pending[1] == ord(']') else -1
                if bell >= 0 and (end < 0 or bell < end): end, length = bell, 1
                elif end >= 0: length = 2
                else: return
                packet = bytes(self.pending[:end+length])
                del self.pending[:end+length]
                if prefix == b'\x1b_G':
                    params = packet[3:-2].split(b';',1)[0]
                    fields = dict(s.split(b'=',1) for s in params.split(b',') if b'=' in s)
                    if fields.get(b'a') == b'q':
                        os.write(self.fd, b'\x1b_Gi='+fields.get(b'i',b'0')+b';OK\x1b\\')
                    else:
                        self.graphics += 1
                        self.unquiet += fields.get(b'q') != b'2'
                        self.broken += b'\x1b' in packet[3:-2]
                        if b'\x1b' in packet[3:-2]: self.fragments.append(repr(packet[:300]))
                elif packet.startswith(b'\x1b]11;?'):
                    os.write(self.fd,b'\x1b]11;rgb:1e1e/1e1e/2e2e\x1b\\')
                continue
            if self.pending[1] == ord('['):
                if len(self.pending) < 3: return
                end = next((i for i,b in enumerate(self.pending[2:],2) if 64 <= b <= 126), None)
                if end is None: return
                packet = bytes(self.pending[:end+1]);del self.pending[:end+1]
                if packet == b'\x1b[6n': os.write(self.fd,b'\x1b[1;1R')
                elif packet in (b'\x1b[c',b'\x1b[0c'): os.write(self.fd,b'\x1b[?62;4;6;22c')
                continue
            del self.pending[:2]


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=ROOT/'target/debug/ii')
    parser.add_argument('--report', type=Path)
    parser.add_argument('--notebook', action='store_true', help='scroll the notebook showcase in both directions')
    args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='ii-yazi-check-') as tmp:
        cfg=Path(tmp)/'config';(cfg/'plugins').mkdir(parents=True)
        plugin_src = (ROOT/'plugins/ii.yazi') if (ROOT/'plugins/ii.yazi').exists() else (Path.home()/'x/dev/own/ii-plugins/ii.yazi')
        (cfg/'plugins/ii.yazi').symlink_to(plugin_src)
        (cfg/'yazi.toml').write_text('[plugin]\nprepend_previewers=[{url="*.{md,mdx,qmd,rmd,svg,dot,gv,ipynb}",run="ii"}]\n')
        step = 10 if args.notebook else 5
        (cfg/'keymap.toml').write_text(f'[mgr]\nprepend_keymap=[{{on="J",run="seek {step}"}},{{on="K",run="seek -{step}"}}]\n')
        # A private executable path avoids changing any installed binary or configuration.
        bindir=Path(tmp)/'bin';bindir.mkdir();(bindir/'ii').symlink_to(args.binary.resolve())
        master,slave=pty.openpty()
        fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',40,140,1260,720))
        env=dict(os.environ,TERM='xterm-kitty',COLORTERM='truecolor',KITTY_WINDOW_ID='1',YAZI_CONFIG_HOME=str(cfg),XDG_STATE_HOME=tmp+'/state',XDG_CACHE_HOME=tmp+'/cache',XDG_RUNTIME_DIR=tmp,PATH=str(bindir)+':'+os.environ['PATH'])
        env.pop('TMUX',None)
        def session():
            os.setsid();fcntl.ioctl(slave,termios.TIOCSCTTY,0)
        fixture = '03.ipynb' if args.notebook else '07.svg'
        child=subprocess.Popen(['yazi',str(ROOT/'showcase'/fixture)],stdin=slave,stdout=slave,stderr=slave,env=env,preexec_fn=session)
        peer=Peer(master)
        def pump(seconds):
            deadline=time.monotonic()+seconds
            while time.monotonic()<deadline:
                if select.select([master],[],[],.01)[0]:
                    try: data=os.read(master,1048576)
                    except OSError: return
                    if not data: return
                    peer.feed(data)
        try:
            pump(2)
            if args.notebook:
                assert b'Data Science Showcase' in peer.visible, peer.visible[-2500:].decode('utf-8', 'replace')
                initial_packets = peer.graphics
                assert initial_packets > 0, 'initial notebook frame did not upload its formulas'
                reused = 0
                notebook_keys = [b'J'] * 25 + [b'K'] * 25
                for key in notebook_keys:
                    before, packets = len(peer.visible), peer.graphics
                    os.write(master,key);pump(.6)
                    if len(peer.visible)>before and peer.graphics == packets:
                        reused += 1
                for heading in (b'Distribution Plots', b'Transformer Architecture', b'Attention Pattern'):
                    assert heading in peer.visible, f'notebook did not reach {heading.decode()}'
                assert reused > 0, 'every scroll retransmitted graphics'
            else:
                for _ in range(20):
                    os.write(master,b'J');pump(.025)
                    os.write(master,b'\x1b[B');pump(.025)
                    os.write(master,b'\x1b[A');pump(.025)
                    os.write(master,b'K');pump(.025)
            pump(1)
            raw=termios.tcgetattr(slave)
            assert raw[3] & (termios.ECHO|termios.ICANON) == 0, 'preview changed TUI input mode'
            os.write(master,b'q');pump(2)
            child.wait(timeout=3)
            assert child.returncode == 0, child.returncode
            text=peer.visible.decode('utf8','replace')
            assert peer.graphics > 0, 'graphics path was not exercised'
            assert peer.unquiet == 0, f'{peer.unquiet} graphic commands could contaminate input'
            assert peer.broken == 0, str(peer.fragments[:3])
            for cell in re.finditer('\U0010eeee(..)', text):
                assert all(unicodedata.combining(ch) == 230 for ch in cell[1]), 'graphics cell lost its row or column'
            assert not re.search(r'[A-Za-z0-9+/]{100,}',text), 'Base64 escaped into visible text'
            assert 'ii:' not in text and 'stack traceback' not in text, text[-1000:]
            assert not re.search(r'A{4,}|D{4,}|B{4,}',text), 'arrow key bytes became visible'
            result=dict(graphic_packets=peer.graphics,unquiet_commands=peer.unquiet,interleaved_packets=peer.broken,exit=child.returncode,arrow_keys=0 if args.notebook else 40,scroll_keys=len(notebook_keys) if args.notebook else 40)
            if args.notebook: result.update(initial_packets=initial_packets, redraws_without_upload=reused)
            print(json.dumps(result,indent=2))
            if args.report: args.report.write_text(json.dumps(result,indent=2)+'\n')
        finally:
            if child.poll() is None:child.kill();child.wait(timeout=3)
            os.close(master);os.close(slave)
            subprocess.run([str(args.binary.resolve()),'--socket',tmp+'/ii/ii.sock','--shutdown'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,env=env)


if __name__=='__main__':main()
