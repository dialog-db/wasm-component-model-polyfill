"""Serve the smoke test page for a person to open.

`smoke web` runs this script on a loopback port. It is Python's plain
directory server with one change: every response carries
`Cache-Control: no-store`. The page's files come out of the Nix store
with a 1970 modification time, and a browser given that as
`Last-Modified` keeps the script and the wasm in its cache across
runs, so a rebuilt page would otherwise show the previous build until
a hard reload.

Arguments: the page directory and the port.
"""

import http.server
import sys

PAGE_DIR, PORT = sys.argv[1], int(sys.argv[2])


class NoStoreHandler(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=PAGE_DIR, **kwargs)

    def end_headers(self):
        self.send_header("Cache-Control", "no-store")
        super().end_headers()


if __name__ == "__main__":
    server = http.server.ThreadingHTTPServer(("127.0.0.1", PORT), NoStoreHandler)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
