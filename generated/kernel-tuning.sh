#!/bin/bash
# kernel-tuning.sh — eGPU PCIe-Absicherung
# Generiert von egpu-manager
#
# WARNUNG: Dieses Skript ändert Kernel- und GRUB-Parameter.
# Bitte VOR der Ausführung:
#   1. Timeshift-Snapshot anlegen
#   2. Skript vollständig lesen und verstehen
#   3. Bei Problemen: kernel-tuning-rollback.sh ausführen
#
# Verwendung: sudo bash kernel-tuning.sh [--dry-run] [--skip-grub] [--skip-nvidia] [--skip-aer-mask]

set -euo pipefail

DRY_RUN=false
SKIP_GRUB=false
SKIP_NVIDIA=false
SKIP_AER_MASK=true  # AER-Masking standardmäßig deaktiviert (riskant)
EGPU_PCI="0000:05:00.0"
ROOT_PORT="0000:00:07.0"
CMPLTO_VALUE="0x6"  # Range C: 65-210ms. Alternativen: 0xA (1-3.5s), 0xE (4-13s)

# --- Argumente parsen ---
for arg in "$@"; do
    case $arg in
        --dry-run) DRY_RUN=true ;;
        --skip-grub) SKIP_GRUB=true ;;
        --skip-nvidia) SKIP_NVIDIA=true ;;
        --enable-aer-mask) SKIP_AER_MASK=false ;;
        --help)
            echo "Verwendung: sudo bash kernel-tuning.sh [--dry-run] [--skip-grub] [--skip-nvidia] [--enable-aer-mask]"
            echo ""
            echo "  --dry-run         Nur anzeigen was getan wird, nichts ändern"
            echo "  --skip-grub       GRUB-Parameter nicht ändern"
            echo "  --skip-nvidia     NVIDIA-Treiberparameter nicht setzen"
            echo "  --enable-aer-mask AER CmpltTO-Masking aktivieren (RISKANT!)"
            exit 0
            ;;
    esac
done

# --- Voraussetzungen ---
if [[ $EUID -ne 0 ]]; then
    echo "FEHLER: Dieses Skript muss als root ausgeführt werden."
    exit 1
fi

echo "=== eGPU Kernel-Tuning ==="
echo "PCIe-Root-Port: $ROOT_PORT"
echo "eGPU-Adresse:   $EGPU_PCI"
echo "CmpltTO-Wert:   $CMPLTO_VALUE"
echo "Dry-Run:        $DRY_RUN"
echo ""

# --- Aktuellen Zustand dokumentieren ---
echo "--- Aktueller Zustand ---"
echo "Kernel: $(uname -r)"
echo "NVIDIA-Treiber: $(nvidia-smi --query-gpu=driver_version --format=csv,noheader | head -1 2>/dev/null || echo 'nicht verfügbar')"
echo "Aktueller CmpltTO: $(setpci -s $ROOT_PORT 0xd4.w 2>/dev/null || echo 'nicht lesbar')"
echo "ASPM-Status: $(cat /sys/module/pcie_aspm/parameters/policy 2>/dev/null || echo 'nicht lesbar')"
echo ""

# --- 1. GRUB-Parameter: pcie_aspm=off ---
if [[ "$SKIP_GRUB" == false ]]; then
    echo "--- GRUB: pcie_aspm=off ---"
    GRUB_FILE="/etc/default/grub"

    if grep -q "pcie_aspm=off" "$GRUB_FILE" 2>/dev/null; then
        echo "pcie_aspm=off ist bereits gesetzt."
    else
        echo "Füge pcie_aspm=off zu GRUB_CMDLINE_LINUX_DEFAULT hinzu..."
        if [[ "$DRY_RUN" == false ]]; then
            cp "$GRUB_FILE" "${GRUB_FILE}.bak.$(date +%Y%m%d%H%M%S)"
            sed -i 's/GRUB_CMDLINE_LINUX_DEFAULT="\(.*\)"/GRUB_CMDLINE_LINUX_DEFAULT="\1 pcie_aspm=off"/' "$GRUB_FILE"
            echo "GRUB aktualisiert. update-grub muss MANUELL ausgeführt werden:"
            echo "  sudo update-grub"
            echo "  sudo reboot"
        else
            echo "[DRY-RUN] Würde pcie_aspm=off hinzufügen"
        fi
    fi
    echo ""
fi

# --- 2. PCIe Completion Timeout (reboot-persistent via systemd) ---
echo "--- PCIe Completion Timeout: $CMPLTO_VALUE ---"
SYSTEMD_UNIT="/etc/systemd/system/egpu-pcie-tuning.service"

if [[ "$DRY_RUN" == false ]]; then
    cat > "$SYSTEMD_UNIT" << UNITEOF
[Unit]
Description=eGPU PCIe Completion Timeout Tuning
After=multi-user.target
Before=egpu-manager.service

