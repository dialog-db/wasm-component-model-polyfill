"""Run the benchmark suite in headless Chrome and keep its report.

The `bench web` menu command runs this script: it serves the built page
on a loopback port, opens it in the flake's Chromium through
chromedriver (the same WebDriver plumbing the browser tests use,
configured by the same `webdriver.json`), waits for the page to publish
its report, writes the JSON where it was asked to, and prints the table.

The measurement deliberately happens here rather than inside a Nix
build: a derivation's output is cached, and a cached benchmark result
is a stale one. Nix builds the page; this script measures it, every
time it is asked.

Arguments: the page directory, the JSON report's destination, then any
`key=value` run controls, which reach the page as its query string.
Environment: `CHROMEDRIVER`, `WASM_BINDGEN_TEST_WEBDRIVER_JSON` for the
browser capabilities, and `WCMP_BENCH_BUDGET_SECONDS` for how long the
whole run may take (900 by default).
"""

import http.server
import json
import os
import socket
import subprocess
import sys
import threading
import time
import urllib.parse
import urllib.request

PAGE_DIR, REPORT_PATH = sys.argv[1:3]
CONTROLS = sys.argv[3:]
BUDGET_SECONDS = float(os.environ.get("WCMP_BENCH_BUDGET_SECONDS", "900"))


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
    with urllib.request.urlopen(req, timeout=120) as response:
        return json.load(response)["value"]


def measure(page_url):
    """Drive the page and return its (table, report JSON) pair."""
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
            request("POST", f"{base}/session/{session}/url", {"url": page_url})
            deadline = time.monotonic() + BUDGET_SECONDS
            while time.monotonic() < deadline:
                report = request(
                    "POST",
                    f"{base}/session/{session}/execute/sync",
                    {"script": "return window.wcmpBenchReport || null", "args": []},
                )
                if report:
                    table = request(
                        "POST",
                        f"{base}/session/{session}/execute/sync",
                        {
                            "script": "return document.getElementById('out').textContent",
                            "args": [],
                        },
                    )
                    return table, report
                time.sleep(0.5)
            raise SystemExit(
                f"the page did not finish within {BUDGET_SECONDS:g}s "
                "(raise WCMP_BENCH_BUDGET_SECONDS, or ask for fewer samples)"
            )
        finally:
            request("DELETE", f"{base}/session/{session}")
    finally:
        driver.terminate()


def main():
    page_port = free_port()
    server = serve(PAGE_DIR, page_port)
    query = "&".join(urllib.parse.quote(control, safe="=") for control in CONTROLS)
    page_url = f"http://127.0.0.1:{page_port}/" + (f"?{query}" if query else "")
    try:
        table, report = measure(page_url)
    finally:
        server.shutdown()

    print(table, end="" if table.endswith("\n") else "\n")

    # The report is kept only when it is one: a page that died
    # says so in JSON of its own, and overwriting the last good
    # report with it would lose the numbers it did not replace.
    try:
        parsed = json.loads(report)
    except json.JSONDecodeError as error:
        raise SystemExit(f"the page's report is not JSON: {error}")
    if not isinstance(parsed, dict) or "benchmarks" not in parsed:
        raise SystemExit(f"the page reported no benchmarks: {report}")

    with open(REPORT_PATH, "w") as file:
        file.write(report if report.endswith("\n") else report + "\n")
    print(f"report written to {REPORT_PATH}")

    failed = [
        benchmark["name"]
        for benchmark in parsed["benchmarks"]
        if benchmark.get("error")
    ]
    if failed:
        raise SystemExit(f"benchmarks failed: {', '.join(failed)}")


if __name__ == "__main__":
    main()
