#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

echo "=== eGPU Manager installieren ==="

# 1. Binaries nach /usr/local/bin/
echo "[1/5] Binaries nach /usr/local/bin/ kopieren..."
sudo cp "$SCRIPT_DIR/target/release/egpu-managerd" /usr/local/bin/egpu-managerd
sudo cp "$SCRIPT_DIR/target/release/egpu-manager-cli" /usr/local/bin/egpu-manager
sudo chmod 755 /usr/local/bin/egpu-managerd /usr/local/bin/egpu-manager

# Widget bleibt im User-Bereich (kein sudo nötig)
cp "$SCRIPT_DIR/target/release/egpu-manager-widget" "$HOME/.local/bin/egpu-manager-widget" 2>/dev/null || true

# Alte Version aus /opt/egpu/ entfernen
if [ -f /opt/egpu/egpu-managerd ]; then
    echo "  Entferne alte Version aus /opt/egpu/..."
    sudo rm -f /opt/egpu/egpu-managerd
fi

# Alte Version aus ~/.local/bin/ entfernen (Daemon + CLI jetzt in /usr/local/bin/)
rm -f "$HOME/.local/bin/egpu-managerd" "$HOME/.local/bin/egpu-manager" 2>/dev/null || true

# 2. Konfiguration
echo "[2/5] Konfiguration nach /etc/egpu-manager/ kopieren..."
sudo mkdir -p /etc/egpu-manager/backups
if [ -f /etc/egpu-manager/config.toml ]; then
    echo "  config.toml existiert bereits — Backup erstellen"
    sudo cp /etc/egpu-manager/config.toml "/etc/egpu-manager/backups/config.toml.bak.$(date +%Y%m%d_%H%M%S)"
    echo "  Neues Template: /etc/egpu-manager/config.toml.new"
    sudo cp "$SCRIPT_DIR/config.toml" /etc/egpu-manager/config.toml.new
else
    sudo cp "$SCRIPT_DIR/config.toml" /etc/egpu-manager/config.toml
fi

# 3. Datenbank-Verzeichnis
echo "[3/5] Datenbank-Verzeichnis erstellen..."
sudo mkdir -p /var/lib/egpu-manager
sudo chown root:root /var/lib/egpu-manager

# 4. systemd-Service
echo "[4/5] systemd-Service installieren..."
sudo cp "$SCRIPT_DIR/egpu-managerd.service" /etc/systemd/system/egpu-managerd.service
sudo systemctl daemon-reload
sudo systemctl enable egpu-managerd.service

# 5. Desktop-Integration
echo "[5/5] Desktop-Integration..."
mkdir -p "$HOME/.config/autostart" "$HOME/.local/share/applications"
cp "$SCRIPT_DIR/crates/egpu-manager-gtk/egpu-manager-widget.desktop" "$HOME/.config/autostart/"
cp "$SCRIPT_DIR/crates/egpu-manager-gtk/egpu-manager-widget.desktop" "$HOME/.local/share/applications/"

echo ""
echo "=== Installation abgeschlossen ==="
echo ""
echo "Daemon:  /usr/local/bin/egpu-managerd"
echo "CLI:     /usr/local/bin/egpu-manager"
echo "Widget:  ~/.local/bin/egpu-manager-widget"
echo ""
echo "Starten:  sudo systemctl start egpu-managerd"
echo "Status:   sudo systemctl status egpu-managerd"
echo "Logs:     journalctl -u egpu-managerd -f"
echo "CLI:      egpu-manager status"
