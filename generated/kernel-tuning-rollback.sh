#!/bin/bash
# kernel-tuning-rollback.sh — Rollback aller eGPU Kernel-Änderungen
# Generiert von egpu-manager
#
# Verwendung: sudo bash kernel-tuning-rollback.sh

set -euo pipefail

if [[ $EUID -ne 0 ]]; then
    echo "FEHLER: Dieses Skript muss als root ausgeführt werden."
    exit 1
fi

echo "=== eGPU Kernel-Tuning Rollback ==="
echo ""

# 1. systemd-Unit entfernen
echo "--- systemd-Unit entfernen ---"
if systemctl is-enabled egpu-pcie-tuning.service &>/dev/null; then
    systemctl disable egpu-pcie-tuning.service
    echo "egpu-pcie-tuning.service deaktiviert"
fi
rm -f /etc/systemd/system/egpu-pcie-tuning.service
systemctl daemon-reload 2>/dev/null || true
echo "Fertig."

# 2. GRUB-Parameter zurücksetzen
echo "--- GRUB: pcie_aspm=off entfernen ---"
GRUB_FILE="/etc/default/grub"
if grep -q "pcie_aspm=off" "$GRUB_FILE" 2>/dev/null; then
    cp "$GRUB_FILE" "${GRUB_FILE}.rollback.$(date +%Y%m%d%H%M%S)"
    sed -i 's/ pcie_aspm=off//g' "$GRUB_FILE"
    echo "pcie_aspm=off entfernt. update-grub MANUELL ausführen:"
    echo "  sudo update-grub"
else
    echo "pcie_aspm=off war nicht gesetzt."
fi

# 3. NVIDIA-Treiberparameter entfernen
echo "--- NVIDIA-Treiberparameter entfernen ---"
rm -f /etc/modprobe.d/nvidia-egpu.conf
echo "Fertig."

# 4. sysctl-Datei entfernen
echo "--- sysctl entfernen ---"
rm -f /etc/sysctl.d/99-egpu-manager.conf
sysctl -w kernel.nmi_watchdog=1 2>/dev/null || true
echo "Fertig."

# 5. AER-Masking zurücksetzen
echo "--- AER-Masking zurücksetzen ---"
setpci -s 0000:05:00.0 ECAP_AER+0x08.L=0x00000000 2>/dev/null || true
echo "Fertig."

echo ""
echo "=== Rollback abgeschlossen ==="
echo "Nächste Schritte:"
echo "  1. sudo update-grub"
echo "  2. Neustart durchführen"
