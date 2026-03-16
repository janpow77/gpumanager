#!/bin/bash
set -euo pipefail

echo "=== eGPU Manager Daemon installieren ==="

# 1. Binary kopieren
echo "[1/4] Binary nach /opt/egpu/ kopieren..."
sudo cp target/release/egpu-managerd /opt/egpu/egpu-managerd
sudo chmod 755 /opt/egpu/egpu-managerd

# 2. Konfiguration
echo "[2/4] Konfiguration nach /etc/egpu-manager/ kopieren..."
sudo mkdir -p /etc/egpu-manager
if [ -f /etc/egpu-manager/config.toml ]; then
    echo "  config.toml existiert bereits — wird nicht überschrieben"
    echo "  Neues Template: /etc/egpu-manager/config.toml.new"
    sudo cp config.toml /etc/egpu-manager/config.toml.new
else
    sudo cp config.toml /etc/egpu-manager/config.toml
fi

# 3. Datenbank-Verzeichnis
echo "[3/4] Datenbank-Verzeichnis erstellen..."
sudo mkdir -p /tmp/egpu-manager-test

# 4. systemd-Service
echo "[4/4] systemd-Service installieren..."
sudo cp egpu-managerd.service /etc/systemd/system/egpu-managerd.service
sudo systemctl daemon-reload
sudo systemctl enable egpu-managerd.service

echo ""
echo "=== Installation abgeschlossen ==="
echo "Starten mit: sudo systemctl start egpu-managerd"
echo "Status:      sudo systemctl status egpu-managerd"
echo "Logs:        journalctl -u egpu-managerd -f"
