#!/bin/bash
set -euo pipefail

# Validate pathways and open the visualizer.
# Usage: ./run_and_visualize.sh [INPUT_DIR] [CUSTOM_RULES]

INPUT="${1:-test_data/pathways/pathways_multiple_issues}"
CUSTOM_RULES="${2:-test_data/custom_rules/custom_rules.yml}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PORT=8999

echo "=== Running validator ==="
cargo run -- --input "$INPUT" --custom-rules "$CUSTOM_RULES" > result.json
echo "=== Validator finished, result saved to result.json ==="

echo "=== Starting local web server on port $PORT ==="
echo "=== Opening visualizer at http://localhost:$PORT/visualize.html ==="
echo "=== Press Ctrl+C to stop the server ==="

# Start a simple HTTP server from the script directory and open the browser
cd "$SCRIPT_DIR"
python3 -m http.server "$PORT" &
SERVER_PID=$!

# Give the server a moment to start
sleep 1

# Open the browser
open "http://localhost:$PORT/visualize.html"

# Wait for user to press Ctrl+C
trap "kill $SERVER_PID 2>/dev/null; echo 'Server stopped.'" EXIT
wait $SERVER_PID
