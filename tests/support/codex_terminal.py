import asyncio, fcntl, json, os, pathlib, pty, re, select, signal, struct, termios, time

class Terminal:
    def __init__(self, args, env, cwd):
        pid, fd = pty.fork()
        if pid == 0:
            os.chdir(cwd)
            os.execve(args[0], args, env)
        self.pid, self.fd, self.raw = pid, fd, bytearray()
        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack('HHHH',45,140,0,0))
        os.set_blocking(fd, False)
    def read(self):
        while select.select([self.fd], [], [], 0)[0]:
            try: chunk=os.read(self.fd, 65536)
            except (BlockingIOError, OSError): break
            if not chunk: break
            self.raw.extend(chunk)
            if b'\x1b[6n' in chunk: os.write(self.fd,b'\x1b[1;1R')
            if b'\x1b]11;?' in chunk: os.write(self.fd,b'\x1b]11;rgb:0000/0000/0000\x1b\\')
    async def ready(self):
        deadline=time.monotonic()+30
        while time.monotonic()<deadline:
            self.read()
            text=self.text()
            if 'mock-model' in text and 'OpenAI Codex' in text: return
            await asyncio.sleep(.1)
        raise AssertionError('TUI not ready: '+self.text()[-2500:])
    def text(self):
        return re.sub(r'\x1b(?:\[[0-?]*[ -/]*[@-~]|\][^\x07]*(?:\x07|\x1b\\))','',self.raw.decode(errors='replace'))
    def close(self):
        os.close(self.fd)
        try: os.kill(self.pid,signal.SIGTERM)
        except (ProcessLookupError, PermissionError): pass
        deadline=time.monotonic()+3
        while time.monotonic()<deadline:
            done,_=os.waitpid(self.pid,os.WNOHANG)
            if done: break
            time.sleep(.05)
        else:
            try: os.kill(self.pid,signal.SIGKILL)
            except (ProcessLookupError, PermissionError): pass
            os.waitpid(self.pid,os.WNOHANG)
