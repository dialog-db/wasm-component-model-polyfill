"""Drive the smoke test page headlessly and compare its report.

The flake's `smoke-web` check runs this script inside the build sandbox:
it serves the built page on a loopback port, opens it in the flake's
Chromium through chromedriver (the same WebDriver plumbing the browser
tests use, configured by the same `webdriver.json`), waits for the
page's `#out` element to hold the report's summary line, and fails
unless that line reports no failure and no skip and equals the line
the native smoke binary printed.

The page declares `script-src 'self' 'wasm-unsafe-eval'`, the policy a
hardened site sets, so the whole run — composition included — happens
under it. `boot.js` records on the root element whether the browser is
enforcing that policy, and this script fails if it is not: a run that
passed because the policy was lost would prove nothing.

Arguments: the page directory, the native summary line, the report's
destination path. Environment: `CHROMEDRIVER`, and
`WASM_BINDGEN_TEST_WEBDRIVER_JSON` for the browser capabilities.
"""

import http.server
import json
import os
import socket
import subprocess
import sys
import threading
import time
import urllib.request

PAGE_DIR, NATIVE_SUMMARY, REPORT_PATH = sys.argv[1:4]
BUDGET_SECONDS = 180


def free_port():
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def serve(directory, port):
    handler = lambda *args, **kwargs: http.server.SimpleHTTPRequestHandler(
        *args, directory=directory, **kwargs
    )
    server = http.server.ThreadingHTTPServer(("127.0.0.1", port), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def wait_for_port(port):
    for _ in range(100):
        with socket.socket() as probe:
            if probe.connect_ex(("127.0.0.1", port)) == 0:
                return
        time.sleep(0.1)
    raise SystemExit(f"nothing listens on port {port}")


def request(method, url, body=None):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("Content-Type", "application/json")
    with urllib.request.urlopen(req, timeout=60) as response:
        return json.load(response)["value"]


def main():
    page_port = free_port()
    server = serve(PAGE_DIR, page_port)
    driver_port = free_port()
    driver = subprocess.Popen(
        [os.environ["CHROMEDRIVER"], f"--port={driver_port}"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        wait_for_port(driver_port)
        with open(os.environ["WASM_BINDGEN_TEST_WEBDRIVER_JSON"]) as file:
            capabilities = json.load(file)
        base = f"http://127.0.0.1:{driver_port}"
        session = request(
            "POST", f"{base}/session", {"capabilities": {"alwaysMatch": capabilities}}
        )["sessionId"]
        try:
            request(
                "POST",
                f"{base}/session/{session}/url",
                {"url": f"http://127.0.0.1:{page_port}/"},
            )
            deadline = time.monotonic() + BUDGET_SECONDS
            text = ""
            while time.monotonic() < deadline:
                text = request(
                    "POST",
                    f"{base}/session/{session}/execute/sync",
                    {
                        "script": "return document.getElementById('out').textContent",
                        "args": [],
                    },
                )
                lines = text.rstrip("\n").split("\n")
                if lines and lines[-1].startswith("smoke: "):
                    break
                time.sleep(0.5)
            csp = request(
                "POST",
                f"{base}/session/{session}/execute/sync",
                {
                    "script": "return document.documentElement.dataset.csp || 'absent'",
                    "args": [],
                },
            )
        finally:
            request("DELETE", f"{base}/session/{session}")
    finally:
        driver.terminate()
        server.shutdown()

    with open(REPORT_PATH, "w") as file:
        file.write(text)
    print(text, end="")
    lines = text.rstrip("\n").split("\n")
    summary = lines[-1] if lines else ""
    if not summary.startswith("smoke: "):
        raise SystemExit(f"the page did not finish within {BUDGET_SECONDS}s")
    if csp != "enforced":
        raise SystemExit(
            "the browser did not enforce the page's content-security policy "
            f"(`document.documentElement.dataset.csp` read `{csp}`), so the run "
            "says nothing about instantiating without `'unsafe-eval'`"
        )
    if summary != NATIVE_SUMMARY:
        raise SystemExit(f"the page reported `{summary}`, the native run `{NATIVE_SUMMARY}`")
    if not summary.endswith(" 0 failed, 0 skipped"):
        raise SystemExit(f"the page reported a failure or a skip: `{summary}`")


if __name__ == "__main__":
    main()
