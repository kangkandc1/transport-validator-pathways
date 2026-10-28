#!/usr/bin/env python3
"""
Simple static file server that also serves the Mapbox token from MAPBOX_KEY env var.
Run with: python3 serve_mapbox.py
The server listens on http://localhost:8000

Endpoints:
  /                    -> serve visualize_with_mapbox.html
  /mapbox-token        -> returns {"token": "<MAPBOX_KEY>"}
  /<any_other_file>    -> serve from current directory
"""

import http.server
import json
import os
import sys

PORT = 8000
MAPBOX_KEY = os.environ.get("MAPBOX_KEY", "")


class MapboxHandler(http.server.SimpleHTTPRequestHandler):
    def do_GET(self):
        # Route: /mapbox-token
        if self.path == "/mapbox-token":
            if not MAPBOX_KEY:
                self.send_response(500)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(json.dumps({"error": "MAPBOX_KEY environment variable not set"}).encode())
                return
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Access-Control-Allow-Origin", "*")
            self.send_header("Cache-Control", "no-store, no-cache, must-revalidate, max-age=0")
            self.end_headers()
            self.wfile.write(json.dumps({"token": MAPBOX_KEY}).encode())
            return

        # Route: / (root) -> serve visualize_with_mapbox.html
        if self.path == "/":
            self.path = "/visualize_with_mapbox.html"

        # Add cache-prevention headers for JSON data files so reload always fetches fresh data
        if self.path.endswith(".json"):
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Cache-Control", "no-store, no-cache, must-revalidate, max-age=0")
            self.end_headers()
            try:
                with open(self.translate_path(self.path), "rb") as f:
                    self.wfile.write(f.read())
            except OSError:
                self.send_error(404, "File not found")
            return

        return super().do_GET()

    def log_message(self, format, *args):
        # Quiet logging
        sys.stderr.write("%s - %s\n" % (self.address_string(), format % args))


if __name__ == "__main__":
    if not MAPBOX_KEY:
        print("⚠️  WARNING: MAPBOX_KEY environment variable is not set.", file=sys.stderr)
        print("   The /mapbox-token endpoint will return an error.", file=sys.stderr)
        print("   Set it with: export MAPBOX_KEY=your_token_here\n", file=sys.stderr)
    else:
        print(f"✅ MAPBOX_KEY found (starts with: {MAPBOX_KEY[:8]}...)", file=sys.stderr)

    print(f"🚀 Serving on http://localhost:{PORT}", file=sys.stderr)
    print("   Open http://localhost:8000 in your browser\n", file=sys.stderr)

    server = http.server.HTTPServer(("", PORT), MapboxHandler)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\n👋 Server stopped.", file=sys.stderr)
        server.server_close()
