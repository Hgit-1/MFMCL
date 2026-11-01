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


def _write_secret_config(path: Path, text: str) -> None:
    """Write a config containing a secret, never briefly world-readable.

    Opening with `os.open(..., O_CREAT|O_EXCL, 0o600)` applies the mode at creation,
    so there is no window where the file exists with default permissions. Writing
    first and calling `chmod` afterwards (the previous approach) leaves exactly that
    window, and on a shared machine it is long enough to read.

    The mode is also asserted after the fact: on a filesystem that ignores the mode
    bits (some network mounts), the guarantee would silently not hold, and a test
    that believes it wrote a private file is worse than one that fails.
    """
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
        handle.write(text)
    mode = path.stat().st_mode & 0o777
    if mode != 0o600:
        raise AssertionError(f"config permissions are {oct(mode)}, expected 0o600")


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
        # PROTOCOL_TLS_SERVER 配合显式的 minimum_version：静态分析原本提示
        # "不安全协议"是因为它看不到最低版本约束。这里补上 TLS 1.2 下限后，协议
        # 版本本身就不再是不安全的了——不需要抑制注释。
        #
        # 仍然是测试专用的本地 mock server：绑定 127.0.0.1 的临时端口，证书由上面
        # 那行 openssl 现生成、1 天过期，唯一目的是让 aria2 走到 HTTPS 分支。
        # 用 create_default_context 而不是裸 SSLContext：前者已经带上安全的默认值
        # （禁用 SSLv2/3、TLS 1.0/1.1、安全密码套件）。之后再显式钉住 TLS 1.2 下限，
        # 是让「不退化到老协议」这件事写出来，而不是隐含在默认值里。
        # 静态分析曾报 py/insecure-protocol，因为它看不到这层约束；现在协议由
        # create_default_context 提供并显式抬到 TLS 1.2+，不是靠注释消掉告警。
        context = ssl.create_default_context(ssl.Purpose.CLIENT_AUTH)
        context.minimum_version = ssl.TLSVersion.TLSv1_2
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
        # 落盘的必要性：aria2 的 --conf-path 只从文件读配置，把 secret 放 argv 会被
        # `ps` 看到，那才是真正的泄漏。这里写的是本进程随机生成、只在本次测试内存活
        # 的 RPC 令牌（secrets.token_hex(32)），不是用户凭据，也无法用于任何外部服务；
        # 文件权限 0o600，位于 TemporaryDirectory 内，测试结束即删除。
        # aria2 的 --conf-path 只从文件读配置，所以 RPC 令牌必须落盘；放进 argv 会
        # 被 `ps` 看到，那才是真正的泄漏。落盘时的三条约束让它不构成「明文存储
        # 敏感数据」：
        #
        #   1. 令牌由 secrets.token_hex(32) 在本次进程内随机生成，只在这个临时
        #      目录里有效，测试结束即随 TemporaryDirectory 一起删除；
        #   2. 文件以 0o600 创建（见 `_write_secret_config`），不是默认的 0o644；
        #   3. 目录本身是 mkdtemp 建的 0o700。
        #
        # 写入逻辑抽到 `_write_secret_config`，让「权限先于内容」这件事集中在一处，
        # 而不是散在调用点上——先写后 chmod 之间存在一个短暂的世界可读窗口。
        _write_secret_config(
            config,
            f"enable-rpc=true\nrpc-listen-all=false\nrpc-listen-port={port}\n"
            f"rpc-secret={secret}\nrpc-allow-origin-all=false\ndisable-ipv6=true\n"
            "check-certificate=true\ncheck-integrity=true\ncontinue=true\nallow-overwrite=true\n"
            "auto-file-renaming=false\nmax-concurrent-downloads=4\nmax-connection-per-server=4\n"
            "split=4\nmin-split-size=1M\nfile-allocation=none\nconsole-log-level=warn\nlog=\n",
        )
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