[Service]
Type=oneshot
ExecStart=/usr/sbin/setpci -s $ROOT_PORT 0xd4.w=$CMPLTO_VALUE
RemainAfterExit=yes

[Install]
WantedBy=multi-user.target
UNITEOF

    # Sofort setzen
    setpci -s "$ROOT_PORT" "0xd4.w=$CMPLTO_VALUE"
    echo "CmpltTO gesetzt auf $CMPLTO_VALUE"
    echo "Neuer Wert: $(setpci -s $ROOT_PORT 0xd4.w)"

    echo "systemd-Unit erstellt: $SYSTEMD_UNIT"
    echo "Aktivierung muss MANUELL erfolgen:"
    echo "  sudo systemctl daemon-reload"
    echo "  sudo systemctl enable egpu-pcie-tuning.service"
else
    echo "[DRY-RUN] Würde CmpltTO auf $CMPLTO_VALUE setzen und systemd-Unit erstellen"
fi
echo ""

# --- 3. NVIDIA-Treiberparameter ---
if [[ "$SKIP_NVIDIA" == false ]]; then
    echo "--- NVIDIA-Treiberparameter ---"
    MODPROBE_FILE="/etc/modprobe.d/nvidia-egpu.conf"

    DRIVER_VERSION=$(nvidia-smi --query-gpu=driver_version --format=csv,noheader | head -1 2>/dev/null || echo "unbekannt")
    echo "Installierte Treiberversion: $DRIVER_VERSION"
    echo "Getestete Version: 576.02"

    if [[ "$DRIVER_VERSION" != "576.02" ]] && [[ "$DRIVER_VERSION" != "unbekannt" ]]; then
        echo "WARNUNG: Treiberversion weicht von der getesteten Version ab!"
        echo "         Parameter manuell testen bevor sie dauerhaft gesetzt werden."
    fi

    if [[ "$DRY_RUN" == false ]]; then
        echo "options nvidia NVreg_EnablePCIeRelaxedOrderingMode=1" > "$MODPROBE_FILE"
        echo "NVIDIA-Parameter geschrieben: $MODPROBE_FILE"
        echo "Wird erst nach Neustart oder Treiber-Reload wirksam."
    else
        echo "[DRY-RUN] Würde NVreg_EnablePCIeRelaxedOrderingMode=1 setzen"
    fi
    echo ""
fi

# --- 4. sysctl-Parameter ---
echo "--- sysctl: kernel.nmi_watchdog=0 ---"
SYSCTL_FILE="/etc/sysctl.d/99-egpu-manager.conf"

if [[ "$DRY_RUN" == false ]]; then
    cat > "$SYSCTL_FILE" << EOF
# eGPU Manager: Reduziert NMI-Interrupts bei hoher PCIe-Last
kernel.nmi_watchdog=0
EOF
    sysctl -p "$SYSCTL_FILE" 2>/dev/null || true
    echo "sysctl-Parameter gesetzt."
else
    echo "[DRY-RUN] Würde kernel.nmi_watchdog=0 setzen"
fi
echo ""

# --- 5. AER-Masking (optional, standardmäßig deaktiviert) ---
if [[ "$SKIP_AER_MASK" == false ]]; then
    echo "--- AER CmpltTO-Masking (RISKANT!) ---"
    echo "WARNUNG: AER-Masking unterdrückt CmpltTO-Fehlermeldungen an den Treiber."
    echo "         Das verhindert den Freeze, kann aber zu stillen VRAM-Korruptionen führen!"
    echo ""

    if [[ "$DRY_RUN" == false ]]; then
        setpci -s "$EGPU_PCI" ECAP_AER+0x08.L=0x00004000
        echo "AER CmpltTO-Masking aktiviert."
    else
        echo "[DRY-RUN] Würde AER CmpltTO-Bit maskieren"
    fi
    echo ""
else
    echo "--- AER CmpltTO-Masking: ÜBERSPRUNGEN (Standard) ---"
    echo "Aktivierung mit: --enable-aer-mask"
    echo ""
fi

# --- BIOS-Hinweise ---
echo "=== BIOS-Prüfung empfohlen (manuell) ==="
echo "1. Thunderbolt Configuration → PCIe Tunnel: 'x4' statt 'Auto'"
echo "2. Thunderbolt Configuration → Security Level: 'No Security' oder 'User Authorization'"
echo "3. Advanced → PCI Subsystem Settings → Above 4G Decoding: 'Enabled'"
echo "4. Advanced → PCI Subsystem Settings → Re-Size BAR Support: 'Disabled'"
echo ""

echo "=== Kernel-Tuning abgeschlossen ==="
echo "Nächste Schritte:"
echo "  1. sudo update-grub (falls GRUB geändert)"
echo "  2. sudo systemctl daemon-reload"
echo "  3. sudo systemctl enable egpu-pcie-tuning.service"
echo "  4. BIOS-Einstellungen prüfen"
echo "  5. Neustart durchführen"
echo ""
echo "Bei Problemen: sudo bash kernel-tuning-rollback.sh"
