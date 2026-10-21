#!/usr/bin/env python3
"""Exercise a real aria2 binary: RPC auth, download/checksum, cancellation, TLS rejection.
All transfers are loopback-only. This is an RPC smoke test, not a public network test.
"""
import argparse
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import secrets
import socket
import ssl
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve()
    if subprocess.check_output([str(binary), "--version"], text=True).splitlines()[0] != "aria2 version 1.37.0":
        raise ValueError("Unexpected binary version")
    content = b"MFMCL aria2 official-source RPC fixture\n" * 8192
    checksum = hashlib.sha256(content).hexdigest()

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            self.send_response(200)
            self.send_header("Content-Length", str(len(content)))
            self.end_headers()
            try:
                if self.path == "/slow":
                    for offset in range(0, len(content), 1024):
                        self.wfile.write(content[offset:offset + 1024])
                        self.wfile.flush()
                        time.sleep(0.05)
                else:
                    self.wfile.write(content)
            except (BrokenPipeError, ConnectionResetError, ssl.SSLError):
                pass

        def log_message(self, *_):
            pass

    with tempfile.TemporaryDirectory(prefix="aria2-rpc-") as temp:
        root = Path(temp)
        cert, key = root / "cert.pem", root / "key.pem"
        subprocess.run([
            "openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
            "-keyout", str(key), "-out", str(cert), "-subj", "/CN=localhost",
        ], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        http = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        https = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(cert, key)
        https.socket = context.wrap_socket(https.socket, server_side=True)
        for server in (http, https):
            threading.Thread(target=server.serve_forever, daemon=True).start()
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        secret = secrets.token_hex(32)
        config = root / "session.conf"
        # Match the runtime's security/transfer configuration (secret stays out of argv).
        config.write_text(
            f"enable-rpc=true\nrpc-listen-all=false\nrpc-listen-port={port}\n"
            f"rpc-secret={secret}\nrpc-allow-origin-all=false\ndisable-ipv6=true\n"
            "check-certificate=true\ncheck-integrity=true\ncontinue=true\nallow-overwrite=true\n"
            "auto-file-renaming=false\nmax-concurrent-downloads=4\nmax-connection-per-server=4\n"
            "split=4\nmin-split-size=1M\nfile-allocation=none\nconsole-log-level=warn\nlog=\n",
            encoding="utf-8",
        )
        config.chmod(0o600)
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        child = subprocess.Popen([str(binary), f"--conf-path={config}"], cwd=root,
                                 stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        checks = []

        def rpc(method, params=None, token=secret):
            request = urllib.request.Request(f"http://127.0.0.1:{port}/jsonrpc", data=json.dumps({
                "jsonrpc": "2.0", "id": "smoke", "method": method,
                "params": [f"token:{token}"] + (params or []),
            }).encode(), headers={"Content-Type": "application/json"})
            try:
                with opener.open(request, timeout=3) as response:
                    return json.load(response)
            except urllib.error.HTTPError as error:
                return json.load(error)

        def result(method, params=None):
            value = rpc(method, params)
            if "error" in value:
                # Never echo remote data which might contain the secret.
                raise AssertionError("RPC returned an error for " + method)
            return value["result"]

        def add(path, output, **options):
            opts = {"dir": str(root), "out": output, "max-tries": "1", "timeout": "5",
                    "connect-timeout": "5", "max-http-redirect": "0",
                    "all-proxy": "", "http-proxy": "", "https-proxy": ""}
            opts.update(options)
            return result("aria2.addUri", [[path], opts])

        def wait(gid, expected):
            for _ in range(150):
                status = result("aria2.tellStatus", [gid])
                if status["status"] == expected:
                    return status
                if status["status"] in {"complete", "error", "removed"}:
                    raise AssertionError(f"Expected {expected}; got {status['status']}")
                time.sleep(0.1)
            raise AssertionError("Download state timed out")

        try:
            for _ in range(100):
                if child.poll() is not None:
                    raise AssertionError("aria2 exited before RPC readiness")
                try:
                    version = result("aria2.getVersion")
                    break
                except (OSError, ValueError):
                    time.sleep(0.1)
            else:
                raise AssertionError("RPC readiness timed out")
            assert version["version"] == "1.37.0"
            assert "HTTPS" in version["enabledFeatures"]
            checks.append("version-and-https-feature")
            assert "error" in rpc("aria2.getVersion", token="wrong-token")
            checks.append("unauthorized-rpc-rejected")
            url = f"http://127.0.0.1:{http.server_port}"
            gid = add(url + "/file", "download.bin", checksum="sha-256=" + checksum)
            status = wait(gid, "complete")
            assert status["completedLength"] == str(len(content))
            assert hashlib.sha256((root / "download.bin").read_bytes()).hexdigest() == checksum
            checks.append("addUri-tellStatus-sha256-download")
            gid = add(url + "/file", "bad.bin", checksum="sha-256=" + "0" * 64)
            assert wait(gid, "error")["errorCode"] == "32"
            checks.append("wrong-checksum-rejected")
            gid = add(url + "/slow", "cancel.bin")
            result("aria2.forcePause", [gid])
            wait(gid, "paused")
            result("aria2.unpause", [gid])
            result("aria2.forceRemove", [gid])
            wait(gid, "removed")
            checks.append("pause-unpause-forceRemove")
            gid = add(f"https://127.0.0.1:{https.server_port}/file", "untrusted.bin")
            wait(gid, "error")
            checks.append("untrusted-tls-certificate-rejected")
            result("aria2.shutdown")
            child.wait(timeout=15)
            # aria2's process exit code reflects download results: the intentional
            # checksum/TLS failures above legitimately yield 1 alongside 0.
            assert child.returncode in (0, 1), child.returncode
            checks.append(f"graceful-shutdown-exit-{child.returncode}")
        finally:
            if child.poll() is None:
                child.kill()
                child.wait()
            for server in (http, https):
                server.shutdown()
                server.server_close()
        report = {"binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(), "checks": checks,
                  "result": "passed", "scope": "Real binary, authenticated loopback JSON-RPC; no GUI or installer exercised"}
        if args.report:
            args.report.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
