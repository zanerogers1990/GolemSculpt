"""Serves the web version on this computer, for trying it before it goes on a website.

    python serve.py                    (from inside web\\dist, then open http://localhost:8642)
    python serve.py 8651               (another port)
    python serve.py 8651 --one-thread  (without the two headers: the page then
                                        uses the one-thread build, as on a
                                        website that does not send them)

It sends the two headers the many-threads build needs
(Cross-Origin-Opener-Policy and Cross-Origin-Embedder-Policy), as a website
must too for that build (see README.md), so what works here works there.
"""
import http.server
import os
import socketserver
import sys

# Serves the folder this file is in, wherever it is started from.
os.chdir(os.path.dirname(os.path.abspath(__file__)))
args = [a for a in sys.argv[1:] if not a.startswith("--")]
PORT = int(args[0]) if args else 8642
ISOLATED = "--one-thread" not in sys.argv


class Handler(http.server.SimpleHTTPRequestHandler):
    extensions_map = {**http.server.SimpleHTTPRequestHandler.extensions_map, ".wasm": "application/wasm", ".js": "text/javascript"}

    def end_headers(self):
        if ISOLATED:
            self.send_header("Cross-Origin-Opener-Policy", "same-origin")
            self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
        self.send_header("Cache-Control", "no-cache")
        super().end_headers()


class Server(socketserver.ThreadingMixIn, socketserver.TCPServer):
    # Each request on its own thread, so the page's files arrive side by side.
    daemon_threads = True
    allow_reuse_address = True


with Server(("127.0.0.1", PORT), Handler) as httpd:
    print(f"GolemSculpt web on http://localhost:{PORT}" + ("" if ISOLATED else " (one-thread: no isolation headers)"))
    httpd.serve_forever()
