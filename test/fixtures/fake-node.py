"""A stand-in for a machine's core, for CI: answers what the bundled interface asks first
(`GET /api/rendezvous` → a node) with the CORS the real core sends to a desktop shell, 404s the rest, and
prints every request with its Origin so CI can check the interface talked to its target cross-origin."""
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

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
        self.answer(404, {'detail': 'not in this stand-in'})

    do_POST = do_PUT = do_DELETE = do_GET

    def log_message(self, *args):
        pass


if __name__ == '__main__':
    ThreadingHTTPServer(('127.0.0.1', int(sys.argv[1]) if len(sys.argv) > 1 else 8768), Node).serve_forever()
