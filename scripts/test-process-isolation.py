"""Offline native-Claude regression using the production Rust launch builder.

Usage: python test-process-isolation.py --harness <cargo lib test executable>
       --cli <official claude executable> --output <new, non-sync directory>
Only synthetic credentials, temporary HOME/config and local sinks are used.
No credential/keychain, live config, registry, system trust or live proxy is read.
Requires Python 3 and openssl (Git's openssl is also supported on Windows).
"""
import argparse
import ipaddress
import json
import os
from pathlib import Path
import queue
import shutil
import signal
import socket
import ssl
import subprocess
import sys
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--harness", type=Path, required=True)
parser.add_argument("--cli", type=Path, required=True)
parser.add_argument("--output", type=Path, required=True)
parser.add_argument("--local-target", default="127.0.0.2",
                    help="An IP assigned to this machine, outside the CLI's NO_PROXY list")
options = parser.parse_args()
target_ip = ipaddress.ip_address(options.local_target)
if target_ip.version != 4 or not (target_ip.is_loopback or target_ip.is_private):
    raise SystemExit("Only a local IPv4 loopback/private test target is permitted")
# A successful bind proves this is our machine, not a LAN neighbour. macOS does
# not bind 127.0.0.2 without an interface change; pass its own private LAN IP.
# No hosts entry, interface alias, firewall rule or system setting is changed.
with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as local_check:
    local_check.bind((str(target_ip), 0))
ROOT = options.output.resolve()
if ROOT.exists() or any(p in str(ROOT).lower() for p in ("onedrive", "cloudstorage", "mobile documents")):
    raise SystemExit("Output must be a new non-sync directory")
ROOT.mkdir(parents=True, mode=0o700)
TOKENS = {name: "sk-ant-oat01-CAM_SYNTHETIC_" + name + "_" + uuid.uuid4().hex * 2 for name in ("A", "B")}
EVENTS, CHILDREN, SERVERS = [], [], []
PHASE, PROTOCOL = "initial", "http"
TARGET_BIND = TARGET_HOST = str(target_ip)
REPORT = {"credentials": "invented A/B only; no real store or keychain accessed", "tests": [], "failures": []}


def tls_fixture():
    openssl = shutil.which("openssl")
    if not openssl and os.name == "nt":
        candidate = Path(r"C:\Program Files\Git\usr\bin\openssl.exe")
        openssl = str(candidate) if candidate.is_file() else None
    if not openssl:
        raise SystemExit("openssl is required for a child-only local test certificate")
    cert, key = ROOT / "test-ca.pem", ROOT / "test-key.pem"
    subprocess.run([openssl, "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
                    "-subj", "/CN=CAM synthetic loopback only", "-addext", "basicConstraints=critical,CA:TRUE",
                    "-addext", "subjectAltName=IP:" + TARGET_HOST, "-keyout", str(key), "-out", str(cert)],
                   check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(cert, key)
    context.set_alpn_protocols(["http/1.1"])
    return context, cert


TLS_CONTEXT, CA_PATH = tls_fixture()


def record_event(label, method, path, credential=None):
    EVENTS.append({"protocol": PROTOCOL, "phase": PHASE, "sink": label, "method": method,
                   "path": path, "fakeCredential": credential})


class SinkHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def reply(self, code, body, content_type="application/json"):
        self.send_response(code)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        try:
            self.wfile.write(body)
            self.wfile.flush()
        except (OSError, ssl.SSLError):
            pass
        self.close_connection = True

    def do_CONNECT(self):
        if self.path not in self.server.allowed_targets:
            record_event(self.server.label, "CONNECT_BLOCKED", "non-test-target")
            self.reply(502, b'{"error":"this sink cannot forward traffic"}')
            return
        self.send_response(200, "Connection established")
        self.end_headers()
        self.wfile.flush()
        try:
            # Terminate locally, NEVER connect to an external destination.
            secured = TLS_CONTEXT.wrap_socket(self.connection, server_side=True)
            SinkHandler(secured, self.client_address, self.server)
            secured.close()
        except (OSError, ssl.SSLError):
            record_event(self.server.label, "TLS_ERROR", "test-target")
        self.close_connection = True

    def do_GET(self):
        record_event(self.server.label, "GET", urlsplit(self.path).path)
        self.reply(404, b'{"error":"local test sink"}')

    def do_POST(self):
        self.rfile.read(min(int(self.headers.get("Content-Length", 0)), 4 * 1024 * 1024))
        path = urlsplit(self.path).path
        auth = self.headers.get("Authorization", "")
        label = next((k for k, value in TOKENS.items() if auth == "Bearer " + value), "UNEXPECTED")
        record_event(self.server.label, "POST", path, label)
        if path != "/v1/messages":
            self.reply(200, b'{"input_tokens":1}' if path.endswith("/count_tokens") else b'{}')
            return
        rows = [
            ("message_start", {"type": "message_start", "message": {"id": "msg_mock_" + uuid.uuid4().hex,
                "type": "message", "role": "assistant", "model": "claude-opus-5-5", "content": [],
                "stop_reason": None, "stop_sequence": None, "usage": {"input_tokens": 1, "output_tokens": 0}}}),
            ("content_block_start", {"type": "content_block_start", "index": 0,
                "content_block": {"type": "text", "text": ""}}),
            ("content_block_delta", {"type": "content_block_delta", "index": 0,
                "delta": {"type": "text_delta", "text": "LOCAL MOCK OK"}}),
            ("content_block_stop", {"type": "content_block_stop", "index": 0}),
            ("message_delta", {"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence": None},
                "usage": {"output_tokens": 3}}),
            ("message_stop", {"type": "message_stop"}),
        ]
        body = "".join("event: " + name + "\ndata: " + json.dumps(row) + "\n\n" for name, row in rows).encode()
        self.reply(200, body, "text/event-stream")


