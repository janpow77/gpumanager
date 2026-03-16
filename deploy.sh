#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")"

echo "=== eGPU Manager Deploy ==="

echo "[1/5] Stoppe Daemon..."
sudo systemctl stop egpu-managerd

echo "[2/5] Installiere Binary..."
sudo cp target/release/egpu-managerd /usr/local/bin/egpu-managerd
sudo chmod 755 /usr/local/bin/egpu-managerd

echo "[3/5] Aktualisiere Config..."
sudo cp config.toml /etc/egpu-manager/config.toml

echo "[4/5] Aktualisiere Service..."
sudo cp egpu-managerd.service /etc/systemd/system/egpu-managerd.service
sudo systemctl daemon-reload

echo "[5/5] Starte Daemon..."
sudo systemctl start egpu-managerd
sleep 3

echo ""
echo "=== Verifizierung ==="
echo ""

echo "--- systemd Status ---"
systemctl is-active egpu-managerd

echo ""
echo "--- LLM Health ---"
curl -s http://127.0.0.1:7842/api/llm/health | python3 -m json.tool

echo ""
echo "--- Discovery (Auszug) ---"
curl -s http://127.0.0.1:7842/api/v1/discover | python3 -c "
import sys,json
d=json.load(sys.stdin)
print(f'Service: {d[\"service\"]} v{d[\"version\"]}')
print(f'LLM Gateway aktiv: {d[\"llm_gateway\"][\"active\"]}')
print(f'Host URL: {d[\"base_url\"][\"host\"]}')
print(f'Docker URL: {d[\"base_url\"][\"docker\"]}')
for g in d['gpus']:
    print(f'GPU: {g[\"name\"]} [{g[\"type\"]}] UUID={g[\"gpu_uuid\"]}')
"

echo ""
echo "--- GPU Acquire Test ---"
RESULT=$(curl -s -X POST http://127.0.0.1:7842/api/gpu/acquire \
  -H 'Content-Type: application/json' \
  -d '{"pipeline":"test","workload_type":"verify","vram_mb":100}')
echo "$RESULT" | python3 -c "
import sys,json
d=json.load(sys.stdin)
print(f'Granted: {d.get(\"granted\")}')
print(f'GPU UUID: {d.get(\"gpu_uuid\",\"?\")}')
print(f'Device: {d.get(\"gpu_device\",\"?\")}')
lease=d.get('lease_id','')
print(f'Lease: {lease}')
"
# Release the test lease
LEASE_ID=$(echo "$RESULT" | python3 -c "import sys,json; print(json.load(sys.stdin).get('lease_id',''))")
if [ -n "$LEASE_ID" ]; then
  curl -s -X POST http://127.0.0.1:7842/api/gpu/release \
    -H 'Content-Type: application/json' \
    -d "{\"lease_id\":\"$LEASE_ID\"}" > /dev/null
  echo "Lease freigegeben."
fi

echo ""
echo "--- Config Diff (sollte leer sein) ---"
diff /etc/egpu-manager/config.toml config.toml && echo "Config synchron." || echo "ACHTUNG: Unterschiede!"

echo ""
echo "=== Deploy abgeschlossen ==="
