"""Runs the client on a pty as kitty would host it (KITTY_WINDOW_ID set), ends
it by `quit` (prefix, q) or `term` (SIGTERM), and prints every byte it wrote with
the escape character shown as <ESC>. Usage: pty_probe.py MODE BIN STORE KEYFILE HOME"""
import os, pty, select, signal, sys, time

mode, binp, store, key, home = sys.argv[1:6]
env = {"PATH": os.environ["PATH"], "HOME": home, "DBX_DATA_DIR": store, "DBX_SECRET_KEY_FILE": key, "KITTY_WINDOW_ID": "1", "TERM": "xterm-256color"}
pid, fd = pty.fork()
if pid == 0:
    os.execve(binp, [binp], env)
out = b""


def pump(seconds):
    global out
    end = time.time() + seconds
    while time.time() < end:
        ready, _, _ = select.select([fd], [], [], 0.1)
        if ready:
            try:
                data = os.read(fd, 65536)
            except OSError:
                return
            if not data:
                return
            out += data


pump(3.5)  # the keyboard-protocol probe gets no answer here and times out
if mode == "quit":
    os.write(fd, b"\x07q")
else:
    os.kill(pid, signal.SIGTERM)
pump(3.0)
try:
    os.kill(pid, signal.SIGKILL)
except ProcessLookupError:
    pass
sys.stdout.write(out.decode("utf-8", "replace").replace("\x1b", "<ESC>"))
