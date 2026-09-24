#!/usr/bin/env python3
"""Linux SDK regression: a connection burst must not strand keep-alive readers."""
import concurrent.futures
import http.client
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import tempfile
import time


def main():
    cli = Path(sys.argv[1] if len(sys.argv) > 1 else "dist/novelc").resolve()
    with tempfile.TemporaryDirectory(prefix="nir-preview-") as root:
        Path(root, "index.html").write_text("preview connection regression")
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 0))
            port = probe.getsockname()[1]
        server = subprocess.Popen(
            [cli, "serve", root, "--port", str(port)],
            stdout=subprocess.DEVNULL,
        )
        sockets = []
        try:
            for _ in range(100):
                try:
                    with socket.create_connection(("127.0.0.1", port), timeout=0.1):
                        break
                except OSError:
                    if server.poll() is not None:
                        raise RuntimeError("preview server exited during startup")
                    time.sleep(0.05)
            else:
                raise RuntimeError("preview server did not start")

            def read_response(sock):
                response = http.client.HTTPResponse(sock)
                response.begin()
                assert response.status == 200, response.status
                assert response.read() == b"preview connection regression"
                assert not response.will_close, "test requires persistent connections"

            for _ in range(100):
                # Buffer a burst in the listener before any accept worker can run.
                os.kill(server.pid, signal.SIGSTOP)
                os.waitpid(server.pid, os.WUNTRACED)
                try:
                    for _ in range(24):
                        sock = socket.create_connection(("127.0.0.1", port), timeout=3)
                        sockets.append(sock)
                        sock.sendall(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
                finally:
                    os.kill(server.pid, signal.SIGCONT)
                with concurrent.futures.ThreadPoolExecutor(max_workers=24) as pool:
                    list(pool.map(read_response, sockets))
                for sock in sockets:
                    sock.close()
                sockets.clear()
            print("Preview connections: 100 bursts of 24 persistent connections passed")
        finally:
            for sock in sockets:
                sock.close()
            if server.poll() is None:
                os.kill(server.pid, signal.SIGCONT)
                server.terminate()
                server.wait(timeout=5)


if __name__ == "__main__":
    main()
