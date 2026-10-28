#!/bin/bash
set -euo pipefail

# Validate pathways and open the Mapbox visualizer.
# Usage: ./run_and_visualize_mapbox.sh [INPUT_DIR] [CUSTOM_RULES]
#
# Set MAPBOX_KEY to your public Mapbox token before running:
#   export MAPBOX_KEY=pk.your_token_here
#   ./run_and_visualize_mapbox.sh
#
# The token is served via the /mapbox-token endpoint and never written into the HTML file,
# so you can safely share visualize_with_mapbox.html without exposing your key.

INPUT="${1:-test_data/pathways/aachen_gtfs}"
CUSTOM_RULES="${2:-test_data/custom_rules/custom_rules.yml}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PORT=8999

if [ -z "${MAPBOX_KEY:-}" ]; then
    echo "⚠️  WARNING: MAPBOX_KEY environment variable is not set." >&2
    echo "   The map will not render without a token." >&2
    echo "   Set it with: export MAPBOX_KEY=your_token_here" >&2
    echo "" >&2
fi

echo "=== Running validator ==="
cargo run --release -- --input "$INPUT" --custom-rules "$CUSTOM_RULES" > result.json
echo "=== Validator finished, result saved to result.json ==="

# Patch serve_mapbox.py's PORT to match ours, then start it
cd "$SCRIPT_DIR"
echo "=== Starting local web server on port $PORT ==="
echo "=== Opening Mapbox visualizer at http://localhost:$PORT ==="
echo "=== Press Ctrl+C to stop the server ==="

# Serve from SCRIPT_DIR, using MAPBOX_KEY from the environment
export MAPBOX_KEY="${MAPBOX_KEY:-}"
python3 -c "
import http.server, json, os, sys

PORT = $PORT
MAPBOX_KEY = os.environ.get('MAPBOX_KEY', '')

class MapboxHandler(http.server.SimpleHTTPRequestHandler):
    def do_GET(self):
        if self.path == '/mapbox-token':
            if not MAPBOX_KEY:
                self.send_response(500)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({'error': 'MAPBOX_KEY environment variable not set'}).encode())
                return
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Access-Control-Allow-Origin', '*')
            self.send_header('Cache-Control', 'no-store, no-cache, must-revalidate, max-age=0')
            self.end_headers()
            self.wfile.write(json.dumps({'token': MAPBOX_KEY}).encode())
            return
        if self.path == '/':
            self.path = '/visualize_with_mapbox.html'
        if self.path.endswith('.json'):
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Cache-Control', 'no-store, no-cache, must-revalidate, max-age=0')
            self.end_headers()
            try:
                with open(self.translate_path(self.path), 'rb') as f:
                    self.wfile.write(f.read())
            except OSError:
                self.send_error(404, 'File not found')
            return
        return super().do_GET()
    def log_message(self, format, *args):
        sys.stderr.write('%s - %s\n' % (self.address_string(), format % args))

server = http.server.HTTPServer(('', PORT), MapboxHandler)
print(f'Serving on http://localhost:{PORT}', file=sys.stderr)
sys.stderr.flush()
server.serve_forever()
" &
SERVER_PID=$!

# Give the server a moment to start
sleep 1

# Open the browser
open "http://localhost:$PORT"

# Wait for user to press Ctrl+C
trap "kill $SERVER_PID 2>/dev/null; echo 'Server stopped.'" EXIT
wait $SERVER_PID