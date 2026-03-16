#!/bin/bash
# install.sh — eGPU Manager Installationsskript
# Generiert von egpu-manager
#
# Dieses Skript installiert KEINE Systempakete. Es:
#   1. Kopiert die kompilierten Binaries nach /usr/local/bin/
#   2. Erstellt Konfigurationsverzeichnisse
#   3. Kopiert die systemd-Unit (aktiviert sie aber NICHT)
#   4. Prüft GNOME AppIndicator Extension
#
# Verwendung: sudo bash install.sh

set -euo pipefail

if [[ $EUID -ne 0 ]]; then
    echo "FEHLER: Dieses Skript muss als root ausgeführt werden."
    exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
BINARY_DIR="$PROJECT_DIR/target/release"

echo "=== eGPU Manager Installation ==="
echo "Projektverzeichnis: $PROJECT_DIR"
echo ""

# 1. Binaries prüfen
if [[ ! -f "$BINARY_DIR/egpu-managerd" ]]; then
    echo "FEHLER: Release-Build nicht gefunden. Bitte zuerst bauen:"
    echo "  cargo build --release"
    exit 1
fi

# 2. Binaries installieren
echo "--- Binaries installieren ---"
install -m 755 "$BINARY_DIR/egpu-managerd" /usr/local/bin/egpu-managerd
install -m 755 "$BINARY_DIR/egpu-manager-cli" /usr/local/bin/egpu-manager
echo "Installiert: /usr/local/bin/egpu-managerd"
echo "Installiert: /usr/local/bin/egpu-manager"

# 3. Verzeichnisse erstellen
echo "--- Verzeichnisse erstellen ---"
mkdir -p /etc/egpu-manager
mkdir -p /var/lib/egpu-manager
mkdir -p /var/log/egpu-manager
mkdir -p /run/egpu-manager
echo "Erstellt: /etc/egpu-manager, /var/lib/egpu-manager, /var/log/egpu-manager, /run/egpu-manager"

# 4. Konfiguration kopieren (nur wenn noch nicht vorhanden)
if [[ ! -f /etc/egpu-manager/config.toml ]]; then
    echo "--- Konfiguration ---"
    if [[ -f "$PROJECT_DIR/config.toml" ]]; then
        install -m 644 "$PROJECT_DIR/config.toml" /etc/egpu-manager/config.toml
        echo "Konfiguration kopiert nach /etc/egpu-manager/config.toml"
    else
        echo "WARNUNG: Keine config.toml gefunden. Bitte manuell erstellen."
    fi
else
    echo "Konfiguration existiert bereits: /etc/egpu-manager/config.toml"
fi

# 5. systemd-Unit installieren
echo "--- systemd-Unit ---"
install -m 644 "$SCRIPT_DIR/egpu-manager.service" /etc/systemd/system/egpu-manager.service
systemctl daemon-reload
echo "Unit installiert. Aktivierung MANUELL:"
echo "  sudo systemctl enable egpu-manager"
echo "  sudo systemctl start egpu-manager"

# 6. GNOME AppIndicator Extension prüfen
echo "--- GNOME AppIndicator Extension ---"
if command -v gnome-extensions &>/dev/null; then
    if gnome-extensions list 2>/dev/null | grep -q "appindicatorsupport"; then
        echo "AppIndicator Extension: installiert und verfügbar"
    else
        echo "WARNUNG: GNOME AppIndicator Extension fehlt."
        echo "  Das GTK4-Widget benötigt diese Extension für das Tray-Icon."
        echo "  Installation: sudo apt install gnome-shell-extension-appindicator"
        echo "  Aktivierung: gnome-extensions enable appindicatorsupport@rgcjonas.gmail.com"
    fi
else
    echo "GNOME nicht erkannt — AppIndicator-Prüfung übersprungen."
fi

echo ""
echo "=== Installation abgeschlossen ==="
echo ""
echo "Nächste Schritte:"
echo "  1. /etc/egpu-manager/config.toml prüfen und anpassen"
echo "  2. sudo systemctl enable egpu-manager"
echo "  3. sudo systemctl start egpu-manager"
echo "  4. http://localhost:7842 im Browser öffnen"
echo ""
echo "Kernel-Tuning (optional, separat):"
echo "  sudo bash $SCRIPT_DIR/kernel-tuning.sh --dry-run"