class Server(ThreadingHTTPServer):
    daemon_threads = True

    def verify_request(self, request, client_address):
        # When the direct tripwire binds this machine's LAN address on macOS,
        # reject other LAN clients. This server never forwards any traffic.
        return client_address[0] in ("127.0.0.1", "127.0.0.2", TARGET_BIND)

    def handle_error(self, request, client_address):
        record_event(self.label, "HANDLER_ERROR", "local-test")


def server(label, target=False, tls=False):
    srv = Server((TARGET_BIND if target else "127.0.0.1", 0), SinkHandler)
    srv.label, srv.allowed_targets = label, set()
    if tls:
        srv.socket = TLS_CONTEXT.wrap_socket(srv.socket, server_side=True)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    SERVERS.append(srv)
    host, port = srv.server_address
    return srv, ("https" if tls else "http") + "://" + (TARGET_HOST if target else host) + ":" + str(port)


def proxy_env(proxy):
    return {k: proxy for k in ("HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy")}


class Claude:
    def __init__(self, case, proxy, credential, inherited_proxy, file_proxy):
        self.location = ROOT / "runs" / PROTOCOL / case
        self.config = self.location / ".claude"
        self.config.mkdir(parents=True, mode=0o700)
        (self.location / "CAM-SYNTHETIC-ONLY").write_text("fake test only", encoding="utf-8")
        self.settings = self.config / "settings.json"
        self.project_settings = self.location / "project" / ".claude" / "settings.local.json"
        self.project_settings.parent.mkdir(parents=True)
        self.write_settings(file_proxy)
        self.write_project(file_proxy)
        keys = {s.upper() for s in ["PATH", "SystemRoot", "WINDIR", "COMSPEC", "PATHEXT", "TEMP", "TMP", "TMPDIR",
                "PROGRAMFILES", "PROGRAMFILES(X86)", "PROGRAMDATA", "NUMBER_OF_PROCESSORS",
                "PROCESSOR_ARCHITECTURE", "CLAUDE_CODE_GIT_BASH_PATH"]}
        environment = {k: v for k, v in os.environ.items() if k.upper() in keys}
        environment.update({"HOME": str(self.location), "USERPROFILE": str(self.location),
            "APPDATA": str(self.location / "AppData/Roaming"), "LOCALAPPDATA": str(self.location / "AppData/Local"),
            "CLAUDE_CONFIG_DIR": str(self.config), "CLAUDE_CODE_OAUTH_TOKEN": "CAM_SYNTHETIC_STALE_PARENT",
            "ANTHROPIC_BASE_URL": DIRECT_URL, "NODE_EXTRA_CA_CERTS": str(CA_PATH), "NO_PROXY": "*", "no_proxy": "*",
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1", "DISABLE_AUTOUPDATER": "1",
            "DISABLE_TELEMETRY": "1", "DISABLE_ERROR_REPORTING": "1", "API_MAX_RETRIES": "0",
            "API_TIMEOUT_MS": "5000", "CLAUDE_CODE_AUTO_CONNECT_IDE": "0"})
        environment.update(proxy_env(inherited_proxy))
        args = ["--print", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose",
                "--no-session-persistence", "--tools", "", "--strict-mcp-config", "--mcp-config", '{"mcpServers":{}}',
                "--model", "claude-opus-5-5", "--system-prompt", "Local mock routing test. Reply OK only.",
                "--permission-mode", "dontAsk", "--no-chrome"]
        environment["CAM_TEST_LAUNCH_JSON"] = json.dumps({"cli": str(options.cli.resolve()), "testRoot": str(self.location),
            "token": TOKENS[credential], "proxy": proxy, "args": args})
        self.queue, self.stderr_lines, self.native_pid = queue.Queue(), 0, None
        self.proc = subprocess.Popen([str(options.harness.resolve()), "--ignored", "--exact",
            "client::tests::native_cli_fixture", "--nocapture"], cwd=self.location / "project", env=environment,
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, encoding="utf-8",
            errors="replace", bufsize=1, creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
        CHILDREN.append(self)
        threading.Thread(target=self.read_stdout, daemon=True).start()
        threading.Thread(target=self.read_stderr, daemon=True).start()

    def write_settings(self, proxy):
        self.settings.write_text(json.dumps({"autoMemoryEnabled": False, "promptSuggestionEnabled": False,
            "env": {**proxy_env(proxy), "NO_PROXY": "*", "no_proxy": "*"}}), encoding="utf-8")

    def write_project(self, proxy):
        self.project_settings.write_text(json.dumps({"env": {**proxy_env(proxy), "NO_PROXY": "*", "no_proxy": "*"}}), encoding="utf-8")

    def read_stdout(self):
        for line in self.proc.stdout:
            try:
                obj = json.loads(line)
                if isinstance(obj, dict):
                    if obj.get("type") == "cam_test_child":
                        self.native_pid = obj["pid"]
                    self.queue.put(obj)
            except ValueError:
                pass

    def read_stderr(self):
        for _ in self.proc.stderr:
            self.stderr_lines += 1

    def turn(self, phase, route, token, timeout=35, offline=False):
        global PHASE
        PHASE, before, start = phase, len(EVENTS), time.monotonic()
        self.proc.stdin.write(json.dumps({"type": "user", "message": {"role": "user", "content": "Reply OK."}}) + "\n")
        self.proc.stdin.flush()
        result = None
        while time.monotonic() - start < timeout:
            try:
                obj = self.queue.get(timeout=0.25)
            except queue.Empty:
                continue
            if obj.get("type") == "result":
                result = {"isError": bool(obj.get("is_error")), "mockReplied": obj.get("result") == "LOCAL MOCK OK"}
                break
        time.sleep(0.2)
        requests = [e for e in EVENTS[before:] if e["method"] == "POST" and e["path"] == "/v1/messages"]
        passed = (not requests if offline else bool(requests)
            and all(e["sink"] == route and e["fakeCredential"] == token for e in requests)
            and result is not None and result["mockReplied"])
        row = {"protocol": PROTOCOL, "case": phase, "nativePid": self.native_pid, "passed": passed,
               "routes": [e["sink"] for e in requests], "fakeCredentials": [e["fakeCredential"] for e in requests],
               "result": result, "observedSeconds": round(time.monotonic() - start, 2)}
        REPORT["tests"].append(row)
        if not passed:
            REPORT["failures"].append(PROTOCOL + ":" + phase)
        print(json.dumps(row), flush=True)

    def close(self):
        if self.proc.poll() is None:
            try:
                self.proc.stdin.close()
                self.proc.wait(timeout=3)
            except (OSError, subprocess.TimeoutExpired):
                # Only the exact synthetic child PID reported by our own fixture.
                if self.native_pid:
                    try:
                        os.kill(self.native_pid, signal.SIGTERM)
                    except ProcessLookupError:
                        pass
                self.proc.wait(timeout=10)


try:
    for PROTOCOL in ("http", "https"):
        DIRECT_SERVER, DIRECT_URL = server("DIRECT_TRIPWIRE", target=True, tls=PROTOCOL == "https")
        A_SERVER, A_URL = server("A")
        B_SERVER, B_URL = server("B")
        for srv in (A_SERVER, B_SERVER):
            srv.allowed_targets = {TARGET_HOST + ":" + str(DIRECT_SERVER.server_port)}
        a = Claude("A", A_URL, "A", B_URL, B_URL)
        a.turn("A-overrides-stale-parent-and-shared-settings", "A", "A")
        b = Claude("B", B_URL, "B", A_URL, A_URL)
        b.turn("B-new-process-uses-B-pair", "B", "B")
        a.turn("A-after-switch-same-process-still-A-pair", "A", "A")
        a.write_settings(A_URL)
        a.write_project(A_URL)
        time.sleep(1)
        a.write_settings(B_URL)
        a.write_project(B_URL)
        time.sleep(2)
        a.turn("A-after-shared-and-project-settings-reload", "A", "A")
        b.turn("B-continues-independently", "B", "B")
        A_SERVER.shutdown()
        A_SERVER.server_close()
        a.turn("A-proxy-offline-no-B-or-direct-fallback", None, None, timeout=15, offline=True)
        b.turn("B-still-works-while-A-proxy-is-offline", "B", "B")
        a.close()
        b.close()
        for srv in (B_SERVER, DIRECT_SERVER):
            srv.shutdown()
            srv.server_close()
finally:
    for child in CHILDREN:
        child.close()
    for srv in SERVERS:
        srv.shutdown()
        srv.server_close()
    REPORT["unexpectedCredentials"] = sum(e.get("fakeCredential") == "UNEXPECTED" for e in EVENTS)
    REPORT["directRequests"] = sum(e["sink"] == "DIRECT_TRIPWIRE" for e in EVENTS)
    if REPORT["unexpectedCredentials"] or REPORT["directRequests"]:
        REPORT["failures"].append("unexpected credential or direct request")
    REPORT["events"] = EVENTS
    REPORT["limits"] = ["Native CLI print/stream-json; not IDE extensions or background detach/reattach.",
        "Offline observation lasts 15 seconds per protocol; no OS firewall sandbox is claimed.",
        "Mihomo routes/residential exits are not configured or tested here."]
    (ROOT / "report.json").write_text(json.dumps(REPORT, indent=2), encoding="utf-8")
    print(json.dumps({"report": str(ROOT / "report.json"), "failures": REPORT["failures"]}), flush=True)

raise SystemExit(1 if REPORT["failures"] else 0)
