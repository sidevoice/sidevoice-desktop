"""A stand-in for a machine's core, for CI: answers what the bundled interface asks first
(`GET /api/rendezvous` → a node), proves its identity to a page paired with it (`GET /api/device/identity`,
ECDSA P-256 with a throwaway key whose public half test/fixtures/room-flow.js pins), serves the presentation
defaults settings read (`GET /api/presentation/languages`), all with the CORS the real core sends to a desktop
shell; 404s the rest, and prints every request with its Origin so CI can check the interface talked to it."""
import base64
import hashlib
import json
import secrets
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

# P-256, in plain Python: the runner needs no package to sign a nonce. The key is CI's own and signs nothing else.
P = 0xFFFFFFFF00000001000000000000000000000000FFFFFFFFFFFFFFFFFFFFFFFF
N = 0xFFFFFFFF00000000FFFFFFFFFFFFFFFFBCE6FAADA7179E84F3B9CAC2FC632551
G = (0x6B17D1F2E12C4247F8BCE6E563A440F277037D812DEB33A0F4A13945D898C296,
     0x4FE342E2FE1A7F9B8EE7EB4A7C0F9E162BCE33576B315ECECBB6406837BF51F5)
KEY = 0x5C1DC0DE5C1DC0DE5C1DC0DE5C1DC0DE5C1DC0DE5C1DC0DE5C1DC0DE00000001
SPKI_PREFIX = bytes.fromhex('3059301306072a8648ce3d020106082a8648ce3d030107034200')


def add(a, b):
    if a is None:
        return b
    if b is None:
        return a
    if a[0] == b[0] and (a[1] + b[1]) % P == 0:
        return None
    if a == b:
        slope = (3 * a[0] * a[0] - 3) * pow(2 * a[1], -1, P) % P
    else:
        slope = (b[1] - a[1]) * pow(b[0] - a[0], -1, P) % P
    x = (slope * slope - a[0] - b[0]) % P
    return x, (slope * (a[0] - x) - a[1]) % P


def times(k, point):
    result = None
    while k:
        if k & 1:
            result = add(result, point)
        point, k = add(point, point), k >> 1
    return result


PUBLIC = times(KEY, G)
SPKI = SPKI_PREFIX + b'\x04' + PUBLIC[0].to_bytes(32, 'big') + PUBLIC[1].to_bytes(32, 'big')


def sign(message):
    """ECDSA over SHA-256, as WebCrypto verifies it: r ‖ s, 32 bytes each."""
    z = int.from_bytes(hashlib.sha256(message).digest(), 'big')
    while True:
        k = secrets.randbelow(N - 1) + 1
        r = times(k, G)[0] % N
        s = pow(k, -1, N) * (z + r * KEY) % N
        if r and s:
            return r.to_bytes(32, 'big') + s.to_bytes(32, 'big')

ACCEPTED = {'tauri://localhost', 'http://tauri.localhost', 'https://tauri.localhost'}


class Node(BaseHTTPRequestHandler):
    def cors(self):
        origin = self.headers.get('Origin', '')
        if origin in ACCEPTED:
            self.send_header('Access-Control-Allow-Origin', origin)
            self.send_header('Vary', 'Origin')

    def answer(self, status, body):
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(data)))
        self.cors()
        self.end_headers()
        self.wfile.write(data)

    def log_request_line(self):
        print(f'request {self.command} {self.path} origin={self.headers.get("Origin", "-")} '
              f'auth={"yes" if self.headers.get("Authorization") else "no"}', flush=True)

    def do_OPTIONS(self):
        self.log_request_line()
        self.send_response(204)
        self.cors()
        self.send_header('Access-Control-Allow-Methods', 'GET, POST, PUT, PATCH, DELETE')
        self.send_header('Access-Control-Allow-Headers', 'content-type, accept, authorization')
        self.end_headers()

    def do_GET(self):
        self.log_request_line()
        if self.path.split('?')[0] == '/api/presentation/echo':
            # What a paired interface sends a local node: its device token, cross-origin (the real core checks it).
            return self.answer(200, {'authorization': self.headers.get('Authorization', '')})
        if self.path.split('?')[0] == '/api/rendezvous':
            return self.answer(200, {'kind': 'node', 'id': 'ci-node', 'host': 'ci-runner', 'room': None})
        if self.path.split('?')[0] == '/api/device/identity':
            nonce = parse_qs(urlparse(self.path).query).get('nonce', [''])[0]
            signature = sign(('sidevoice-node-identity:' + nonce).encode())
            return self.answer(200, {'public_key': base64.b64encode(SPKI).decode(),
                                     'signature': base64.urlsafe_b64encode(signature).decode().rstrip('=')})
        if self.path.split('?')[0] == '/api/presentation/languages':
            return self.answer(200, {})
        self.answer(404, {'detail': 'not in this stand-in'})

    do_POST = do_PUT = do_DELETE = do_GET

    def log_message(self, *args):
        pass


if __name__ == '__main__':
    if sys.argv[1:2] == ['--key']:  # what a page pairs with: the public key and its fingerprint
        print(base64.b64encode(SPKI).decode(), base64.urlsafe_b64encode(hashlib.sha256(SPKI).digest()).decode().rstrip('='))
        sys.exit(0)
    ThreadingHTTPServer(('127.0.0.1', int(sys.argv[1]) if len(sys.argv) > 1 else 8768), Node).serve_forever()
