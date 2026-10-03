"""Chromium sans fenêtre, piloté par le protocole DevTools (CDP) — bibliothèque standard seule.

Le client WebSocket minimal qu'il faut pour Runtime.evaluate : la page de
l'app tourne pour de vrai, les tests appellent ses fonctions et lisent leurs
résultats. Chrome(url) démarre un navigateur au profil jetable ; eval(expr)
attend les promesses et rend la valeur (JSON).
"""

import base64
import json
import os
import shutil
import socket
import struct
import subprocess
import tempfile
import time
import urllib.request


def find_chromium():
    for name in ("chromium", "chromium-browser", "google-chrome-stable", "google-chrome", "brave"):
        p = shutil.which(name)
        if p:
            return p
    return None


class JsError(Exception):
    pass


class _Socket:
    """Une connexion WebSocket (RFC 6455), côté client : trames texte masquées."""

    def __init__(self, url):
        rest = url.split("://", 1)[1]
        hostport, path = rest.split("/", 1)
        host, port = hostport.split(":")
        self.s = socket.create_connection((host, int(port)), timeout=600)
        key = base64.b64encode(os.urandom(16)).decode()
        self.s.sendall((f"GET /{path} HTTP/1.1\r\nHost: {hostport}\r\nUpgrade: websocket\r\n"
                        f"Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n").encode())
        head = b""
        while b"\r\n\r\n" not in head:
            chunk = self.s.recv(4096)
            if not chunk:
                raise ConnectionError("WebSocket refusé")
            head += chunk
        if b" 101 " not in head.split(b"\r\n", 1)[0]:
            raise ConnectionError(head.split(b"\r\n", 1)[0].decode())
        self.buf = head.split(b"\r\n\r\n", 1)[1]

    def _read(self, n):
        while len(self.buf) < n:
            chunk = self.s.recv(1 << 20)
            if not chunk:
                raise ConnectionError("WebSocket fermé")
            self.buf += chunk
        out, self.buf = self.buf[:n], self.buf[n:]
        return out

    def send(self, text, opcode=1):
        data = text.encode() if isinstance(text, str) else text
        n = len(data)
        head = bytes([0x80 | opcode])
        if n < 126:
            head += bytes([0x80 | n])
        elif n < 1 << 16:
            head += bytes([0x80 | 126]) + struct.pack(">H", n)
        else:
            head += bytes([0x80 | 127]) + struct.pack(">Q", n)
        mask = os.urandom(4)
        body = bytes(b ^ mask[i & 3] for i, b in enumerate(data)) if n < 4096 else _mask(data, mask)
        self.s.sendall(head + mask + body)

    def recv(self):
        parts = []
        while True:
            b0, b1 = self._read(2)
            n = b1 & 0x7F
            if n == 126:
                n = struct.unpack(">H", self._read(2))[0]
            elif n == 127:
                n = struct.unpack(">Q", self._read(8))[0]
            payload = self._read(n)
            op = b0 & 0x0F
            if op == 9:                      # ping
                self.send(payload, 10)
                continue
            if op == 8:
                raise ConnectionError("WebSocket fermé par le navigateur")
            parts.append(payload)
            if b0 & 0x80:
                return b"".join(parts).decode()

    def close(self):
        try:
            self.s.close()
        except OSError:
            pass


def _mask(data, mask):
    m = int.from_bytes(mask * ((len(data) + 3) // 4), "big") >> (8 * (-len(data) % 4))
    return (int.from_bytes(data, "big") ^ m).to_bytes(len(data), "big")


class Chrome:
    def __init__(self, url, binary=None, args=()):
        self.profile = tempfile.mkdtemp(prefix="csvfab-cdp-")
        self.proc = subprocess.Popen(
            [binary or find_chromium(), "--headless=new", "--remote-debugging-port=0", f"--user-data-dir={self.profile}",
             "--no-first-run", "--no-default-browser-check", "--disable-extensions", "--password-store=basic", *args, url],
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        port_file = os.path.join(self.profile, "DevToolsActivePort")
        deadline = time.time() + 30
        while not os.path.exists(port_file) or os.path.getsize(port_file) == 0:
            if time.time() > deadline:
                raise RuntimeError("Chromium n'a pas ouvert son port DevTools")
            time.sleep(0.05)
        with open(port_file) as f:
            port = f.read().split()[0]
        page = None
        while page is None and time.time() < deadline:
            with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(f"http://127.0.0.1:{port}/json/list") as r:
                page = next((t for t in json.load(r) if t["type"] == "page"), None)
            time.sleep(0.05)
        self.ws = _Socket(page["webSocketDebuggerUrl"])
        self.seq = 0

    def call(self, method, **params):
        self.seq += 1
        mid = self.seq
        self.ws.send(json.dumps({"id": mid, "method": method, "params": params}))
        while True:
            msg = json.loads(self.ws.recv())
            if msg.get("id") == mid:
                if "error" in msg:
                    raise JsError(msg["error"])
                return msg["result"]

    def eval(self, expr, timeout=60):
        """La valeur de expr (promesses attendues) ; JsError si elle lève ou dépasse timeout secondes."""
        # eval global (indirect) : expressions et suites d'instructions, portée de la page.
        expr = (f"Promise.race([Promise.resolve((0, eval)({json.dumps(expr)})), new Promise((_, no) => "
                f"setTimeout(() => no(new Error('délai dépassé ({timeout} s)')), {int(timeout * 1000)}))])")
        r = self.call("Runtime.evaluate", expression=expr, awaitPromise=True, returnByValue=True)
        if "exceptionDetails" in r:
            d = r["exceptionDetails"]
            raise JsError((d.get("exception") or {}).get("description") or d.get("text"))
        return r["result"].get("value")

    def wait_for(self, expr, timeout=30):
        deadline = time.time() + timeout
        while time.time() < deadline:
            try:
                if self.eval(expr):
                    return
            except JsError:
                pass
            time.sleep(0.05)
        raise TimeoutError(expr)

    def close(self):
        self.ws.close()
        self.proc.terminate()
        try:
            self.proc.wait(10)
        except subprocess.TimeoutExpired:
            self.proc.kill()
        shutil.rmtree(self.profile, ignore_errors=True)
