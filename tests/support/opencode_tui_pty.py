import errno
import fcntl
import os
import pty
import select
import signal
import struct
import sys
import termios
import time

pid, terminal = pty.fork()
if pid == 0:
    os.execvpe(sys.argv[1], sys.argv[1:], os.environ)

fcntl.ioctl(terminal, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 120, 0, 0))
stopping = False


def Stop(signum, frame):
    global stopping
    stopping = True
    try:
        os.killpg(pid, signal.SIGTERM)
    except ProcessLookupError:
        pass


signal.signal(signal.SIGTERM, Stop)
signal.signal(signal.SIGINT, Stop)
try:
    while not stopping:
        if not select.select([terminal], [], [], 0.2)[0]:
            continue
        try:
            data = os.read(terminal, 65536)
        except OSError as error:
            if error.errno == errno.EIO:
                break
            raise
        if not data:
            break
        sys.stdout.buffer.write(data)
        sys.stdout.buffer.flush()
finally:
    Stop(None, None)
    deadline = time.monotonic() + 2
    while True:
        done, status = os.waitpid(pid, os.WNOHANG)
        if done:
            break
        if time.monotonic() >= deadline:
            os.killpg(pid, signal.SIGKILL)
            os.waitpid(pid, 0)
            break
        time.sleep(0.05)
    os.close(terminal)
