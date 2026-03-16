#!/bin/bash
# ollama-fallback.sh — Wechselt Ollama auf die Ziel-GPU
# Wird vom egpu-ollama-fallback.service aufgerufen
#
# Liest die Ziel-GPU aus /run/egpu-manager/ollama-gpu-target
# und schreibt sie in /etc/default/ollama, dann Restart.

set -euo pipefail

TARGET_FILE="/run/egpu-manager/ollama-gpu-target"
OLLAMA_ENV="/etc/default/ollama"

if [[ ! -f "$TARGET_FILE" ]]; then
    echo "FEHLER: Ziel-Datei nicht gefunden: $TARGET_FILE"
    exit 1
fi

TARGET_GPU=$(cat "$TARGET_FILE")
echo "Ollama GPU-Wechsel: Ziel = $TARGET_GPU"

# /etc/default/ollama aktualisieren
if [[ -f "$OLLAMA_ENV" ]]; then
    # Bestehende CUDA_VISIBLE_DEVICES-Zeile ersetzen oder hinzufügen
    if grep -q "CUDA_VISIBLE_DEVICES" "$OLLAMA_ENV"; then
        sed -i "s|CUDA_VISIBLE_DEVICES=.*|CUDA_VISIBLE_DEVICES=\"$TARGET_GPU\"|" "$OLLAMA_ENV"
    else
        echo "CUDA_VISIBLE_DEVICES=\"$TARGET_GPU\"" >> "$OLLAMA_ENV"
    fi
else
    echo "CUDA_VISIBLE_DEVICES=\"$TARGET_GPU\"" > "$OLLAMA_ENV"
fi

echo "Ollama wird neugestartet..."
systemctl restart ollama.service

echo "Ollama GPU-Wechsel abgeschlossen: $TARGET_GPU"
