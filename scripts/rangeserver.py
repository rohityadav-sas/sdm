#!/usr/bin/env python3
"""Local test server: Range support, optional per-connection speed cap.
Mimics sites that throttle each connection (where parallel downloads shine).

    python3 scripts/rangeserver.py --size-mb 200 --per-conn-mbps 2 --port 8000
    then download http://127.0.0.1:8000/file.bin
"""
import argparse, re, socketserver, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ap = argparse.ArgumentParser()
ap.add_argument("--size-mb", type=int, default=100)
ap.add_argument("--per-conn-mbps", type=float, default=0.0, help="MB/s cap per connection (0 = unlimited)")
ap.add_argument("--max-conns", type=int, default=0, help="reject (503) beyond this many simultaneous transfers")
ap.add_argument("--port", type=int, default=8000)
args = ap.parse_args()

TOTAL = args.size_mb * 1024 * 1024
CHUNK = 64 * 1024
BLOCK = bytes((i * 31) % 251 for i in range(CHUNK))
active = 0
lock = threading.Lock()


class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def do_GET(self):
        global active
        a, b, code = 0, TOTAL - 1, 200
        r = self.headers.get("Range")
        if r:
            m = re.match(r"bytes=(\d+)-(\d*)", r)
            a = int(m[1]); b = min(int(m[2]), TOTAL - 1) if m[2] else TOTAL - 1; code = 206
        big = (b - a) > 1000
        if big and args.max_conns:
            with lock:
                if active >= args.max_conns:
                    self.send_response(503); self.send_header("Retry-After", "1")
                    self.send_header("Content-Length", "0"); self.send_header("Connection", "close"); self.end_headers()
                    return
        if big:
            with lock:
                active += 1
        try:
            self.send_response(code)
            if code == 206:
                self.send_header("Content-Range", f"bytes {a}-{b}/{TOTAL}")
            self.send_header("Accept-Ranges", "bytes"); self.send_header("ETag", '"bench"')
            self.send_header("Content-Length", str(b - a + 1)); self.send_header("Connection", "close")
            self.end_headers()
            pos = a
            delay = CHUNK / (args.per_conn_mbps * 1024 * 1024) if args.per_conn_mbps else 0
            while pos <= b:
                n = min(CHUNK, b - pos + 1)
                self.wfile.write(BLOCK[:n]); pos += n
                if delay:
                    time.sleep(delay * n / CHUNK)
        except (BrokenPipeError, ConnectionResetError):
            pass
        finally:
            if big:
                with lock:
                    active -= 1


class S(ThreadingHTTPServer):
    daemon_threads = True
    request_queue_size = 128


print(f"serving {args.size_mb} MB at http://127.0.0.1:{args.port}/file.bin  per-conn cap: {args.per_conn_mbps or 'none'} MB/s", flush=True)
S(("127.0.0.1", args.port), H).serve_forever()
