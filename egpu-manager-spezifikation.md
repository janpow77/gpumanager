# egpu-manager — Zentrale Spezifikation

**Version:** 2.4 — Stand 14. März 2026
**System:** ASUS NUC15JNLU7X4, Ubuntu 24.04.3 LTS, Kernel 6.8.0-101-generic
**Autor:** Jan

**Änderungshistorie:**
- **v1.1:** Abschnitt 5a (Pipeline-Analyse), 5c (Prioritätssystem), 6a (Pipeline-Widget).
- **v1.2:** Abschnitt 5b (Projekt-Wizard, Erkennungs-Bibliothek).
- **v1.3:** Abschnitt 5d (Remote-GPU, Failover, Agent-Modus).
- **v1.4:** Abschnitt 5e (Windows-Setup-Generator).
- **v2.0:** Abschnitte 4.5–4.9 (Lifecycle, nvidia-smi-Timeout, Docker-Fehler, AER-Edge-Cases, Recovery-Scheduling). 5f (audit_designer-Integration). 7a (Wayland/GNOME). 8a (REST-API). 8b (Retention). 8c (Integrity-Checks). 13 (Test-Strategie).
- **v2.1:** Komplette Überarbeitung aller v2.0-Lücken:
  - **Kritisch:** Netzwerk-Bindung und Auth-Modell konsolidiert (Abschnitt 3a). Remote-Endpunkte laufen auf separatem Bind mit Token-Auth.
  - **Kritisch:** `[[docker.containers]]` vollständig entfernt. Einziges Schema ist `[[pipeline]]`.
  - **Hoch:** Container-Migration nutzt `docker compose` Recreate statt `docker restart` mit Env-Var (Abschnitt 4.4).
  - **Hoch:** GPU-Identifikation über PCI-Bus-ID statt nvidia-smi-Index (Abschnitt 4.2a).
  - **Hoch:** Verbindliches Auth-Modell für Remote (Pre-Shared Token + optionales mTLS, Abschnitt 3a).
  - **Mittel:** Transaktionale Konfigurationsupdates mit Backup/Rollback (Abschnitt 9a).
  - **Mittel:** Kernel-Tuning reboot-persistent via systemd-Einheit (Abschnitt 8).
  - **Mittel:** Stateful-Dienste-Hooks (DB, Redis, Celery) vor Recovery (Abschnitt 4.4).
  - **Mittel:** Abnahmekriterien pro Phase (Abschnitt 11).
  - **UI:** Zustandsmodell, Accessibility, Skalierung, Audit-Log, Guardrails (Abschnitt 6).
  - **Terminologie:** "RTX 5060 Ti" durchgängig für interne GPU. Bandbreite einheitlich in GT/s.
- **v2.2:** Freeze-Prävention und optimale Workload-Verteilung:
  - **Kritisch:** Abschnitt 4.2b (PCIe-Link-Health-Monitoring) — Erkennung von Link-Degradation bevor AER-Fehler auftreten.
  - **Kritisch:** Abschnitt 4.3 erweitert — Warnstufe Gelb löst jetzt proaktive Workload-Drosselung aus (nicht nur schnelleres Polling).
  - **Kritisch:** Abschnitt 4.10 (CUDA-Watchdog) — 500ms-Heartbeat erkennt Treiber-Freeze vor nvidia-smi.
  - **Kritisch:** Abschnitt 8 erweitert — NVIDIA-Treiberparameter, AER-Masking-Option, vollständige CmpltTO-Range, BIOS-Hinweise.
  - **Hoch:** Abschnitt 5.3 überarbeitet — PCIe-Bandbreite wird jetzt über `nvidia-smi dmon` gemessen (pcie_tx/pcie_rx in KB/s).
  - **Hoch:** Abschnitt 5.5 (Ollama als Host-Service-Workload) — VRAM-Monitoring und Steuerung über Ollama-API.
  - **Hoch:** Abschnitt 5c.2 erweitert — Scheduling nutzt tatsächlichen VRAM-Verbrauch statt nur Schätzungen, GPU-Compute-Auslastung als Schwellenwert.
  - **Mittel:** Abschnitt 5.2 — Display-VRAM-Reservierung auf RTX 5060 Ti berücksichtigt.
  - **Mittel:** Abschnitt 5d.2 — `max_latency_ms` pro Workload-Typ für Remote-GPU-Routing.
  - **Mittel:** Abschnitt 5.6 (Celery Task-Type Reporting) — Webhook für dynamischen VRAM-Bedarf pro Task.
  - **Kritisch:** Abschnitt 14 (Risikoanalyse — Claude Code Vorfall vom 13. März 2026) — Verbindliche Lektüre, 7 Regeln für Claude Code.
- **v2.3:** GUI-Überarbeitung für Erklärbarkeit und sichere Bedienung:
  - **Kritisch:** Web-UI trennt Verbindungszustand und Betriebszustand; persistente Betriebsleiste zeigt Scheduler-, Queue- und Recovery-Status.
  - **Hoch:** Pipeline-Karten zeigen Entscheidungsgründe, Queue-Position und Blocker statt nur Rohzustand.
  - **Hoch:** Schreibende Aktionen nutzen verpflichtende Dry-Run-/Impact-Vorschau vor Bestätigung.
  - **Mittel:** Recovery-State-Machine wird sichtbar gemacht; Audit-Log bleibt unveränderlich, ist aber filter- und durchsuchbar.
  - **Mittel:** Eingebettetes `audit_designer`-Dashboard ist standardmäßig read-only; Vollsteuerung bleibt in der Hauptoberfläche.
- **v2.4:** Benutzerinitiierte eGPU-Deaktivierung:
  - **Kritisch:** Neue Daemon-Zustände `draining` und `disabled` für die eGPU.
  - **Hoch:** Web-UI erhält Schalter "eGPU nach aktuellem Task deaktivieren" mit klarer Queue-/Drain-Anzeige.
  - **Hoch:** Nach Deaktivierungswunsch werden keine neuen Tasks mehr auf die eGPU geplant; laufende Tasks dürfen sauber fertig laufen.
  - **Mittel:** Nach Leerstand wird die eGPU per Thunderbolt-Deauthorization logisch vom System getrennt; Reaktivierung erfolgt explizit über "eGPU aktivieren".

---

## 1. Ausgangslage und Problemstellung

Das System betreibt zwei NVIDIA-GPUs parallel:

| GPU | Modell | VRAM | PCIe-Adresse | Anbindung |
|---|---|---|---|---|
| **Intern** | NVIDIA GeForce RTX 5060 Ti | 8 GB (8.151 MB) | `0000:02:00.0` | PCIe x8 direkt |
| **Extern (eGPU)** | NVIDIA GeForce RTX 5070 Ti | 16 GB (16.303 MB) | `0000:05:00.0` | Razer Core X V2, USB4/Thunderbolt |

Die interne RTX 5060 Ti übernimmt den Display und leichtere Compute-Aufgaben. Die externe RTX 5070 Ti dient als primäre Recheneinheit für LLM-Inferenz, OCR und Embeddings.

Die eGPU-Anbindung läuft über den Thunderbolt-Tunnel mit begrenzter Bandbreite: `2.5 GT/s × 4 Lanes = 10 GT/s` (effektiv ca. 8 Gbit/s nach PCIe-Encoding-Overhead). Das hat am 13. März 2026 zu einem PCIe Completion Timeout (CmpltTO) geführt, der den NVIDIA-Treiber ohne Recovery-Callback eingefroren hat und das gesamte System unbootbar gemacht hat:

```
nvidia 0000:05:00.0: PCIe Bus Error: severity=Uncorrectable (Non-Fatal), type=Transaction Layer
nvidia 0000:05:00.0: [14] CmpltTO (First)
nvidia 0000:05:00.0: AER: can't recover (no error_detected callback)
```

Der egpu-manager soll dieses Szenario künftig auf drei Ebenen verhindern: durch Kernel-Absicherung, durch Echtzeit-Überwachung mit automatischer Recovery, und durch intelligente Workload-Verteilung die die Thunderbolt-Bandbreite nicht überlastet.

---

## 2. Technologie-Stack

**Sprache:** Rust (stable). Begründung: minimaler Speicherverbrauch, kein GC-Overhead der Monitoring-Intervalle verfälscht, direkter sysfs-Zugriff, starke Typsicherheit für Systemoperationen.

**Async Runtime:** Tokio.

**HTTP-Server:** Axum (eingebettet im Daemon, kein separater Prozess). Zwei Bind-Adressen: lokal für Web-UI, konfigurierbar für Remote-API (siehe 3a).

**Desktop-Widget:** GTK4 via gtk4-rs, mit libayatana-appindicator für Systemtray-Kompatibilität unter Wayland/GNOME (siehe 7a).

**Konfiguration:** TOML mit versioniertem Schema (siehe 9a).

**Logging:** SQLite für persistente Ereignishistorie (mit Retention-Policy, siehe 8b), tracing/tracing-subscriber für strukturiertes Laufzeit-Logging.

**IPC:** Unix Domain Socket unter `/run/egpu-manager/egpu-manager.sock` für Kommunikation zwischen Daemon, Widget und CLI.

---

## 3. Architektur-Übersicht

Der egpu-manager besteht aus vier Komponenten die unabhängig voneinander kompiliert und betrieben werden können.

**egpu-managerd** ist der Core-Daemon. Er läuft als systemd-Service, überwacht beide GPUs, führt Recovery durch und stellt den HTTP-Server sowie den Unix-Socket bereit.

**egpu-manager-gtk** ist das GTK4-Desktop-Widget. Es kommuniziert ausschließlich über den Unix-Socket mit dem Daemon und hat keinen direkten Hardware-Zugriff.

**egpu-manager-cli** ist ein Kommandozeilen-Client für Statusabfragen und manuelle Steuerung.

**egpu-manager-web** ist die HTML-Oberfläche die vom eingebetteten Axum-Server ausgeliefert wird.

### 3a. Netzwerk-Bindung und Sicherheitsmodell

Der Daemon betreibt **zwei separate HTTP-Listener** um den Widerspruch zwischen "rein lokal, ohne Auth" und "Remote-Nodes registrieren sich über das Netzwerk" aufzulösen:

**Lokaler Listener (Web-UI + vollständige API):**
- Bind: `127.0.0.1:7842` — **fest im Quellcode, nicht konfigurierbar**. Es gibt kein `[webserver].bind` oder `[webserver].port` in der Konfiguration. Damit ist es unmöglich die Management-API versehentlich nach außen zu exponieren.
- Keine Authentifizierung — alle API-Endpunkte frei zugänglich (nur von localhost erreichbar)
- CORS: `cors_origins` aus der Konfiguration (z.B. `http://localhost:3002` für audit_designer) — einziger konfigurierbare Aspekt des lokalen Listeners
- Dient: Web-UI, SSE-Stream, alle Management-Endpunkte, Wizard

**Remote-Listener (nur Remote-API, optional):**
- Bind: `0.0.0.0:7843` (Port konfigurierbar, standardmäßig deaktiviert)
- Wird nur aktiviert wenn `[remote]` in der Konfiguration vorhanden ist
- **Pre-Shared Token Auth:** Jeder Request muss den Header `Authorization: Bearer <token>` enthalten. Das Token wird bei der erstmaligen Einrichtung generiert (`egpu-manager remote init`) und in `/etc/egpu-manager/remote-token.secret` gespeichert. Dasselbe Token wird im Windows-Setup-Paket eingebettet.
- **Optionales mTLS:** Wenn `tls = true` konfiguriert ist, erwartet der Remote-Listener ein Client-Zertifikat. Die CA wird bei `egpu-manager remote init` generiert. Das Client-Zertifikat wird im Setup-Paket mitgeliefert.
- **Eingeschränkte Endpunkte:** Nur `/api/remote/register`, `/api/remote/unregister`, `/api/remote/heartbeat` sind über den Remote-Listener erreichbar. Kein Zugriff auf Recovery, Wizard oder Konfiguration.

```toml
[remote]
enabled = true
bind = "0.0.0.0"
port = 7843
token_path = "/etc/egpu-manager/remote-token.secret"
tls = false
tls_cert = "/etc/egpu-manager/tls/server.crt"
tls_key = "/etc/egpu-manager/tls/server.key"
tls_ca = "/etc/egpu-manager/tls/ca.crt"    # für mTLS Client-Validierung
```

**Token-Generierung und -Rotation:**
```bash
# Erstmalige Einrichtung (generiert Token + optional TLS-Zertifikate)
egpu-manager remote init

# Token rotieren (invalidiert alle bestehenden Remote-Nodes, Setup-Pakete müssen neu generiert werden)
egpu-manager remote rotate-token

# Aktuelles Token anzeigen (für manuelles Setup)
egpu-manager remote show-token
```

---

## 4. Komponente 1 — Core-Daemon (egpu-managerd)

### 4.1 Systemd-Integration

Der Daemon läuft als systemd-Service mit minimalen Privilegien. Er benötigt ausschließlich folgende Capabilities: `CAP_SYS_RAWIO` für sysfs-Schreibzugriff auf PCIe-Reset, `CAP_NET_BIND_SERVICE` für den HTTP-Server, und Lesezugriff auf `/dev/kmsg` für Kernel-Log-Monitoring.

Der Daemon darf unter keinen Umständen folgende Operationen durchführen: `apt`, `dpkg`, `dkms`, `update-grub`, `update-initramfs`, `grub-install`, `modprobe`, `rmmod`. Diese Operationen sind auf Systemebene durch AppArmor oder systemd-Restrictions zu sperren.

### 4.2 Monitoring-Quellen

Der Daemon überwacht folgende Quellen in konfigurierbaren Intervallen (Standard: 5 Sekunden):

`nvidia-smi --query-gpu=gpu_bus_id,name,temperature.gpu,utilization.gpu,utilization.memory,memory.used,memory.free,power.draw,pstate --format=csv,noheader` für den GPU-Status beider Karten. Dieser Aufruf ist mit einem konfigurierbaren Timeout geschützt (siehe 4.6).

`/sys/bus/pci/devices/0000:05:00.0/aer_dev_nonfatal` für den AER-Fehlerzähler der eGPU — ein steigender Zähler ist das Frühwarnsignal vor einem CmpltTO. Edge Cases für diesen Zähler sind in Abschnitt 4.8 definiert.

`/sys/bus/thunderbolt/devices/` für den Thunderbolt-Verbindungsstatus und die Autorisierung.

`/dev/kmsg` in einem separaten Tokio-Task als Echtzeit-Stream für CmpltTO-Muster — dieser Task hat höchste Priorität und soll einen Freeze erkennen bevor er das System blockiert.

`/var/run/docker.sock` für den Status der bekannten CUDA-Container. Die Fehlerbehandlung für die Docker-API ist in Abschnitt 4.7 definiert.

### 4.2a GPU-Identifikation über PCI-Bus-ID

**WICHTIG:** GPUs werden intern ausschließlich über ihre PCI-Bus-ID identifiziert (`0000:02:00.0` bzw. `0000:05:00.0`), niemals über den nvidia-smi-Index. Der nvidia-smi-Index kann sich nach einem PCIe-Reset, Thunderbolt-Reconnect oder Treiber-Reload ändern.

Der Daemon erstellt beim Start eine Mapping-Tabelle `PCI-Bus-ID → nvidia-smi-Index` und aktualisiert diese nach jedem Recovery-Vorgang. Das Mapping wird aus der nvidia-smi-Ausgabe abgeleitet (Feld `gpu_bus_id`).

In der Konfigurationsdatei und in der REST-API werden GPUs ebenfalls über PCI-Bus-ID referenziert. Die `gpu_device`- und `cuda_fallback_device`-Felder in `[[pipeline]]` enthalten PCI-Bus-IDs statt numerischer Indizes:

```toml
[[pipeline]]
gpu_device = "0000:05:00.0"           # RTX 5070 Ti
cuda_fallback_device = "0000:02:00.0" # RTX 5060 Ti
```

Für die Docker-Container-Konfiguration übersetzt der Daemon die Bus-ID zur Laufzeit in den aktuellen nvidia-smi-Index für `CUDA_VISIBLE_DEVICES` bzw. nutzt `NVIDIA_VISIBLE_DEVICES` mit der UUID aus `nvidia-smi -L`:

```bash
# Bevorzugt: UUID-basiert (stabil über Reboots)
NVIDIA_VISIBLE_DEVICES=GPU-xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx

# Fallback: Bus-ID-basiert
NVIDIA_VISIBLE_DEVICES=0000:05:00.0
```

### 4.2b PCIe-Link-Health-Monitoring

**Dieses Monitoring ist die wichtigste Freeze-Prävention.** AER-Fehlerzähler und kmsg-Stream sind reaktiv — sie melden Probleme die bereits aufgetreten sind. PCIe-Link-Health-Monitoring erkennt Vorboten eines Freeze bevor er eintritt.

Der Daemon überwacht folgende sysfs-Dateien der eGPU im gleichen Intervall wie nvidia-smi:

```
/sys/bus/pci/devices/0000:05:00.0/current_link_speed   # z.B. "2.5 GT/s"
/sys/bus/pci/devices/0000:05:00.0/current_link_width    # z.B. "4"
/sys/bus/pci/devices/0000:05:00.0/max_link_speed        # z.B. "2.5 GT/s"
/sys/bus/pci/devices/0000:05:00.0/max_link_width        # z.B. "4"
```

Zusätzlich für den Thunderbolt-Tunnel:

```
/sys/bus/thunderbolt/devices/0-3/link_speed             # Thunderbolt-Link-Geschwindigkeit
/sys/bus/thunderbolt/devices/0-3/link_width              # Thunderbolt-Link-Breite
```

**Link-Degradation-Erkennung:**

| Zustand | Bedeutung | Aktion |
|---|---|---|
| `current_link_width` < `max_link_width` | Link degradiert (z.B. x4 → x1) | Sofort Warnstufe Orange, proaktive Drosselung |
| `current_link_speed` < `max_link_speed` | Speed degradiert | Warnstufe Gelb, proaktive Drosselung |
| `current_link_speed` = "Unknown" oder Datei nicht lesbar | Link-Flap oder eGPU getrennt | Sofort Warnstufe Orange, Recovery einleiten |
| Thunderbolt `link_speed` ändert sich | Tunnel-Instabilität | Warnstufe Gelb |

**Warum das entscheidend ist:** Der CmpltTO am 13. März 2026 wurde wahrscheinlich durch einen PCIe-Link-Flap ausgelöst — einen kurzzeitigen Verbindungsverlust auf dem Thunderbolt-Tunnel. Vor einem solchen Flap degradiert der Link oft erst (x4 → x2 → x1) bevor er ganz ausfällt. Diese Degradation ist in sysfs sichtbar bevor der erste AER-Fehler auftritt — typischerweise 100–500ms vorher. Das ist das Zeitfenster in dem der egpu-manager **präventiv** handeln kann.

```toml
[gpu]
link_health_check_interval_ms = 500    # Schneller als nvidia-smi-Poll
link_degradation_action = "throttle"   # "throttle", "migrate", "warn_only"
```

### 4.3 Frühwarnsystem

Der Daemon implementiert drei Warnstufen auf Basis des AER-Fehlerzählers, der PCIe-Link-Health und der nvidia-smi-Ausgabe.

**Warnstufe Gelb** wird ausgelöst durch:
- AER-Fehlerzähler steigt innerhalb von 60 Sekunden um mehr als `aer_warning_threshold` (Standard: 3)
- nvidia-smi antwortet nicht innerhalb des konfigurierten Timeouts (siehe 4.6)
- PCIe-Link-Speed degradiert (aber Link-Width noch stabil)
- PCIe-Bandbreitenauslastung über `bandwidth_warning_percent` (Standard: 70 %)
- GPU-Compute-Auslastung über `compute_warning_percent` (Standard: 90 %) bei gleichzeitig wartenden Tasks

**Reaktion bei Gelb — Proaktive Drosselung (nicht nur schnelleres Polling):**
1. Monitoring-Intervall auf 1 Sekunde reduzieren.
2. Link-Health-Monitoring auf 250ms erhöhen.
3. **Keine neuen Tasks auf die eGPU zulassen** — alle neuen Scheduling-Anfragen werden auf die RTX 5060 Ti oder die Warteschlange umgeleitet.
4. Niedrig-priorisierte Tasks (Priorität 4–5) auf der eGPU werden **aktiv auf die interne GPU migriert** sofern dort VRAM verfügbar ist.
5. Mittlere Prioritäten (2–3) bleiben auf der eGPU, werden aber nicht durch neue Tasks ergänzt.
6. Priorität 1 bleibt auf der eGPU und wird erst bei Orange migriert.
7. Widget färbt sich gelb, Log-Eintrag wird geschrieben.

**Warnstufe Orange** wird ausgelöst durch:
- CmpltTO-Muster in `/dev/kmsg` erkannt
- AER-Burst: Zähler steigt um mehr als `aer_burst_threshold` (Standard: 10) in einem Intervall
- PCIe-Link-Width degradiert (z.B. x4 → x1)
- `nvidia_smi_max_consecutive_timeouts` (Standard: 3) aufeinanderfolgende nvidia-smi-Timeouts
- CUDA-Watchdog meldet keine Antwort (siehe 4.10)

**Reaktion bei Orange:** Sofortiger Recovery (4.4). **Alle** Tasks werden von der eGPU migriert, nicht nur niedrig-priorisierte.

**Warnstufe Rot:** Automatischer Recovery fehlgeschlagen. Manuelle Intervention erforderlich. Widget zeigt Warnsymbol, Push-Benachrichtigung versandt.

**Hysterese:** Der Wechsel von einer höheren Warnstufe zurück auf Grün erfolgt nicht sofort, sondern erst nach einer konfigurierbaren Beruhigungsphase (`warning_cooldown_seconds`, Standard: 120). Während der Beruhigungsphase:
- Von Orange → Gelb: Neue Tasks bleiben weiterhin gesperrt für die eGPU.
- Von Gelb → Grün: Tasks werden schrittweise zurückmigriert (niedrigste Priorität zuerst, 30 Sekunden zwischen jeder Migration) um die eGPU nicht sofort wieder voll zu belasten.
- Der Link-Health-Status muss während der gesamten Beruhigungsphase stabil bleiben (keine Degradation, kein AER-Delta). Bei erneutem Trigger: Cooldown-Timer startet neu.

### 4.4 Gestufter Recovery-Prozess

Der Recovery-Prozess wird als State-Machine implementiert. Der aktuelle Zustand wird in der SQLite-Datenbank persistiert (Tabelle `recovery_state`) damit ein Daemon-Neustart den Recovery an der richtigen Stelle fortsetzen kann. Während ein Recovery-Prozess läuft, werden alle neuen Workload-Scheduling-Anfragen in eine Warteschlange gestellt (siehe 4.9).

**Stufe 0 — Stateful-Dienste quiescen (vor jedem Reset):**
Bevor ein GPU-Reset durchgeführt wird, werden alle stateful Dienste der betroffenen Container kontrolliert heruntergefahren um Datenkorruption zu verhindern. Die Reihenfolge ist umgekehrt zur Abhängigkeitskette: **Worker zuerst, dann Queues, dann Datenbanken.**

1. **Celery-Worker:** `celery control shutdown` senden (via Docker exec) für graceful Task-Completion, Timeout 30 Sekunden, dann SIGTERM. Damit werden laufende Tasks abgeschlossen bevor Redis oder die Datenbank gestoppt werden.
2. **Generische GPU-Container:** `docker stop` mit SIGTERM und konfiguriertem Timeout (`container_stop_timeout_seconds`).
3. **Redis-Container:** `BGSAVE` auslösen (via Docker exec), 5 Sekunden warten bis der Snapshot geschrieben ist, dann `docker stop` mit SIGTERM und 10 Sekunden Grace Period.
4. **PostgreSQL-Container:** `CHECKPOINT` auslösen (via Docker exec), dann `docker stop` mit SIGTERM und 15 Sekunden Grace Period.

```toml
[[pipeline]]
# ...
quiesce_hooks = [
  { container = "audit_designer_redis", command = "redis-cli BGSAVE", timeout_seconds = 5 },
  { container = "audit_designer_db", command = "psql -U postgres -c 'CHECKPOINT'", timeout_seconds = 5 }
]
```

**Stufe 1 — PCIe Function Level Reset:**
Der Daemon schreibt `1` in `/sys/bus/pci/devices/0000:05:00.0/reset`. Das dauert typischerweise unter einer Sekunde. Danach wird `nvidia-smi` erneut abgefragt (mit Timeout, siehe 4.6). Ist die GPU wieder erreichbar, ist der Recovery abgeschlossen — die in Stufe 0 gestoppten Dienste werden in umgekehrter Reihenfolge neu gestartet. Wartezeit vor nächster Stufe: 10 Sekunden.

**Stufe 2 — CUDA-Workload-Migration auf Fallback-GPU:**
Der Daemon migriert alle CUDA-Container auf die Fallback-GPU. Da `docker restart` mit geänderten Umgebungsvariablen die Container-Config nicht ändert, nutzt der Daemon **docker compose** für die Migration.

**Wichtig:** Override-Dateien sind **pro Service** benannt, nicht pro Projekt. Damit können mehrere Services im selben Compose-Projekt unabhängig im Fallback sein (z.B. `celery_worker` auf Fallback-GPU aber `jupyter` noch auf eGPU):

1. Der Daemon generiert eine Override-Datei **pro Service** im Projektverzeichnis:
   ```
   docker-compose.egpu-fallback.celery_worker.yml
   docker-compose.egpu-fallback.jupyter.yml
   ```
   Inhalt:
   ```yaml
   services:
     celery_worker:
       environment:
         NVIDIA_VISIBLE_DEVICES: "GPU-uuid-der-fallback-gpu"
   ```
2. Beim Recreate werden **alle** vorhandenen Fallback-Overrides dieses Projekts zusammengeführt:
   ```bash
   docker compose -f docker-compose.yml \
     -f docker-compose.egpu-fallback.celery_worker.yml \
     -f docker-compose.egpu-fallback.jupyter.yml \
     up -d --force-recreate celery_worker
   ```
3. Beim Zurückmigrieren eines einzelnen Service wird nur dessen Override-Datei entfernt und ein Recreate mit den verbleibenden Overrides ausgeführt. Wenn alle Overrides entfernt sind, läuft der Stack wieder im Originalzustand.
4. Der Daemon verwaltet eine Liste aller aktiven Override-Dateien in der SQLite-Datenbank (Tabelle `fallback_overrides`: `compose_file`, `service_name`, `override_path`, `created_at`).

Alternativ — wenn der Container nicht über docker compose verwaltet wird — nutzt der Daemon die Docker-API zum Entfernen und Neuerstellen des Containers mit geänderten Umgebungsvariablen (`docker rm` + `docker create` + `docker start`). Die Container-Konfiguration wird vorher vollständig ausgelesen und nur die GPU-bezogenen Felder geändert.

Danach wird erneut ein PCIe-Reset versucht.

**Stufe 3 — Thunderbolt-Reauthorisierung:**
Der Daemon schreibt `0` in `/sys/bus/thunderbolt/devices/0-3/authorized` und danach `1` zurück. Das löst einen vollständigen Thunderbolt-Reconnect aus. Danach wird das PCI-Bus-ID → nvidia-smi-Index Mapping neu erstellt (4.2a). Anschließend werden die CUDA-Container auf die wiederhergestellte eGPU zurückmigriert — der Override wird entfernt und `docker compose up -d --force-recreate` ausgeführt.

**Stufe 4 — Manuelle Intervention:**
Der automatische Recovery ist ausgeschöpft. Eine Push-Benachrichtigung ist versandt worden. Der Daemon läuft weiter mit der RTX 5060 Ti als alleinige GPU und loggt alle weiteren Ereignisse. Die Container bleiben im Fallback-Modus.

### 4.5 Daemon-Lifecycle: Start, Shutdown und Crash-Recovery

#### 4.5.1 Start-Sequenz

Beim Start führt der Daemon folgende Schritte in dieser Reihenfolge aus:

1. Konfigurationsdatei laden und validieren (inklusive Schema-Version, siehe 9a).
2. SQLite-Datenbank öffnen, Schema-Migration ausführen falls nötig.
3. Prüfen ob ein unterbrochener Recovery-Prozess in der Tabelle `recovery_state` vorliegt. Falls ja: Recovery an der persistierten Stufe fortsetzen.
4. nvidia-smi abfragen um den aktuellen Hardware-Zustand beider GPUs zu ermitteln und das PCI-Bus-ID → Index Mapping zu erstellen.
5. Alle konfigurierten Container über die Docker-API scannen und den aktuellen GPU-Zustand rekonstruieren: welcher Container läuft auf welcher GPU, welcher VRAM ist belegt. Dabei wird geprüft ob Override-Dateien aus einem vorherigen Fallback existieren.
6. AER-Fehlerzähler als Baseline speichern (der aktuelle Wert wird als Referenzpunkt für Delta-Berechnungen genommen, nicht als absoluter Zähler).
7. Monitoring-Tasks starten (nvidia-smi-Poll, AER-Watch, kmsg-Stream, Docker-Watch).
8. HTTP-Server (lokal + optional Remote-Listener) und Unix-Socket starten.
9. Startup-Ereignis in SQLite loggen mit dem rekonstruierten Zustand.

Falls nvidia-smi beim Start nicht antwortet, startet der Daemon im **Degraded Mode**: er loggt den Zustand, setzt Warnstufe Orange und versucht alle 30 Sekunden erneut die GPU zu erreichen. Der HTTP-Server und alle anderen Funktionen laufen trotzdem.

#### 4.5.2 Graceful Shutdown

Bei `SIGTERM` (von `systemctl stop`) führt der Daemon folgende Schritte aus:

1. Alle laufenden Recovery-Prozesse in der SQLite-Datenbank als `interrupted` markieren mit der aktuellen Stufe.
2. Alle SSE-Verbindungen mit einer `shutdown`-Nachricht benachrichtigen.
3. HTTP-Server stoppen (keine neuen Verbindungen, laufende Requests mit 503 beantworten).
4. Container-Zustand in SQLite persistieren: welcher Container läuft auf welcher GPU, ob ein Fallback aktiv ist, welche Override-Dateien existieren.
5. Container werden **nicht** automatisch zurück auf die Primary-GPU gesetzt — sie bleiben in ihrem aktuellen Zustand.
6. Unix-Socket schließen.
7. SQLite-Datenbank sauber schließen (WAL-Checkpoint).
8. Shutdown-Ereignis loggen.

Timeout für den gesamten Shutdown: 15 Sekunden (konfigurierbar). Danach erzwingt systemd SIGKILL.

#### 4.5.3 Crash-Recovery

Wenn der Daemon unerwartet beendet wird (SIGKILL, OOM, Panic):

Beim nächsten Start erkennt er den unsauberen Shutdown daran, dass kein `shutdown`-Ereignis in der SQLite-Datenbank vorliegt seit dem letzten `startup`-Ereignis. In diesem Fall:

1. Warnung loggen: "Unsauberer Shutdown erkannt".
2. Recovery-State prüfen und ggf. fortsetzen (wie bei normalem Start).
3. Docker-Container-Zustand über die Docker-API rekonstruieren (nicht aus der möglicherweise veralteten SQLite-Persistenz).
4. Vorhandene Override-Dateien (`docker-compose.egpu-fallback.*.yml`) pro Service erkennen und den Fallback-Zustand daraus ableiten. Die SQLite-Tabelle `fallback_overrides` wird mit dem tatsächlichen Dateisystem abgeglichen.
5. AER-Fehlerzähler neu einlesen und als neue Baseline setzen.

### 4.6 nvidia-smi Timeout-Handling

nvidia-smi kommuniziert über ioctl mit dem NVIDIA-Treiber. Wenn der Treiber eingefroren ist (genau das CmpltTO-Szenario das den egpu-manager motiviert), kann nvidia-smi unbegrenzt blockieren.

Der Daemon ruft nvidia-smi daher immer als **Tokio-Spawn-Blocking-Task mit Timeout** auf:

```rust
// Pseudocode
let result = tokio::time::timeout(
    Duration::from_secs(config.nvidia_smi_timeout_seconds), // Standard: 5
    tokio::task::spawn_blocking(|| run_nvidia_smi())
).await;
```

Drei Ergebnis-Szenarien:

**Erfolg:** nvidia-smi antwortet innerhalb des Timeouts. Normaler Betrieb. Das PCI-Bus-ID → Index Mapping wird validiert.

**Timeout:** nvidia-smi antwortet nicht innerhalb von `nvidia_smi_timeout_seconds`. Der Daemon:
1. Loggt "nvidia-smi Timeout — GPU möglicherweise eingefroren".
2. Setzt Warnstufe **Gelb** (ab dem ersten Timeout). Die Warnstufe bleibt Gelb solange die Anzahl konsekutiver Timeouts unter `nvidia_smi_max_consecutive_timeouts` (Standard: 3) liegt.
3. Setzt Warnstufe **Orange** und startet Recovery erst wenn `nvidia_smi_max_consecutive_timeouts` erreicht ist (3 aufeinanderfolgende Timeouts).
4. Wechselt auf den kmsg-Stream als primäre Informationsquelle.
5. Der blockierte nvidia-smi-Prozess wird per SIGKILL beendet.
6. Der nächste nvidia-smi-Aufruf erfolgt erst nach `nvidia_smi_retry_interval_seconds` (Standard: 15).

**Parse-Fehler:** nvidia-smi antwortet, aber die Ausgabe ist nicht parsebar (z.B. "GPU access blocked" oder leere Ausgabe). Der Daemon behandelt das wie einen Timeout.

### 4.7 Docker-API Fehlerbehandlung

**Docker-Daemon nicht erreichbar** (Socket existiert nicht oder Connection Refused):
- Loggen als Warning, nicht als Error.
- Alle Docker-abhängigen Features deaktivieren (Container-Monitoring, CUDA-Fallback, Quiesce-Hooks).
- Retry alle 30 Sekunden.
- Warnstufe wird **nicht** erhöht — Docker-Ausfall ist kein GPU-Problem.
- Im Widget erscheint ein "Docker offline"-Badge.

**Container-Neustart fehlgeschlagen** (beim Recovery oder Fallback):
- Erster Versuch: `docker compose up -d --force-recreate <service>` mit 60 Sekunden Timeout.
- Zweiter Versuch: `docker compose stop <service>` (SIGTERM, 10s), dann `docker compose up -d <service>`.
- Dritter Versuch: `docker kill <container>`, `docker rm <container>`, `docker compose up -d <service>`.
- Alle drei fehlgeschlagen: Container als "unrecoverable" markieren, im Widget rot anzeigen. Recovery wird **nicht** abgebrochen — andere Container können trotzdem migriert werden.

**Container existiert nicht** (konfiguriert aber nicht vorhanden):
- Beim Start: Warning loggen, Container aus dem aktiven Scheduling ausschließen.
- Im Widget: grau anzeigen mit "nicht gefunden"-Badge.
- Beim nächsten Docker-Scan prüfen ob der Container inzwischen existiert.

**Docker-API Timeout:**
- Timeout: 10 Sekunden pro API-Call.
- Bei Timeout: Retry nach 5 Sekunden, maximal 3 Versuche.
- Danach: wie "Docker-Daemon nicht erreichbar" behandeln.

### 4.8 AER-Fehlerzähler Edge Cases

Der AER-Fehlerzähler unter `/sys/bus/pci/devices/0000:05:00.0/aer_dev_nonfatal` ist ein Kernel-interner Zähler mit folgenden Sonderfällen:

**Counter-Reset nach PCIe-Reset:** Nach einem PCIe Function Level Reset (Recovery Stufe 1) wird der Zähler vom Kernel auf 0 zurückgesetzt. Der Daemon speichert den Zählerstand vor dem Reset und setzt nach dem Reset eine neue Baseline. Die Delta-Berechnung für das Frühwarnsystem startet mit der neuen Baseline.

**Counter-Reset nach Thunderbolt-Reauthorisierung:** Identisch zum PCIe-Reset — neue Baseline setzen.

**Datei nicht lesbar:** Wenn die sysfs-Datei nicht existiert (eGPU nicht angeschlossen) oder nicht lesbar ist (Berechtigungsproblem), setzt der Daemon die AER-Überwachung aus und verlässt sich ausschließlich auf den kmsg-Stream als Frühwarnung. Im Widget erscheint ein "AER nicht verfügbar"-Hinweis.

**Integer-Overflow:** Der Kernel-Zähler ist ein `unsigned long` (64 Bit). Ein Overflow ist in der Praxis nicht erreichbar. Der Daemon parsed den Wert als `u64`. Falls der neue Wert kleiner als die Baseline ist, wird das als Counter-Reset behandelt (neue Baseline setzen).

**Schnelle Burst-Fehler:** Wenn der Zähler innerhalb eines einzelnen Poll-Intervalls um mehr als `aer_burst_threshold` (Standard: 10) steigt, wird sofort Warnstufe Orange gesetzt — ohne auf das Zeitfenster von 60 Sekunden zu warten.

### 4.9 Recovery-Scheduling-Interaktion

Recovery und Workload-Scheduling sind zwei unabhängige Subsysteme die sich gegenseitig beeinflussen:

**Während ein Recovery läuft:**
- Der Scheduler nimmt keine neuen GPU-Zuweisungen für die betroffene GPU vor.
- Neue Scheduling-Anfragen für die betroffene GPU werden in eine Warteschlange gestellt.
- Scheduling-Anfragen für die nicht-betroffene GPU werden normal verarbeitet.
- Die Warteschlange wird nach erfolgreichem Recovery in Prioritätsreihenfolge abgearbeitet.

**Recovery während aktivem Scheduling:**
- Wenn ein Recovery-Trigger auftritt während der Scheduler gerade einen Container migriert, wird die Migration abgebrochen und der Recovery hat Vorrang.
- Der Scheduler markiert den Container als "migration_interrupted" und wiederholt die Migration nach dem Recovery.

**Deadlock-Vermeidung:**
- Recovery und Scheduling nutzen einen gemeinsamen `RwLock<GpuState>`. Recovery hält den Write-Lock, Scheduling liest mit Read-Lock.
- Timeout für den Write-Lock: 5 Sekunden. Wird der Lock nicht erworben, loggt der Daemon eine Warnung und erzwingt den Lock.

### 4.9a Benutzerinitiierte eGPU-Deaktivierung (Drain-Mode)

Neben dem automatischen Recovery unterstützt der Daemon eine **benutzerinitiierte Deaktivierung** der eGPU. Ziel ist nicht ein garantiertes physisches Stromlosmachen des Gehäuses, sondern eine **saubere logische Abkopplung** vom Host, ohne laufende GPU-Arbeit hart abzubrechen.

**Ablauf nach Klick auf "eGPU nach aktuellem Task deaktivieren":**

1. Der Daemon setzt `egpu_admission_state = "draining"`.
2. Ab diesem Zeitpunkt werden **keine neuen Tasks** mehr auf die eGPU geplant.
3. Bereits laufende Tasks auf der eGPU dürfen sauber fertig laufen. Falls mehrere Tasks aktiv sind, wartet der Daemon auf **alle aktuell laufenden eGPU-Tasks**.
4. Neue Tasks werden währenddessen auf die interne GPU, auf Remote-GPUs oder in die Warteschlange gelenkt.
5. Ollama-Modelle die nur im VRAM liegen aber nicht mehr aktiv genutzt werden, werden entladen.
6. Sobald keine aktiven eGPU-Workloads mehr laufen, schreibt der Daemon `0` in `/sys/bus/thunderbolt/devices/0-3/authorized`.
7. Der Zustand wechselt auf `egpu_admission_state = "disabled"`; die eGPU gilt dann als bewusst deaktiviert und **nicht** als Fehlerfall.

**Wichtig für die UI-Terminologie:**
- Der Button heißt bewusst **nicht** "eGPU sofort ausschalten".
- Standardaktion ist immer ein **graceful drain**.
- Wenn aktuell ein Task läuft, zeigt die UI explizit: "Deaktivierung geplant — eGPU wird nach Ende des aktuellen Tasks deaktiviert."
- Wenn mehrere Tasks laufen, zeigt die UI die genaue Anzahl: "nach Ende von 2 laufenden Tasks".

**Reaktivierung:**
- Über den Button `eGPU aktivieren` oder den API-Endpunkt `/api/egpu/activate`.
- Der Daemon schreibt `1` in `/sys/bus/thunderbolt/devices/0-3/authorized`, wartet auf Re-Enummeration, erstellt das PCI-Bus-ID → nvidia-smi-Index Mapping neu und setzt `egpu_admission_state = "open"`.

**Abgrenzung zu Recovery:**
- `draining` und `disabled` sind **keine** Warn- oder Recovery-Zustände.
- Während `draining` läuft kein Recovery, solange kein echter Fehler auftritt.
- Tritt während des Drain-Modus ein echter Hardware-Fehler auf, hat Recovery Vorrang.

### 4.10 CUDA-Watchdog

**Problem:** nvidia-smi hat einen Polling-Intervall von mindestens 1 Sekunde. Ein CmpltTO friert den Treiber in Millisekunden ein. Der kmsg-Stream erkennt den Freeze — aber erst nachdem der Treiber bereits tot ist.

**Lösung:** Ein separates Watchdog-Binary (`egpu-manager-watchdog`) das einen trivialen CUDA-Call in einer Endlosschleife ausführt und den Daemon über den Zustand informiert.

```c
// egpu-manager-watchdog.c (kompiliert mit nvcc, ca. 20 Zeilen)
// Ruft alle 500ms cudaGetDeviceProperties() auf die eGPU auf.
// Schreibt "OK\n" auf stdout bei Erfolg.
// Timeout: Wenn der Call nicht innerhalb von 2000ms zurückkehrt,
// schreibt der Daemon "FROZEN\n" (wird nie geschrieben, weil der
// Prozess selbst hängt — der Daemon erkennt den Timeout).
```

Der Daemon startet den Watchdog als Child-Prozess und liest dessen stdout. Wenn der Watchdog innerhalb von `cuda_watchdog_timeout_ms` (Standard: 2000) kein "OK" liefert, nimmt der Daemon an dass der CUDA-Treiber eingefroren ist und:

1. Setzt Warnstufe Orange.
2. SIGKILL an den Watchdog-Prozess.
3. Startet den Recovery-Prozess (4.4).
4. Startet den Watchdog nach erfolgreichem Recovery neu.

**Warum ein separates Binary:** Der Watchdog muss CUDA-Bindings haben. Das ist in Rust über `cudarc` möglich, aber ein separates C-Binary ist einfacher zu bauen, hat keine Rust-Abhängigkeiten und kann gezielt ge-SIGKILL-t werden ohne den Daemon zu beeinflussen. Der Watchdog hat keinerlei Logik — er ist ein reiner Heartbeat-Sender.

**Vorteil gegenüber nvidia-smi:** nvidia-smi fragt den Management-Interface des Treibers ab. Der CUDA-Watchdog fragt den Compute-Pfad ab — genau den Pfad der beim CmpltTO einfriert. Der Watchdog erkennt einen Freeze typischerweise 3–5 Sekunden vor nvidia-smi.

```toml
[gpu]
cuda_watchdog_enabled = true
cuda_watchdog_interval_ms = 500
cuda_watchdog_timeout_ms = 2000
cuda_watchdog_binary = "/usr/lib/egpu-manager/egpu-watchdog"
```

Falls der Watchdog nicht installiert oder nicht kompilierbar ist (CUDA-Toolkit nicht vorhanden), läuft der Daemon ohne Watchdog und verlässt sich auf die anderen Monitoring-Quellen. Im Widget erscheint ein Hinweis "CUDA-Watchdog nicht aktiv".

---

## 5. Komponente 2 — GPU-Workload-Verteilung

### 5.1 Grundprinzip

Die Workload-Verteilung folgt dem Prinzip: die RTX 5070 Ti übernimmt alle rechenintensiven Aufgaben die von ihrer höheren Leistung profitieren, die RTX 5060 Ti übernimmt den Display, leichte Aufgaben und dient als Fallback. Eine automatische Verlagerung von der 5070 Ti auf die 5060 Ti erfolgt bei erkanntem Frühwarnzustand (Warnstufe Orange oder höher).

### 5.2 GPU-Zuweisung nach Workload-Typ

| Workload | Primär-GPU | Fallback-GPU | NVIDIA_VISIBLE_DEVICES |
|---|---|---|---|
| LLM-Inferenz (Ollama, llama.cpp) | RTX 5070 Ti | RTX 5060 Ti | `GPU-uuid-5070ti` → Fallback: `GPU-uuid-5060ti` |
| OCR (Donut, Tesseract-GPU) | RTX 5070 Ti | RTX 5060 Ti | `GPU-uuid-5070ti` → Fallback: `GPU-uuid-5060ti` |
| Embeddings (pgvector, sentence-transformers) | RTX 5070 Ti | RTX 5060 Ti | `GPU-uuid-5070ti` → Fallback: `GPU-uuid-5060ti` |
| Celery Worker (audit_designer) | RTX 5070 Ti | RTX 5060 Ti | `GPU-uuid-5070ti` → Fallback: `GPU-uuid-5060ti` |
| Ollama (Host-Service) | RTX 5070 Ti | RTX 5060 Ti | Steuerung über Ollama-API (siehe 5.5) |
| Display/Xorg | RTX 5060 Ti | — | fest |
| Alle anderen Container | keine GPU | — | nicht gesetzt |

**Display-VRAM-Reservierung auf der RTX 5060 Ti:**

Die RTX 5060 Ti übernimmt den Display (Xorg/Wayland Compositor). Der VRAM-Verbrauch des Displays hängt von der Auflösung und der Anzahl der Monitore ab:

| Konfiguration | Display-VRAM-Verbrauch (ca.) |
|---|---|
| 1× 1080p | 50–100 MB |
| 1× 1440p | 100–200 MB |
| 1× 4K | 200–400 MB |
| 2× 4K | 400–700 MB |

Der Daemon ermittelt den tatsächlichen Display-VRAM beim Start über `nvidia-smi --query-gpu=memory.used --format=csv,noheader -i 0` (bevor Container gestartet werden) und speichert diesen Wert als `display_vram_reserved_mb`. Dieser Wert wird vom verfügbaren VRAM der RTX 5060 Ti abgezogen:

```
Verfügbarer VRAM für Compute = memory_total_mb - display_vram_reserved_mb
                              = 8151 - 350 (Beispiel 4K) = 7801 MB
```

Falls der Wert nicht ermittelt werden kann, wird der konfigurierbare Fallback-Wert `display_vram_reserve_mb` verwendet (Standard: 512 MB — konservativ).

```toml
[gpu]
display_vram_reserve_mb = 512   # Fallback wenn automatische Erkennung fehlschlägt
```

### 5.3 Bandbreitenmanagement

Die Thunderbolt-Anbindung der 5070 Ti ist auf `2.5 GT/s × 4 Lanes = 10 GT/s` (effektiv ca. 8 Gbit/s nach PCIe 8b/10b-Encoding) begrenzt.

**Messung der tatsächlichen PCIe-Bandbreite:**

Die Bandbreite kann nicht über die Standard-nvidia-smi-Abfrage gemessen werden (`utilization.memory` ist VRAM-Controller-Auslastung, nicht PCIe-Durchsatz). Der Daemon nutzt stattdessen `nvidia-smi dmon` im Streaming-Modus:

```bash
nvidia-smi dmon -i 1 -s p -d 1
# Ausgabe: pcie_tx (KB/s), pcie_rx (KB/s) — tatsächlicher PCIe-Durchsatz
```

Der Daemon startet `nvidia-smi dmon` als persistenten Child-Prozess und liest den stdout-Stream. Die Werte `pcie_tx` und `pcie_rx` werden in KB/s geliefert und vom Daemon auf den theoretischen Maximaldurchsatz bezogen:

```
Theoretisches Maximum: 10 GT/s × 80% (Encoding-Effizienz) = ~1.000.000 KB/s
Aktuelle Auslastung:   (pcie_tx + pcie_rx) / 1.000.000 × 100%
```

Warnstufe Gelb wird ausgelöst wenn die Auslastung über `bandwidth_warning_percent` (Standard: 70 %) steigt. Bei gleichzeitiger LLM-Inferenz und OCR-Verarbeitung soll der Daemon die OCR-Tasks in eine Warteschlange stellen statt beide parallel auf der 5070 Ti zu verarbeiten.

**Bandbreitenbezogene Freeze-Prävention:**

Hohe PCIe-Bandbreitenauslastung allein verursacht keinen CmpltTO — aber sie verringert den Headroom für Completion-Timeouts. Bei 95%+ Auslastung können normale PCIe-Transaktionen so lange verzögert werden dass sie den Timeout-Wert erreichen. Der Daemon soll daher ab 85% Auslastung aktiv Tasks drosseln (nicht nur warnen), indem er:

1. Neue Task-Starts auf der eGPU blockiert.
2. Batch-orientierte Tasks (Embeddings, OCR-Queues) pausiert.
3. Nur den aktuell laufenden Primär-Task (z.B. LLM-Inferenz) auf der eGPU behält.

### 5.4 Pipeline-Konfiguration

Container werden ausschließlich über `[[pipeline]]`-Blöcke konfiguriert. Das alte `[[docker.containers]]`-Format existiert nicht mehr.

```toml
[[pipeline]]
project = "audit_designer"
container = "audit_designer_celery_worker"
compose_file = "/home/janpow/Projekte/audit_designer/docker-compose.yml"
compose_service = "celery_worker"
workload_types = ["ocr", "embeddings", "llm"]
gpu_priority = 1
gpu_device = "0000:05:00.0"
cuda_fallback_device = "0000:02:00.0"
vram_estimate_mb = 8192
exclusive_gpu = false
restart_on_fallback = true
redis_containers = ["audit_designer_redis"]
depends_on = []
remote_capable = ["llm", "embeddings"]
cuda_only = ["ocr"]
quiesce_hooks = [
  { container = "audit_designer_redis", command = "redis-cli BGSAVE", timeout_seconds = 5 },
  { container = "audit_designer_db", command = "psql -U postgres -c 'CHECKPOINT'", timeout_seconds = 5 }
]
```

Neue Felder gegenüber dem alten Format:
- `compose_file`: Pfad zur docker-compose.yml des Projekts (für `docker compose` Recreate-Flow)
- `compose_service`: Name des Service in der docker-compose.yml
- `gpu_device` und `cuda_fallback_device`: PCI-Bus-IDs statt numerischer Indizes
- `remote_capable`: Workload-Typen die über Netzwerk-Routing auf Remote-GPUs ausgelagert werden können
- `cuda_only`: Workload-Typen die direkten CUDA-Zugriff brauchen und nicht remote-fähig sind
- `quiesce_hooks`: Befehle die vor einem Recovery auf den zugehörigen stateful Containern ausgeführt werden

### 5.5 Ollama als Host-Service-Workload

**Problem:** Ollama läuft als systemd-Service auf dem Host (`systemctl status ollama`), nicht in einem Docker-Container. Es ist in keinem `[[pipeline]]`-Block konfigurierbar, verbraucht aber den Großteil des eGPU-VRAM (LLaMA-3-70B-Q4: ~12 GB, LLaMA-3-8B: ~5 GB, Gemma-2-9B: ~6 GB). Der egpu-manager muss Ollama als Workload kennen und steuern können.

**Lösung:** Der Daemon überwacht Ollama über zwei Kanäle:

**1. Ollama-API-Monitoring (`/api/ps`):**
Der Daemon fragt alle 5 Sekunden `http://localhost:11434/api/ps` ab. Die Antwort enthält alle geladenen Modelle mit ihrem VRAM-Verbrauch:

```json
{
  "models": [
    {
      "name": "llama3:70b-instruct-q4_K_M",
      "size": 42949672960,
      "size_vram": 12884901888,
      "digest": "...",
      "expires_at": "2026-03-14T11:30:00Z"
    }
  ]
}
```

Der Daemon nutzt `size_vram` als tatsächlichen VRAM-Verbrauch für das Scheduling — nicht eine statische Schätzung.

**2. nvidia-smi Process-Monitoring:**
`nvidia-smi pmon -i 1 -c 1` zeigt alle GPU-Prozesse mit PID und VRAM-Verbrauch. Der Daemon korreliert Ollama-PIDs (aus `pgrep ollama_llama_server`) mit dem GPU-VRAM-Verbrauch. Das ist ein Fallback falls die Ollama-API nicht antwortet.

**Steuerung von Ollama:**

Der Daemon kann Ollama nicht direkt steuern wie einen Docker-Container (kein Recreate, kein Env-Var-Override). Stattdessen nutzt er die Ollama-API:

- **Modell entladen:** `POST /api/generate` mit `{"model": "...", "keep_alive": 0}` entlädt ein Modell aus dem VRAM.
- **Modell auf andere GPU laden:** Nicht direkt möglich über die Ollama-API. Bei Fallback auf die RTX 5060 Ti muss Ollama mit geändertem `CUDA_VISIBLE_DEVICES` neugestartet werden. Da der Daemon selbst nicht die Berechtigung hat `systemctl restart ollama` auszuführen oder `/etc/default/ollama` zu schreiben (und das auch nicht soll — siehe Abschnitt 10 und 14), wird dies über einen **dedizierten systemd-Hilfsdienst** gelöst:

  ```ini
  # /etc/systemd/system/egpu-ollama-fallback.service
  [Unit]
  Description=eGPU Ollama GPU Fallback Switch

  [Service]
  Type=oneshot
  ExecStart=/usr/lib/egpu-manager/ollama-fallback.sh
  ```

  ```bash
  # /usr/lib/egpu-manager/ollama-fallback.sh
  # Dieses Skript wird manuell installiert und geprüft (wie kernel-tuning.sh).
  # Der Daemon löst es über D-Bus PolicyKit oder über einen sudoers-Eintrag aus:
  #   egpu-manager ALL=(root) NOPASSWD: /usr/bin/systemctl start egpu-ollama-fallback.service
  # Das Skript liest den gewünschten GPU-Zustand aus /run/egpu-manager/ollama-gpu-target
  # und schreibt ihn in /etc/default/ollama, dann restart ollama.service.
  ```

  Der Daemon schreibt die gewünschte GPU-Zuweisung in `/run/egpu-manager/ollama-gpu-target` (ein Verzeichnis auf das er Schreibrechte hat) und löst dann den Hilfsdienst aus. Der Hilfsdienst läuft als root und führt den eigentlichen Restart durch. Damit bleibt der Daemon-Prozess selbst unprivilegiert.

  **Alternativer Ansatz ohne systemd-Hilfsdienst:** Ollama seit Version 0.5 unterstützt die Umgebungsvariable `OLLAMA_GPU_DEVICES` die über die Ollama-API gesetzt werden kann (experimentell). Falls verfügbar, nutzt der Daemon diesen Pfad und braucht keinen systemd-Restart.
- **Modell auf Remote-GPU laden:** `OLLAMA_HOST` ist ein Client-seitiges Setting. Ollama selbst wird nicht umkonfiguriert — stattdessen werden die Anwendungen (Docker-Container) so konfiguriert dass sie den Remote-Ollama ansteuern.

**Ollama-Konfiguration im egpu-manager:**

```toml
[ollama]
enabled = true
host = "http://localhost:11434"
poll_interval_seconds = 5
gpu_device = "0000:05:00.0"               # Auf welcher GPU läuft Ollama
fallback_device = "0000:02:00.0"
systemd_unit = "ollama.service"            # Für Restart bei Fallback
env_file = "/etc/default/ollama"           # Environment-Datei für GPU-Override
priority = 1                               # Ollama hat höchste Priorität (Nutzer wartet)
max_vram_mb = 14000                        # Maximaler VRAM den Ollama nutzen darf
auto_unload_idle_minutes = 10              # Idle-Modelle nach 10 Min entladen
```

**Scheduling-Integration:**
- Der VRAM-Verbrauch von Ollama wird in die VRAM-Scheduling-Logik (5c.2) einbezogen — als dynamischer Wert aus der API, nicht als statische Schätzung.
- Wenn ein neuer Pipeline-Task VRAM auf der eGPU braucht und Ollama den Großteil belegt, kann der Daemon über die API ein idle Modell entladen um Platz zu machen.
- Bei Warnstufe Orange wird Ollama als erstes von der eGPU genommen (Modelle entladen, dann systemd-Restart auf Fallback-GPU).

### 5.6 Celery Task-Type Reporting

**Problem:** Ein einzelner Celery-Worker kann sequentiell OCR (8 GB VRAM), Embeddings (2 GB) und LLM (4 GB) Tasks ausführen. Der egpu-manager sieht nur "celery_worker läuft auf GPU X" und weiß nicht ob gerade ein 200 MB-Embedding-Task oder ein 8 GB-OCR-Task läuft. Die `vram_estimate_mb`-Schätzung muss den Worst-Case annehmen und verschwendet damit Scheduling-Kapazität.

**Lösung:** Der Celery-Worker meldet den aktuellen Task-Typ an den egpu-manager über einen HTTP-Webhook.

**Webhook-Endpunkt im egpu-manager:**

```
POST /api/pipelines/{container}/workload-update
Body: {
  "workload_type": "ocr",
  "vram_estimate_mb": 8192,
  "task_id": "abc-123",
  "started_at": "2026-03-14T10:30:00Z"
}
```

**Integration in die Celery-Worker:**

Die bestehenden Celery-Worker in audit_designer und flowinvoice erhalten einen Task-Decorator der vor und nach jedem GPU-Task den egpu-manager informiert:

```python
# audit_designer/backend/app/tasks/gpu_tasks.py
import httpx

def notify_egpu_manager(container: str, workload_type: str, vram_mb: int):
    try:
        httpx.post(f"http://localhost:7842/api/pipelines/{container}/workload-update",
                   json={"workload_type": workload_type, "vram_estimate_mb": vram_mb},
                   timeout=2.0)
    except Exception:
        pass  # Fire-and-forget, Daemon-Ausfall blockiert keine Tasks

@celery_app.task
def run_ocr(document_id: str):
    notify_egpu_manager("audit_designer_celery_worker", "ocr", 8192)
    try:
        # ... OCR-Task ...
    finally:
        notify_egpu_manager("audit_designer_celery_worker", "idle", 0)
```

**Scheduling-Auswirkung:**
- Wenn kein Workload-Update kommt, nutzt der Scheduler `vram_estimate_mb` aus der Config (Worst-Case).
- Wenn ein Workload-Update kommt, nutzt der Scheduler den gemeldeten Wert und gibt den Differenz-VRAM frei für andere Tasks.
- Im Pipeline-Widget wird der aktuelle Task-Typ live angezeigt (z.B. "OCR — 8192 MB" statt nur "aktiv").

---

## 5a. Pipeline-Analyse durch Claude Code (Phase 0)

Bevor die eigentliche Entwicklung beginnt, soll Claude Code alle vorhandenen Projekte analysieren und daraus automatisch die vollständigen Container-Profile und Pipeline-Definitionen für die Konfigurationsdatei ableiten. Das ist eine einmalige Analyse-Phase die vor Phase 1 der Entwicklung ausgeführt wird.

### 5a.1 Anweisung an Claude Code für die Pipeline-Analyse

Claude Code soll folgende Verzeichnisse und Dateien in dieser Reihenfolge analysieren:

```
~/Projekte/audit_designer/
~/Projekte/flowinvoice/
~/Projekte/hpp/
~/Projekte/Workshop/
```

Für jedes Projekt soll Claude Code folgende Dateien auswerten: `docker-compose.yml` und `docker-compose.*.yml` für Container-Definitionen und Umgebungsvariablen, `Dockerfile` und `Dockerfile.*` für CUDA-Base-Images und Treiber-Abhängigkeiten, `requirements.txt`, `pyproject.toml` und `Cargo.toml` für GPU-Bibliotheken (torch, tensorflow, cuda, onnxruntime-gpu, sentence-transformers, donut-python), Celery-Task-Definitionen für GPU-intensive Tasks, und `.env`-Dateien für bestehende CUDA-Konfigurationen.

Claude Code soll außerdem den Pfad zur `docker-compose.yml` und den Service-Namen (`compose_service`) korrekt erfassen sowie die PCI-Bus-IDs für `gpu_device` und `cuda_fallback_device` verwenden.

Das Ergebnis der Analyse ist eine Datei `pipeline-profiles.toml` im Projektverzeichnis des egpu-manager, die vor der Integration in die Hauptkonfiguration manuell geprüft wird.

### 5a.2 Erkannte Pipelines (Vorausfüllung auf Basis bekannter Projektstruktur)

**audit_designer** nutzt Celery für asynchrone GPU-Tasks. Der `celery_worker`-Container übernimmt OCR über Donut Vision Transformer, Embeddings über sentence-transformers und pgvector, sowie LLM-Analyse über Ollama/Llama. Jupyter dient als interaktive Entwicklungsumgebung mit direktem CUDA-Zugriff. Der Flower-Container ist ein reiner Monitor ohne GPU-Bedarf. Backend, Frontend und Redis haben keinen GPU-Bedarf.

**flowinvoice** nutzt GPU für Rechnungsanalyse via OCR und LLM. Redis ist ohne GPU-Bedarf, hat aber durch den Freeze eine AOF-Korruption erlitten — der egpu-manager soll Redis bei jedem Recovery-Vorgang kontrolliert stoppen.

**hpp** (Hessisches Preismonitoring-Portal) hat nach aktuellem Kenntnisstand keinen GPU-Bedarf — Backend, Frontend und PostgreSQL laufen ohne CUDA. Claude Code soll das in der Analyse bestätigen oder widerlegen.

---

## 5b. Projekt-Wizard — Neues Projekt hinzufügen

### 5b.1 Konzept

Der Wizard erlaubt es jederzeit ein neues Projekt in den egpu-manager einzutragen ohne die Konfigurationsdatei manuell zu bearbeiten. Er ist sowohl über die Weboberfläche als auch über die CLI erreichbar. Der Wizard führt eine automatische Erkennung durch, bereitet das Ergebnis auf und legt die fertige Pipeline-Definition zur Bestätigung vor. Erst nach manueller Bestätigung wird die `config.toml` aktualisiert (transaktional, siehe 9a) und der Daemon lädt die Konfiguration neu — ohne Neustart.

### 5b.2 Wizard-Ablauf (Web)

**Schritt 1 — Projektpfad angeben:**
Der Nutzer gibt den Pfad zum Projektverzeichnis ein oder wählt ihn über einen Verzeichnis-Browser. Der Wizard prüft sofort ob das Verzeichnis lesbar ist und ob eine `docker-compose.yml` oder ein `Dockerfile` vorhanden ist.

**Schritt 2 — Automatische Erkennung:**
Der Wizard analysiert das Projektverzeichnis nach denselben Kriterien wie die Phase-0-Analyse (Abschnitt 5a.1). Er erkennt: gefundene Container mit Image-Namen, erkannte GPU-Bibliotheken, Workload-Typen, Redis-/DB-Container, bestehende CUDA-Konfigurationen, `compose_file`-Pfad und `compose_service`-Name. Die Erkennung läuft asynchron mit Fortschrittsbalken.

**Schritt 3 — Aufbereitung und Bestätigung:**
Editierbares Formular mit allen Feldern. Unsichere Felder gelb markiert. Quiesce-Hooks werden automatisch vorgeschlagen wenn Redis- oder PostgreSQL-Container erkannt werden.

**Schritt 4 — Live-Test (optional):**
Container mit zugewiesener GPU starten, CUDA-Initialisierung prüfen über `nvidia-smi`.

**Schritt 5 — Übernehmen:**
Transaktionaler Write in die `config.toml` (siehe 9a), Hot-Reload, sofortige Anzeige im Pipeline-Widget.

### 5b.3 Wizard-Ablauf (CLI)

```bash
egpu-manager wizard add ~/Projekte/mein-neues-projekt
egpu-manager wizard remove flowinvoice
egpu-manager wizard edit audit_designer_jupyter
egpu-manager wizard list
```

### 5b.4 Erkennungs-Bibliothek

Die automatische Erkennung ist als eigenständige Rust-Crate `egpu-manager-detector` implementiert:

| Bibliothek / Package | Erkannter Workload-Typ |
|---|---|
| `torch`, `tensorflow`, `jax` | llm, training |
| `sentence-transformers`, `faiss-gpu` | embeddings |
| `donut-python`, `pytesseract` mit GPU | ocr |
| `onnxruntime-gpu` | inference |
| `llama-cpp-python`, `ollama` | llm |
| `cuda-toolkit` im Dockerfile | generic-cuda |
| `pgvector` | embeddings |
| `transformers` + `accelerate` | llm, training |

---

## 5c. Prioritätssystem für GPU-Workload-Verteilung

### 5c.1 Prioritätsstufen

| Priorität | Bedeutung | Zuweisung bei Normalbetrieb (Grün) | Zuweisung bei Warnstufe Gelb | Zuweisung bei Orange/Rot |
|---|---|---|---|---|
| 1 — Kritisch | Aktiver Produktions-Task, Nutzer wartet | RTX 5070 Ti bevorzugt, andere verdrängen | Bleibt auf eGPU (wenn bereits dort), keine neuen Starts auf eGPU | Wird auf Fallback-GPU migriert |
| 2 — Hoch | Hintergrund-Task mit Zeitvorgabe | RTX 5070 Ti wenn frei, sonst Fallback | Fallback-GPU | Fallback-GPU |
| 3 — Normal | Reguläre Hintergrundverarbeitung | RTX 5070 Ti wenn VRAM ausreicht | Fallback-GPU | Fallback-GPU |
| 4 — Niedrig | Batch-Verarbeitung ohne Zeitdruck | Fallback-GPU wenn Prio-1/2 aktiv | Aktiv von eGPU migriert | Fallback-GPU |
| 5 — Minimal | Optionale Hintergrundaufgaben | Nur wenn beide GPUs unter 30 % | Aktiv von eGPU migriert | Fallback-GPU |

**Explizite Entscheidungsregel für Priorität 1 vs. Sicherheit:**

Sicherheit hat immer Vorrang vor Priorität. Die Regel "Immer RTX 5070 Ti" gilt **nur bei Warnstufe Grün**. Sobald die Warnstufe auf Gelb oder höher steigt, gelten die Sicherheitsregeln:

- **Gelb:** Priorität-1-Tasks die bereits auf der eGPU laufen bleiben dort. Aber neue Prio-1-Tasks starten auf der Fallback-GPU. Der Nutzer wird informiert dass die eGPU unter Beobachtung steht.
- **Orange:** Alle Tasks werden migriert — auch Priorität 1. Kein Task läuft auf der eGPU.
- **Compute >90%:** Kein neuer Task auf der betroffenen GPU — unabhängig von der Priorität. Bestehende Tasks laufen weiter.

Das bedeutet: Die Priorität regelt die Reihenfolge unter Tasks, aber nicht die Sicherheitsentscheidung ob eine GPU überhaupt genutzt werden darf.

### 5c.2 VRAM- und Compute-Scheduling-Logik

Das Scheduling basiert auf **drei Inputs** (nicht nur VRAM-Schätzungen):

**1. Tatsächlicher VRAM-Verbrauch (primär):**
Der Daemon nutzt den tatsächlich belegten VRAM aus `nvidia-smi --query-gpu=memory.used` und `nvidia-smi pmon` (pro Prozess) als Hauptgrundlage. Für Ollama wird `size_vram` aus der Ollama-API verwendet (5.5). Für Container die per Workload-Update melden (5.6), wird der gemeldete Wert verwendet.

**2. VRAM-Schätzung (Fallback):**
`vram_estimate_mb` wird nur verwendet wenn kein tatsächlicher Verbrauchswert verfügbar ist (Container noch nicht gestartet, nvidia-smi nicht erreichbar, kein Workload-Update). Der Daemon loggt Abweichungen >20 % zwischen Schätzung und tatsächlichem Verbrauch und schlägt im Widget eine Korrektur vor.

**3. GPU-Compute-Auslastung (Schwellenwert):**
Auch wenn VRAM verfügbar ist, kann ein neuer Task die Inferenz-Geschwindigkeit aller Tasks ruinieren wenn die GPU-Compute-Auslastung bereits hoch ist. Der Daemon prüft `utilization.gpu` aus nvidia-smi:

| Compute-Auslastung | Scheduling-Entscheidung |
|---|---|
| < 70 % | Neuer Task darf starten |
| 70–90 % | Neuer Task darf nur starten wenn Priorität ≤ 2 |
| > 90 % | Kein neuer Task, auch nicht Priorität 1 — Warteschlange oder Fallback-GPU |

Konfigurierbar:
```toml
[gpu]
compute_warning_percent = 90
compute_soft_limit_percent = 70
```

**VRAM-Budget pro GPU:**

| GPU | Total VRAM | Display-Reserve | Verfügbar für Compute |
|---|---|---|---|
| RTX 5070 Ti | 16.303 MB | 0 MB (kein Display) | 16.303 MB |
| RTX 5060 Ti | 8.151 MB | ~350 MB (4K, automatisch ermittelt) | ~7.800 MB |
| RTX 5060 Remote | 16.384 MB | 0 MB (headless) | 16.384 MB |

**Entscheidungslogik:**

- Neuer Task höhere Priorität → laufender Task wird auf Fallback verlagert (sofern VRAM und Compute dort ausreichen).
- Kein Fallback möglich → Warteschlange, Nutzer benachrichtigen.
- Gleiche Priorität → First-Come-First-Served.
- Bei Warnstufe Gelb → Keine neuen Tasks auf eGPU (siehe 4.3).
- Bei `egpu_admission_state = "draining"` → Keine neuen Tasks auf eGPU; nur bereits laufende eGPU-Tasks dürfen fertig laufen.
- Bei `egpu_admission_state = "disabled"` → eGPU wird vom Scheduler vollständig ignoriert bis sie explizit wieder aktiviert wird.

### 5c.3 Manuelle Prioritätsänderung zur Laufzeit

```bash
egpu-manager priority set audit_designer_celery_worker 1
egpu-manager priority set audit_designer_jupyter 4
```

---

## 5d. Remote-GPU über Netzwerk

### 5d.1 Konzept und Einsatzszenario

Eine RTX 5060 mit 16 GB VRAM steht gelegentlich auf einem anderen Rechner im lokalen Netzwerk zur Verfügung. Der egpu-manager behandelt sie als optionale dritte GPU mit dem Status "verfügbar" oder "nicht verfügbar".

Remote-GPU-Zugriff erfolgt nicht über CUDA direkt, sondern über Inference-Dienste (Ollama, llama.cpp) die auf dem Remote-Node laufen. Die Kommunikation wird über den Remote-Listener (Abschnitt 3a) mit Token-Auth abgesichert.

### 5d.2 Remote-Node-Konfiguration

```toml
[[remote_gpu]]
name = "remote-5060"
host = "192.168.1.XXX"
port_ollama = 11434
port_llama_cpp = 8080
port_egpu_agent = 7843
gpu_name = "NVIDIA GeForce RTX 5060"
vram_mb = 16384
availability = "on-demand"       # "always", "on-demand", "scheduled"
check_interval_seconds = 30
connection_timeout_seconds = 5
priority = 2
auto_assign = false
max_latency_ms = { llm = 50, embeddings = 100, batch = 500 }
```

### 5d.2a Latenz-Schwellenwerte für Remote-Routing

Nicht jeder Workload-Typ ist bei jeder Latenz noch sinnvoll auf die Remote-GPU auslagerbar:

| Workload-Typ | Max. akzeptable Latenz | Begründung |
|---|---|---|
| LLM-Inferenz (Chat, interaktiv) | 50 ms | Nutzer wartet auf Token-Stream, Latenz addiert sich pro Token |
| LLM-Inferenz (Batch, Hintergrund) | 200 ms | Kein Nutzer wartet, Gesamtdurchsatz wichtiger |
| Embeddings | 100 ms | Batch-orientiert, Latenz fällt bei vielen Embeddings kaum ins Gewicht |
| OCR | — (nicht remote-fähig) | Braucht direktes CUDA |
| Batch-Verarbeitung | 500 ms | Nur Durchsatz zählt |

Der Daemon misst die Round-Trip-Latenz zum Remote-Node bei jedem Healthcheck (ICMP-Ping oder HTTP-Response-Time). Wenn die Latenz den konfigurierten Schwellenwert für den jeweiligen Workload-Typ überschreitet, wird der Workload **nicht** auf den Remote-Node geroutet — auch wenn die Remote-GPU verfügbar ist und VRAM hat.

Im Widget wird die aktuelle Latenz neben der Remote-GPU angezeigt. Bei Überschreitung eines Schwellenwerts: gelbe Markierung mit Hinweis "Latenz zu hoch für LLM-Chat (aktuell: 85ms, max: 50ms)".

### 5d.3 Verfügbarkeits-Management

- Healthcheck alle 30 Sekunden via HTTP gegen Ollama/llama.cpp.
- "nicht verfügbar" → "verfügbar": Benachrichtigung, kein automatischer Workload-Shift (außer `auto_assign = true`).
- "verfügbar" → "nicht verfügbar": Sofortiger Failover auf lokale GPUs.

### 5d.4 Workload-Routing

**Ollama-Routing:** `OLLAMA_HOST=http://remote-ip:11434` per docker compose Override.

**llama.cpp-Routing:** Remote-URL in Container-Konfiguration eintragen.

**Direkte CUDA-Workloads:** Nicht remote-fähig. Im Widget als "nur lokal" markiert. Feld `cuda_only` in der Pipeline-Definition.

### 5d.5 GPU-Pool und Gesamtpriorität

| GPU | VRAM | Anbindung | Stärken | Schwächen |
|---|---|---|---|---|
| RTX 5070 Ti (eGPU) | 16 GB | Thunderbolt, 10 GT/s | Höchste Rechenleistung | Bandbreite begrenzt, Freeze-Risiko |
| RTX 5060 Ti (intern) | 8 GB | PCIe x8 direkt | Stabil, immer verfügbar | Weniger VRAM |
| RTX 5060 (Remote) | 16 GB | Netzwerk (LAN/WLAN) | Viel VRAM, kein Thunderbolt-Risiko | Latenz, nicht immer verfügbar |

### 5d.6 egpu-manager-Agent auf dem Remote-Node

Agent-Modus über Feature-Flag `--features agent-only`. Läuft auf Port 7843, authentifiziert sich mit dem Pre-Shared Token am Primary. Liefert: GPU-Status, laufende Prozesse, Latenz, Ollama/llama.cpp-Verfügbarkeit.

---

## 5e. Windows-11-Setup-Generator für den Remote-Node

### 5e.1 Konzept

Der egpu-manager generiert über die Weboberfläche ein ZIP-Setup-Paket für den Windows-11-Remote-Node. Das Paket enthält alles für eine automatische Einrichtung per PowerShell.

Der primäre Zielablauf ist **offline-tauglich**:
1. Setup-ZIP auf dem NUC generieren
2. ZIP auf einen USB-Stick kopieren
3. USB-Stick am Windows-11-Rechner einstecken
4. ZIP lokal auf dem Windows-Rechner entpacken
5. `install.ps1` dort als Administrator starten

Das Setup ist damit auch dann nutzbar, wenn der Windows-Rechner noch keinen Zugriff auf ein gemeinsames Netzlaufwerk oder keine direkte Dateiübertragung per SMB hat.

### 5e.2 Inhalt des Setup-Pakets

```
egpu-remote-setup/
├── README.txt                          # Kurzanleitung (Deutsch)
├── install.ps1                         # Hauptinstallationsskript
├── uninstall.ps1                       # Deinstallationsskript
├── config/
│   ├── ollama-config.json              # mit NUC-IP vorausgefüllt
│   ├── egpu-agent-config.toml          # mit NUC-IP, Port, Token
│   ├── auth-token.secret               # Pre-Shared Token für API-Auth
│   └── firewall-rules.ps1
├── services/
│   ├── ollama-service.xml
│   └── egpu-agent-service.xml
├── checksums/
│   └── SHA256SUMS.txt                  # Hashes aller Dateien
└── installers/
    ├── ollama-windows-amd64.exe        # pinned Version mit Hash
    └── nssm.exe                        # pinned Version mit Hash
```

### 5e.2a Offline-/USB-Installationsablauf

**Auf dem NUC (Linux):**
1. Nutzer öffnet die egpu-manager-Weboberfläche.
2. Nutzer klickt auf `Windows-Setup für Remote-GPU generieren`.
3. Der Daemon erzeugt `egpu-remote-setup.zip`.
4. Die Weboberfläche zeigt danach explizit an: "ZIP auf USB-Stick kopieren und auf dem Windows-11-Rechner entpacken."

**Transport:**
1. ZIP-Datei wird auf einen USB-Stick kopiert.
2. Empfohlene Dateisysteme für den Stick: exFAT oder NTFS.
3. Die ZIP-Datei bleibt unverändert; kein manuelles Editieren im Archiv.

**Auf dem Windows-11-Rechner:**
1. USB-Stick einstecken.
2. `egpu-remote-setup.zip` in ein lokales Verzeichnis kopieren, z.B. `C:\Users\<Name>\Downloads\egpu-remote-setup.zip`.
3. ZIP nach `C:\egpu-remote\setup\` entpacken.
4. PowerShell **als Administrator** öffnen.
5. In das entpackte Verzeichnis wechseln:

   ```powershell
   cd C:\egpu-remote\setup\egpu-remote-setup
   ```

6. Das Installationsskript starten:

   ```powershell
   .\install.ps1
   ```

7. Das Skript führt Integritätsprüfung, Installationen, Firewall-Regeln und die Registrierung am NUC aus.

**Wichtig:**
- Das ZIP wird immer **lokal entpackt** und nicht direkt vom USB-Stick ausgeführt.
- Dadurch werden Probleme mit Execution Policy, temporären Pfaden und gesperrten Installer-Dateien reduziert.
- `README.txt` im Paket beschreibt denselben Ablauf in Kurzform auf Deutsch.

### 5e.3 Supply-Chain-Sicherheit

**Ollama-Installer:** Version wird in der egpu-manager-Konfiguration gepinnt (`ollama_version = "0.6.2"`). Download über HTTPS mit SHA256-Prüfung. Der Hash wird aus der offiziellen Release-Seite ermittelt und im Build-Manifest des egpu-manager festgehalten. Bei Hash-Mismatch bricht die Generierung ab.

**NSSM:** Version 2.24, SHA256-Hash fest im Quellcode hinterlegt. Download von `https://nssm.cc/release/nssm-2.24.zip`. Bei Hash-Mismatch: Abbruch.

**Paket-Integrität:** `checksums/SHA256SUMS.txt` enthält Hashes aller Dateien. Das Installationsskript prüft Integrität in Schritt 0.

### 5e.4 Das PowerShell-Installationsskript

**Schritt 0 — Execution Policy und Integrität:**
Prüft PowerShell Execution Policy, gibt Anweisung für temporäres Bypass. Prüft SHA256-Hashes aller Dateien gegen `SHA256SUMS.txt`.

**Schritt 1 — Voraussetzungen:** Windows 11, Admin-Rechte, NVIDIA-Treiber ≥576.02, 20 GB Speicherplatz, NUC erreichbar.

**Schritt 2 — NVIDIA-Treiber:** Prüfung, ggf. Download-Link und Pause. Fortschritt persistiert in `C:\egpu-remote\install-progress.json`.

**Schritt 3 — Ollama:** Installer ausführen, als Windows-Service via NSSM, `OLLAMA_HOST=0.0.0.0:11434`.

**Schritt 4 — Firewall:** Ports 11434, 8080, 7843 nur von NUC-IP. Benannte Regeln für saubere Deinstallation.

**Schritt 5 — Agent (optional):** Binary installieren, als Service, Token aus `auth-token.secret` einlesen.

**Schritt 6 — Registrierung:** HTTP POST an `http://NUC-IP:7843/api/remote/register` mit Token-Auth. NUC bestätigt, Remote-GPU erscheint im Widget (nach Nutzer-Bestätigung).

**Schritt 7 — Zusammenfassung und Log:** `C:\egpu-remote\install.log`.

### 5e.5 Deinstallation

`uninstall.ps1` entfernt Dienste, Firewall-Regeln, Konfiguration. Sendet Abmeldung an NUC via `/api/remote/unregister`.

---

## 5f. audit_designer GPU-Dashboard-Integration

### 5f.1 Konzept

Der audit_designer erhält ein eingebettetes GPU-Dashboard-Widget das den aktuellen GPU-Zustand direkt in der Anwendung anzeigt. Der Nutzer muss nicht zwischen audit_designer und egpu-manager-Weboberfläche wechseln.

### 5f.2 Datenquelle

Das Widget bezieht Daten über die REST-API des egpu-manager (`http://localhost:7842`). Es nutzt `GET /api/events/stream` (SSE) für Echtzeit-Updates, `GET /api/status` für den globalen Zustand und `GET /api/pipelines` für Pipeline-spezifische Entscheidungsgründe.

Falls der Daemon nicht erreichbar ist, zeigt das Widget dezent "GPU-Manager nicht verfügbar" und blockiert keine Funktionalität.

### 5f.3 Dashboard-Ansicht

**Panel (Seitenleiste, eingeklappt):** Farbiger Punkt + Kurzstatus wie "GPU OK", "eGPU gedrosselt" oder "Recovery läuft". Zusätzlich ein kleines Queue-Badge falls wartende Tasks existieren.

**Panel (Seitenleiste, ausgeklappt):**
- Kompakte Betriebsleiste: Warnstufe, `eGPU offen/gedrosselt/gesperrt`, Queue-Länge, Recovery-Stufe, Remote-Verfügbarkeit
- VRAM-Gesamtbalken beider GPUs (gestapelt, farblich je Pipeline)
- Eigene audit_designer-Pipelines: GPU-Zuweisung, VRAM, Workload, Priorität und **Entscheidungsgrund** (z.B. "Fallback wegen Gelb", "wartet wegen VRAM", "Remote wegen Latenz gesperrt")
- Letztes Recovery-Ereignis mit Kurzgrund
- Remote-GPU-Status mit aktueller Latenz und Routing-Hinweis

**Vollbild (`/gpu-dashboard`):** Vollständige Pipeline-Übersicht aller Projekte als read-only Diagnoseansicht. Schreibende Aktionen werden bewusst nicht angeboten; stattdessen klarer Link "Im GPU-Manager öffnen".

### 5f.4 Interaktive Funktionen und Grenzen

- Panel ein-/ausklappen, eigene Pipelines filtern, Vollbild öffnen
- Link "Im GPU-Manager öffnen" → `http://localhost:7842`
- Toasts quittieren und letzte Warnung aufklappen
- **Keine** Recovery-Aktionen aus dem audit_designer
- **Keine** Prioritätsänderung und **keine** manuelle GPU-Zuweisung im eingebetteten Dashboard; diese Aktionen bleiben exklusiv der Hauptoberfläche vorbehalten

### 5f.5 Implementierung

**Frontend (Vue 3):**
- `frontend/src/components/gpu/GpuDashboard.vue`
- `frontend/src/composables/useGpuManager.ts` (SSE, API-Client, reaktiver Zustand)
- Sidebar-Integration als ausklappbares Panel
- Route `/gpu-dashboard` für Vollbild

**Backend:** Kein Backend-Code nötig — Frontend kommuniziert direkt mit `localhost:7842`.

**CORS-Konfiguration im egpu-manager:**
```toml
[local_api]
cors_origins = ["http://localhost:3002"]
```

Das eingebettete Dashboard nutzt ausschließlich lesende Endpunkte (`GET`, SSE). Schreibende API-Calls aus `audit_designer` sind nicht Teil des Standardszenarios.

### 5f.6 Benachrichtigungen

Toast-Nachrichten im audit_designer bei Warnstufen-Änderungen:
- **Gelb:** Gelber Toast, 10s sichtbar.
- **Orange:** Oranger Toast, bleibt sichtbar bis Entwarnung.
- **Rot:** Roter Toast mit Link zur egpu-manager-Weboberfläche, bleibt sichtbar.

---

## 5g. Anwendungsintegration — GPU-Aware Applications

### 5g.1 Grundprinzip

Die Anwendungen (audit_designer, flowinvoice, Workshop) sollen den egpu-manager **kennen und kooperativ nutzen**. Die reine "Kontrolle von außen" (docker compose Override) ist ein Fallback für den Fehlerfall — im Normalbetrieb sollen Anwendungen **vor** einem GPU-Task den egpu-manager fragen welche GPU sie nutzen sollen und **nach** dem Task die GPU wieder freigeben.

Das Grundmodell ist ein **GPU-Leasing**: Die Anwendung reserviert GPU-Kapazität für einen bestimmten Workload, nutzt sie, und gibt sie danach zurück. Der egpu-manager entscheidet basierend auf Priorität, verfügbarem VRAM, Warnstufe und Bandbreite welche GPU zugewiesen wird.

### 5g.2 API-Endpunkte für Anwendungsintegration

**`POST /api/gpu/acquire`** — GPU-Kapazität reservieren (vor Task-Start)

```json
// Request
{
  "pipeline": "audit_designer_celery_worker",
  "workload_type": "ocr",
  "vram_mb": 8192,
  "duration_estimate_seconds": 300,
  "priority_override": null
}

// Response (Erfolg)
{
  "granted": true,
  "gpu_device": "0000:05:00.0",
  "gpu_uuid": "GPU-bd7dd984-fd6a-3d83-a22c-539b5b438290",
  "nvidia_visible_devices": "GPU-bd7dd984-fd6a-3d83-a22c-539b5b438290",
  "lease_id": "lease-abc-123",
  "expires_at": "2026-03-14T11:05:00Z",
  "warning_level": "green",
  "message": "eGPU zugewiesen"
}

// Response (abgelehnt — eGPU gedrosselt)
{
  "granted": true,
  "gpu_device": "0000:02:00.0",
  "gpu_uuid": "GPU-xxxxx-interne-gpu",
  "nvidia_visible_devices": "GPU-xxxxx-interne-gpu",
  "lease_id": "lease-def-456",
  "expires_at": "2026-03-14T11:05:00Z",
  "warning_level": "yellow",
  "message": "eGPU gedrosselt — interne GPU zugewiesen"
}

// Response (kein VRAM frei)
{
  "granted": false,
  "gpu_device": null,
  "lease_id": null,
  "queue_position": 3,
  "estimated_wait_seconds": 120,
  "message": "Kein VRAM verfügbar — in Warteschlange"
}
```

**`POST /api/gpu/release`** — GPU-Kapazität freigeben (nach Task-Ende)

```json
{
  "lease_id": "lease-abc-123",
  "actual_vram_mb": 7500,
  "actual_duration_seconds": 240,
  "success": true
}
```

**`GET /api/gpu/recommend`** — GPU-Empfehlung ohne Reservierung (für Entscheidungshilfe)

```json
// Response
{
  "recommended_gpu": "0000:05:00.0",
  "nvidia_visible_devices": "GPU-bd7dd984-...",
  "warning_level": "green",
  "egpu_available": true,
  "available_vram_mb": {
    "egpu": 8500,
    "internal": 7200,
    "remote": null
  },
  "ollama_model": "qwen3:14b",
  "ollama_host": "http://localhost:11434"
}
```

### 5g.3 Python-Client-Bibliothek

Für die einfache Integration in die bestehenden Python-Projekte wird eine leichtgewichtige Client-Bibliothek bereitgestellt:

```python
# egpu_client.py — Drop-in für audit_designer und flowinvoice
# Kopiert in: audit_designer/backend/app/utils/egpu_client.py
#              flowinvoice/backend/app/utils/egpu_client.py

import os
import httpx
from contextlib import contextmanager
from typing import Optional

EGPU_MANAGER_URL = os.getenv("EGPU_MANAGER_URL", "http://127.0.0.1:7842")
EGPU_MANAGER_TIMEOUT = 3.0

class GpuLease:
    def __init__(self, lease_id: str, gpu_device: str, gpu_uuid: str, warning_level: str):
        self.lease_id = lease_id
        self.gpu_device = gpu_device
        self.gpu_uuid = gpu_uuid
        self.warning_level = warning_level
        self.nvidia_visible_devices = gpu_uuid

def acquire_gpu(
    pipeline: str,
    workload_type: str,
    vram_mb: int,
    duration_seconds: int = 300,
) -> Optional[GpuLease]:
    """GPU-Kapazität vom egpu-manager reservieren.

    Returns None wenn der egpu-manager nicht erreichbar ist —
    die Anwendung fällt dann auf ihre eigene GPU-Logik zurück.
    """
    try:
        resp = httpx.post(
            f"{EGPU_MANAGER_URL}/api/gpu/acquire",
            json={
                "pipeline": pipeline,
                "workload_type": workload_type,
                "vram_mb": vram_mb,
                "duration_estimate_seconds": duration_seconds,
            },
            timeout=EGPU_MANAGER_TIMEOUT,
        )
        data = resp.json()
        if data.get("granted"):
            return GpuLease(
                lease_id=data["lease_id"],
                gpu_device=data["gpu_device"],
                gpu_uuid=data.get("gpu_uuid", ""),
                warning_level=data.get("warning_level", "unknown"),
            )
        return None  # Nicht gewährt — Warteschlange oder kein VRAM
    except Exception:
        return None  # egpu-manager nicht erreichbar — Fallback

def release_gpu(lease: GpuLease, actual_vram_mb: int = 0, success: bool = True):
    """GPU-Kapazität freigeben."""
    try:
        httpx.post(
            f"{EGPU_MANAGER_URL}/api/gpu/release",
            json={
                "lease_id": lease.lease_id,
                "actual_vram_mb": actual_vram_mb,
                "success": success,
            },
            timeout=EGPU_MANAGER_TIMEOUT,
        )
    except Exception:
        pass  # Fire-and-forget

def get_recommended_gpu() -> dict:
    """GPU-Empfehlung holen (ohne Reservierung)."""
    try:
        resp = httpx.get(
            f"{EGPU_MANAGER_URL}/api/gpu/recommend",
            timeout=EGPU_MANAGER_TIMEOUT,
        )
        return resp.json()
    except Exception:
        return {"recommended_gpu": None, "egpu_available": False}

def get_ollama_host() -> str:
    """Aktuell empfohlenen Ollama-Host holen.

    Gibt den lokalen Ollama oder den Remote-Ollama zurück,
    je nach eGPU-Verfügbarkeit und Warnstufe.
    """
    info = get_recommended_gpu()
    return info.get("ollama_host", "http://localhost:11434")

@contextmanager
def gpu_context(pipeline: str, workload_type: str, vram_mb: int):
    """Context-Manager für GPU-Tasks.

    Verwendung:
        with gpu_context("audit_designer_celery_worker", "ocr", 8192) as gpu:
            if gpu:
                os.environ["NVIDIA_VISIBLE_DEVICES"] = gpu.nvidia_visible_devices
            # ... GPU-Task ausführen ...
    """
    lease = acquire_gpu(pipeline, workload_type, vram_mb)
    try:
        yield lease
    finally:
        if lease:
            release_gpu(lease)
```

### 5g.4 Integration in audit_designer Celery-Worker

```python
# audit_designer/backend/app/modules/vp_ai/tasks/embedding_tasks.py
from app.utils.egpu_client import gpu_context, get_ollama_host

@celery_app.task
def process_embedding_batch(batch_id: str):
    with gpu_context("audit_designer_celery_worker", "embeddings", 2048) as gpu:
        if gpu:
            device = "cuda"
            os.environ["NVIDIA_VISIBLE_DEVICES"] = gpu.nvidia_visible_devices
        else:
            device = "cpu"  # egpu-manager nicht erreichbar → CPU-Fallback

        model = SentenceTransformer("paraphrase-multilingual-mpnet-base-v2", device=device)
        # ... Embeddings berechnen ...

@celery_app.task
def run_ocr(document_id: str):
    with gpu_context("audit_designer_celery_worker", "ocr", 8192) as gpu:
        if gpu:
            os.environ["NVIDIA_VISIBLE_DEVICES"] = gpu.nvidia_visible_devices
            use_gpu = True
        else:
            use_gpu = False

        reader = easyocr.Reader(["de", "en"], gpu=use_gpu)
        # ... OCR ausführen ...
```

### 5g.5 Integration in audit_designer Backend (Ollama-Routing)

```python
# audit_designer/backend/app/modules/vp_ai/services/llm_service.py
from app.utils.egpu_client import get_ollama_host

class LLMService:
    def get_ollama_client(self):
        # Statt hardcoded localhost:11434:
        host = get_ollama_host()
        return OllamaClient(host=host)
```

### 5g.6 Graceful Degradation

**Wenn der egpu-manager nicht läuft**, funktionieren alle Anwendungen trotzdem:
- `acquire_gpu()` gibt `None` zurück → Anwendung nutzt ihre eigene GPU-Logik (CUDA_VISIBLE_DEVICES aus .env)
- `get_ollama_host()` gibt `http://localhost:11434` zurück → wie bisher
- `release_gpu()` scheitert leise → kein Impact

Das bedeutet: Die Integration ist **opt-in und nicht-blockierend**. Die Anwendungen funktionieren identisch ob der egpu-manager läuft oder nicht. Der egpu-manager verbessert die GPU-Nutzung, ist aber keine Abhängigkeit.

### 5g.7 SSE-Integration für Echtzeit-Reaktion

Anwendungen die auf Warnstufen-Änderungen reagieren wollen, können den SSE-Stream abonnieren:

```python
# Für lang laufende Worker die auf GPU-Warnungen reagieren sollen
import sseclient

def listen_for_gpu_warnings(callback):
    """Hört auf GPU-Warnungen und ruft callback auf."""
    try:
        response = httpx.stream("GET", f"{EGPU_MANAGER_URL}/api/events/stream")
        client = sseclient.SSEClient(response)
        for event in client.events():
            if event.event == "warning_level":
                data = json.loads(event.data)
                callback(data)
    except Exception:
        pass  # egpu-manager nicht erreichbar
```

---

## 6. Komponente 3 — Weboberfläche

Die Weboberfläche ist unter `http://localhost:7842` erreichbar (lokaler Listener, siehe 3a). Keine Authentifizierung für lokalen Zugriff.

### 6.1 Zustandsmodell der UI

Die Weboberfläche trennt **Verbindungszustand** und **Betriebszustand** strikt voneinander. So wird verhindert dass ein technischer Verbindungsfehler mit einer Scheduler- oder Recovery-Entscheidung verwechselt wird.

**Verbindungszustand:**

| UI-Zustand | Anzeige | Auslöser |
|---|---|---|
| `connected` | Normalbetrieb, Live-Daten | SSE-Stream aktiv |
| `loading` | Spinner, "Verbinde..." | Initialer Seitenaufruf |
| `reconnecting` | Gelbe Leiste "Verbindung unterbrochen, versuche erneut..." | SSE-Stream abgebrochen |
| `stale` | Gelbe Leiste "Daten veraltet (seit X Sekunden)" | Kein SSE-Event seit 30s |
| `command_pending` | Button ausgegraut + Spinner + "wird ausgeführt..." | Nach Klick auf Aktion, bis Antwort |
| `error` | Rote Leiste "Verbindung fehlgeschlagen" + Retry-Button | 5 Reconnect-Versuche fehlgeschlagen |
| `daemon_offline` | Graue Leiste "GPU-Manager nicht gestartet" | Initial kein Daemon erreichbar |

**SSE-Reconnect-Strategie:** Exponential Backoff (1s, 2s, 4s, 8s, 16s), maximal 5 Versuche. Danach Wechsel in `error`-Zustand mit manuellem Retry-Button. Bei jedem Reconnect wird `GET /api/status` aufgerufen um den vollständigen State zu synchronisieren.

**Betriebszustand (aus `/api/status`):**

| System-Zustand | Anzeige | Auslöser |
|---|---|---|
| `normal` | Grüne Betriebsleiste, eGPU nimmt neue Tasks an | Warnstufe Grün, kein Recovery, keine Drosselung |
| `throttled` | Gelbe Betriebsleiste "eGPU gedrosselt" | Warnstufe Gelb oder harte Scheduler-Limits |
| `draining` | Blaue oder neutrale Betriebsleiste "Deaktivierung geplant" | Nutzer hat eGPU-Deaktivierung angefordert; laufende Tasks drainen aus |
| `disabled` | Graue Betriebsleiste "eGPU deaktiviert" | Thunderbolt-Deauthorization erfolgreich, eGPU absichtlich offline |
| `recovery_active` | Orange Betriebsleiste mit Stufe und Timer | Recovery-State-Machine aktiv |
| `degraded` | Orange oder rote Betriebsleiste mit Primärgrund | Degraded Mode, Docker offline, AER nicht verfügbar o.ä. |
| `remote_limited` | Zusatzbadge "Remote eingeschränkt" | Remote verfügbar, aber für bestimmte Workloads wegen Latenz blockiert |

### 6.2 Bereiche

Oberhalb aller Bereiche liegt eine **persistente Betriebsleiste**. Sie zeigt in einer Zeile:
- Warnstufe
- Recovery-Stufe oder "kein Recovery"
- `eGPU offen/gedrosselt/draining/deaktiviert/gesperrt`
- Anzahl wartender Tasks
- Remote-Status (`verfügbar`, `zu hohe Latenz`, `offline`)
- Docker-Status
- "Daten veraltet" falls SSE stale ist

Die Oberfläche besteht darunter aus drei Bereichen:

**Bereich 1 — GPU-Status:** Zeigt für alle GPUs (lokal + Remote): Name, PCIe-Adresse (bzw. "Remote" mit Netzwerk-Icon), Temperatur, Auslastung (GPU und VRAM als Fortschrittsbalken), Leistungsaufnahme, Power-State, Thunderbolt-Status (nur eGPU), AER-Fehlerzähler (nur eGPU), Warnstufe mit farbiger Kennzeichnung. Zusätzlich erhält jede GPU-Karte Mini-Verlaufskurven für die letzten 5 bis 15 Minuten: VRAM, Compute, PCIe TX/RX, AER-Delta und Link-Breite. Remote-GPUs werden nicht nur farblich, sondern auch mit Netzwerk-Icon, Label und eigenem Badge unterschieden.

Auf der eGPU-Karte gibt es eine explizite Steuerschaltfläche:
- Wenn die eGPU aktiv ist: **`eGPU nach aktuellem Task deaktivieren`**
- Wenn kein eGPU-Task läuft: **`eGPU jetzt deaktivieren`**
- Während `draining`: Button ist durch Statusanzeige ersetzt: "Deaktivierung geplant — wartet auf laufenden Task"
- Im deaktivierten Zustand: **`eGPU aktivieren`**

**Bereich 2 — Pipeline-Widget:** Karten-Layout, jede Karte enthält:
- Projektnamen und Container-Namen
- Aktuell zugewiesene GPU mit Farbkennzeichnung (blau: RTX 5070 Ti, grün: RTX 5060 Ti, lila: Remote, grau: inaktiv)
- Workload-Typ (OCR / Embeddings / LLM / inaktiv)
- VRAM-Nutzung in MB
- Prioritätsstufe als Badge (1–5), per Dropdown änderbar
- Schaltfläche für manuelle GPU-Zuweisung
- **Entscheidungsgrund** als Pflichtfeld: z.B. "manuell fixiert", "Fallback wegen Gelb", "wartet auf VRAM", "Remote-Latenz 85ms > 50ms"
- Queue-Position und aktive Blocker falls die Pipeline nicht läuft
- Herkunft der Zuweisung: `auto`, `manual`, `fallback`, `remote`
- Falls die eGPU deaktiviert werden soll: Hinweis `drain_pending` mit Text wie "läuft noch auf eGPU, Deaktivierung danach"
- Karten nach Priorität sortiert; aktive Tasks nur dann animiert wenn `prefers-reduced-motion` dies erlaubt
- VRAM-Gesamtbalken am oberen Rand (gestapelt, farblich je Pipeline, alle GPUs)

**Bereich 3 — Recovery-, Ereignis- und Audit-Log:** Drei Tabs:
- **Recovery:** Sichtbare State-Machine mit aktueller Stufe, Startzeit, letztem Fehler, nächstem Retry, betroffenen Pipelines und bereits ausgeführten Aktionen
- **Ereignislog:** Letzte 100 Ereignisse (Monitoring, Warnstufe, Recovery, Workload-Wechsel). Filterbar nach Typ.
- **Audit-Log:** Alle manuellen Aktionen mit Zeitstempel und Ursprung. Das Audit-Log ist **nicht löschbar**, aber filterbar und durchsuchbar (nach Aktionstyp, Ursprung, Container, Zeitfenster) damit es operativ nutzbar bleibt.

Recovery-Aktionen (PCIe-Reset, Thunderbolt-Reconnect) als Schaltflächen mit Guardrails (siehe 6.4).

Das HTML ist vollständig eingebettet — keine externen CSS-Frameworks, keine CDN-Abhängigkeiten.

### 6.3 Responsive Layout

Die drei Bereiche werden ab Viewport-Breite unter 1024px vertikal gestapelt. Pipeline-Widget ist die primäre Ansicht auf kleinen Bildschirmen — GPU-Status und Logs per Tab-Leiste erreichbar.

Mindest-Viewport: 360px. Touch-Targets mindestens 44×44px (WCAG 2.5.5).

### 6.4 Guardrails für Recovery-Aktionen

Recovery-Buttons sind nicht einfach anklickbar. Jede Recovery-Aktion erfordert:

1. **Dry-Run-/Impact-Vorschau:** Vor jeder schreibenden Aktion ruft die UI denselben API-Endpunkt mit `?dry_run=true` auf. Die Vorschau zeigt erwarteten Effekt: Neustart ja/nein, geschätzter freier VRAM danach, verdrängte Pipelines, Queue-Effekt, ob Ollama entladen wird.
2. **2-Step-Confirm:** Erst nach erfolgreicher Vorschau erscheint der Bestätigungsdialog mit Auswirkungsbeschreibung ("PCIe-Reset wird die eGPU für ca. 5 Sekunden offline nehmen. Alle CUDA-Container auf der eGPU werden migriert.").
3. **Fallback-Hinweis wenn keine Prognose möglich ist:** Falls der Dry-Run keine belastbare Vorschau liefern kann, zeigt der Dialog explizit "Auswirkung nicht zuverlässig bestimmbar". Die Aktion bleibt möglich, aber nur nach zusätzlicher Bestätigung.
4. **Cooldown:** Nach einer Recovery-Aktion sind Recovery-Buttons für `reset_cooldown_seconds` (Standard: 30) deaktiviert (ausgegraut mit Countdown-Anzeige).
5. **Disable bei laufender Recovery:** Wenn eine Recovery bereits läuft, sind alle Recovery-Buttons deaktiviert mit dem Hinweis "Recovery läuft (Stufe X)...".
6. **Audit-Log-Eintrag:** Jede manuelle Recovery-Aktion wird im Audit-Log protokolliert.

Für die Aktion **`eGPU nach aktuellem Task deaktivieren`** gelten zusätzlich:

1. Die Vorschau nennt explizit wie viele eGPU-Tasks aktuell noch laufen und ob die Deaktivierung **sofort** oder **nach Task-Ende** erfolgt.
2. Der Bestätigungsdialog verwendet genau diese Formulierung, z.B. "Die eGPU wird nach Beendigung des aktuell laufenden LLM-Tasks deaktiviert."
3. Nach Bestätigung zeigt die UI keinen statischen Erfolg, sondern den Zwischenzustand `draining` mit laufender Aktualisierung.
4. Solange `draining` aktiv ist, sind manuelle Zuweisungen auf die eGPU gesperrt.

### 6.5 Accessibility

Die Weboberfläche erfüllt WCAG 2.1 Level AA:

- **Nicht nur Farben:** Jede Warnstufe hat zusätzlich ein Icon und einen Textlabel (z.B. "⚠ Gelb: Erhöhte Fehlerrate" statt nur gelber Hintergrund).
- **Nicht nur Animationen:** Aktive Tasks, Recovery und Queue-Zustände werden mit Text, Icon und Badge vermittelt; Animation ist rein ergänzend.
- **GPU-Typen visuell redundant kodiert:** Intern, eGPU und Remote unterscheiden sich über Badge, Icon und Text, nicht nur über Farbe.
- **Tastatur-Navigation:** Alle interaktiven Elemente per Tab erreichbar. Fokus-Ring sichtbar (3px Outline). Escape schließt Dialoge.
- **Fokusführung:** Nach Aktionen (z.B. Priorität ändern) springt der Fokus auf die Bestätigungsmeldung.
- **Kontraste:** Mindestens 4.5:1 für Text, 3:1 für grafische Elemente. Dark-Mode-Variante mit geprüften Kontrasten.
- **Screen-Reader:** ARIA-Labels für alle Fortschrittsbalken, Live-Regions für SSE-Updates (`aria-live="polite"` für Status-Updates, `aria-live="assertive"` für Warnungen).
- **Reduzierte Bewegung:** Bei `prefers-reduced-motion: reduce` sind pulsierende Rahmen, Live-Highlighting und Auto-Scroll im Log deaktiviert.

### 6.6 Skalierung bei vielen Pipelines

Bis 8 Pipelines:
- Karten-Layout ist Standard

Ab 9 Pipelines:
- **Filterleiste:** Filtern nach Projekt, GPU-Zuweisung, Priorität, Status (aktiv/inaktiv).
- **Suchfeld:** Freitext-Suche über Projektname und Container-Name.
- **Gruppierung:** Pipelines nach Projekt gruppierbar (Toggle).

Ab 13 Pipelines:
- **Kompaktmodus als Standard:** Tabellenartige Zeilen mit einblendbarer Detailansicht; Karten-Layout bleibt optional.
- **Entscheidungs-Spalte:** Eigene Spalte für `decision_reason`, Queue-Position und Blocker.

Ab 20 Pipelines:
- **Virtualisierung:** Nur sichtbare Zeilen oder Karten werden gerendert.

---

## 6a. Pipeline-Widget im GTK4-Desktop-Widget

Kompakte Pipeline-Übersicht im GTK4-Popup: Projektkürzel, Workload-Icon, GPU-Badge, VRAM-Balken und kurzer Entscheidungsgrund. Am oberen Rand steht eine reduzierte Betriebsleiste mit Warnstufe, Queue-Länge und Recovery-Stufe.

Das GTK4-Popup ist bewusst **read-only**. Es zeigt die drei dringendsten Pipelines (aktiv oder wartend) plus Link "Weboberfläche öffnen". Destruktive oder schreibende Aktionen werden hier nicht angeboten.

---

## 7. Komponente 4 — GTK4-Desktop-Widget

Systemtray-Icon mit Farbstatus: grün/gelb/orange/rot. Einfacher Klick: Popup mit GPU-Kennzahlen. Doppelklick: Weboberfläche im Browser.

Kommuniziert ausschließlich über Unix-Socket. Kein Hardware-Zugriff, kein Root-Bedarf.

### 7a. GTK4 unter Wayland/GNOME

GNOME/Wayland hat keinen nativen Systemtray. Widget nutzt `libayatana-appindicator` (`StatusNotifierItem`-D-Bus-Protokoll). Benötigt GNOME-Extension `AppIndicator and KStatusNotifierItem Support`.

Install-Skript prüft und warnt:
```bash
gnome-extensions list | grep -q "appindicatorsupport" || \
  echo "WARNUNG: GNOME AppIndicator Extension fehlt."
```

**Fallback ohne Extension:** Widget läuft als Hintergrunddienst ohne Tray-Icon. Weboberfläche via `egpu-manager open` erreichbar.

---

## 8. Kernel-Absicherung (manuell auszuführen)

**Dies ist die wichtigste Verteidigungslinie gegen eGPU-Freezes.** Der Monitoring-Daemon (Abschnitt 4) kann einen Freeze nur erkennen und den Schaden begrenzen. Die Kernel-Absicherung kann den Freeze tatsächlich verhindern.

Der egpu-manager generiert ein Skript `kernel-tuning.sh` das folgende Änderungen vornimmt. Das Skript soll vor der Ausführung manuell geprüft werden. Jede Änderung ist einzeln aktivierbar/deaktivierbar.

### 8.1 GRUB-Parameter

```bash
# /etc/default/grub — GRUB_CMDLINE_LINUX_DEFAULT erweitern:
pcie_aspm=off                    # PCIe Active State Power Management deaktivieren
pcie_acs_override=downstream,multifunction  # bereits gesetzt, beibehalten
```

`pcie_aspm=off` ist die wichtigste Einzelmaßnahme. ASPM versetzt PCIe-Links in Low-Power-States aus denen das Aufwachen bei Thunderbolt-Verbindungen bis zu 100ms dauern kann — lang genug für einen CmpltTO. Deaktivierung erhöht den Stromverbrauch um ca. 2–5W, eliminiert aber eine Hauptursache für Timeouts.

### 8.2 PCIe Completion Timeout — Vollständige Konfiguration

Der PCIe Completion Timeout ist in Register `0xd4` (Device Control 2) des Root-Port `0000:00:07.0` konfiguriert. Die PCIe-Spezifikation definiert folgende Ranges:

| Wert | Range | Timeout |
|---|---|---|
| `0x0` | Default | 50µs – 50ms (plattformabhängig) |
| `0x1` | Range A | 50µs – 100µs |
| `0x2` | Range A | 1ms – 10ms |
| `0x5` | Range B | 16ms – 55ms |
| `0x6` | Range C | 65ms – 210ms |
| `0x9` | Range C | 260ms – 900ms |
| `0xA` | Range D | 1s – 3.5s |
| `0xE` | Range D | 4s – 13s (absolutes Maximum) |

**Aktuelle Empfehlung:** `0x6` (65–210ms) ist ein guter Kompromiss. Für Thunderbolt-eGPU-Setups mit bekannter Instabilität kann `0xA` (1–3.5s) gewählt werden. `0xE` (4–13s) ist nur empfohlen wenn häufige CmpltTOs auftreten und die höheren Timeouts das System nicht anderweitig beeinträchtigen (z.B. blockierte PCIe-Transaktionen die den Treiber 13 Sekunden warten lassen).

**WICHTIG:** Vor dem Setzen den aktuellen Wert auslesen und dokumentieren:
```bash
# Aktuellen Wert lesen:
sudo setpci -s 0000:00:07.0 0xd4.w
# Ausgabe z.B. "0000" → Default-Range

# Neuen Wert setzen (65–210ms):
sudo setpci -s 0000:00:07.0 0xd4.w=0x6
```

**Reboot-persistent** via systemd-Einheit:

```ini
# /etc/systemd/system/egpu-pcie-tuning.service
[Unit]
Description=eGPU PCIe Completion Timeout Tuning
After=multi-user.target
Before=egpu-manager.service

[Service]
Type=oneshot
ExecStart=/usr/sbin/setpci -s 0000:00:07.0 0xd4.w=0x6
RemainAfterExit=yes

[Install]
WantedBy=multi-user.target
```

### 8.3 NVIDIA-Treiberparameter

Der NVIDIA-Treiber hat undokumentierte Registry-Parameter die die PCIe-Fehlertoleranz beeinflussen. Diese werden über `/etc/modprobe.d/nvidia-egpu.conf` konfiguriert:

```bash
# /etc/modprobe.d/nvidia-egpu.conf
options nvidia NVreg_EnablePCIeRelaxedOrderingMode=1
```

`NVreg_EnablePCIeRelaxedOrderingMode=1` erlaubt dem Treiber PCIe-Transaktionen in beliebiger Reihenfolge abzuschließen. Das reduziert die Wahrscheinlichkeit von CmpltTOs weil der Treiber nicht auf die Completion einer spezifischen Transaktion warten muss bevor er die nächste abschickt.

**WARNUNG:** Diese Parameter sind undokumentiert und können sich zwischen Treiberversionen ändern. Das Skript prüft die installierte Treiberversion und gibt eine Warnung aus wenn sie von der getesteten Version abweicht.

**Getestete Treiberversion:** 576.02 (CUDA 13.x). Bei anderen Versionen: Parameter manuell testen.

### 8.4 AER-Masking (optional, riskant)

Es ist möglich den CmpltTO-Fehler auf Kernel-Ebene zu maskieren damit der Treiber den Fehler nicht sieht und nicht einfriert:

```bash
# CmpltTO-Bit (Bit 14) in der AER Uncorrectable Error Mask setzen:
sudo setpci -s 0000:05:00.0 ECAP_AER+0x08.L=0x00004000
```

**Risiko:** Das maskiert den Fehler, behebt ihn aber nicht. Die PCIe-Transaktion die den Timeout ausgelöst hat ist trotzdem fehlgeschlagen — der Treiber bekommt nur keinen Fehler gemeldet. Das kann zu stillen Datenkorruptionen im VRAM führen (z.B. beschädigte Modellgewichte bei LLM-Inferenz, fehlerhafte OCR-Ergebnisse).

**Empfehlung:** Nur als letztes Mittel wenn CmpltTOs häufig auftreten und alle anderen Maßnahmen (ASPM off, Timeout erhöhen, Bandbreitenmanagement) nicht ausreichen. Im `kernel-tuning.sh` ist diese Option standardmäßig auskommentiert mit deutlicher Warnung.

### 8.5 BIOS-Einstellungen (manuell)

Der ASUS NUC15JNLU7X4 hat BIOS-Einstellungen die die Thunderbolt-PCIe-Konfiguration beeinflussen. Diese können nicht per Skript geändert werden. Das `kernel-tuning.sh` gibt folgende Hinweise aus:

```
BIOS-Prüfung empfohlen (manuell im BIOS):
1. Thunderbolt Configuration → PCIe Tunnel: "x4" statt "Auto" (verhindert dynamisches Downgrade)
2. Thunderbolt Configuration → Security Level: "No Security" oder "User Authorization"
   (nicht "Secure Connect" — das fügt Latenz hinzu)
3. Advanced → PCI Subsystem Settings → Above 4G Decoding: "Enabled"
4. Advanced → PCI Subsystem Settings → Re-Size BAR Support: "Disabled"
   (ReBAR über Thunderbolt kann CmpltTOs auslösen)
```

### 8.6 sysctl-Parameter

```bash
# /etc/sysctl.d/99-egpu-manager.conf
vm.overcommit_memory=1           # bereits gesetzt
kernel.nmi_watchdog=0             # NMI-Interrupts bei hoher PCIe-Last reduzieren
```

### 8.7 Rollback

Das Skript erzeugt ein Rollback-Skript `kernel-tuning-rollback.sh` das:
1. Die systemd-Einheit `egpu-pcie-tuning.service` deaktiviert und entfernt.
2. Die GRUB-Parameter zurücksetzt (`pcie_aspm` entfernen).
3. Die NVIDIA-Treiberparameter entfernt (`/etc/modprobe.d/nvidia-egpu.conf`).
4. Die sysctl-Datei entfernt.
5. AER-Masking zurücksetzt (falls aktiviert).
6. `update-grub` und `update-initramfs -u` ausführt.

Falls das System nach dem Kernel-Tuning nicht mehr korrekt startet, kann der Nutzer über den Recovery-Modus (GRUB → Advanced → Recovery) das Rollback-Skript ausführen.

---

## 8a. Formale REST-API-Definition

Alle Endpunkte unter `http://localhost:7842/api/`. Antworten: JSON. Fehler: `{"error": "...", "code": "ERROR_CODE"}`.

### GPU-Status

| Endpunkt | Methode | Beschreibung |
|---|---|---|
| `/api/status` | GET | Vollständiger Status aller GPUs, Daemon-Info, Warnstufe |
| `/api/status/gpu/{pci_address}` | GET | Status einer GPU (per PCI-Bus-ID) |

Response-Beispiel für `/api/status`:
```json
{
  "daemon": {
    "version": "1.0.0",
    "uptime_seconds": 3600,
    "warning_level": "green",
    "egpu_admission_state": "open",
    "pending_egpu_disable": false,
    "scheduler_queue_length": 0,
    "degraded_reason": null,
    "recovery_active": false,
    "recovery_stage": null,
    "mode": "normal",
    "config_schema_version": 1
  },
  "gpus": [
    {
      "pci_address": "0000:02:00.0",
      "nvidia_index": 0,
      "name": "NVIDIA GeForce RTX 5060 Ti",
      "type": "internal",
      "temperature_c": 42,
      "utilization_gpu_percent": 15,
      "memory_used_mb": 2450,
      "memory_total_mb": 8151,
      "power_draw_w": 45.0,
      "pstate": "P0",
      "status": "online"
    },
    {
      "pci_address": "0000:05:00.0",
      "nvidia_index": 1,
      "name": "NVIDIA GeForce RTX 5070 Ti",
      "type": "egpu",
      "temperature_c": 55,
      "utilization_gpu_percent": 80,
      "memory_used_mb": 10597,
      "memory_total_mb": 16303,
      "power_draw_w": 180.0,
      "pstate": "P0",
      "thunderbolt_status": "authorized",
      "disable_requested": false,
      "aer_nonfatal_count": 0,
      "pcie_link_speed": "2.5 GT/s",
      "pcie_link_width": 4,
      "pcie_tx_kbps": 450000,
      "pcie_rx_kbps": 120000,
      "bandwidth_utilization_percent": 45,
      "cuda_watchdog_status": "ok",
      "status": "online"
    }
  ],
  "remote_gpus": [
    {
      "name": "remote-5060",
      "host": "192.168.1.100",
      "gpu_name": "NVIDIA GeForce RTX 5060",
      "vram_mb": 16384,
      "status": "available",
      "latency_ms": 2,
      "last_seen": "2026-03-14T10:30:00Z"
    }
  ]
}
```

### Pipelines

| Endpunkt | Methode | Beschreibung |
|---|---|---|
| `/api/pipelines` | GET | Alle Pipelines mit Laufzeit-Status und VRAM-Summary |
| `/api/pipelines/{container}` | GET | Einzelne Pipeline |
| `/api/pipelines/{container}/priority` | PUT | Priorität ändern. Body: `{"priority": 2}` |
| `/api/pipelines/{container}/assign` | POST | GPU-Zuweisung ändern. Body: `{"gpu_device": "0000:02:00.0"}` |
| `/api/pipelines/{container}/workload-update` | POST | Task-Typ melden (Celery-Webhook, siehe 5.6). Body: `{"workload_type": "ocr", "vram_estimate_mb": 8192}` |
| `/api/ollama/status` | GET | Ollama-Status: geladene Modelle, VRAM-Verbrauch, GPU-Zuweisung |
| `/api/ollama/unload` | POST | Modell aus VRAM entladen. Body: `{"model": "llama3:70b"}` |

Antworten von `/api/pipelines` und `/api/pipelines/{container}` enthalten zusätzlich:
- `decision_reason`: maschinenlesbarer und lesbarer Hauptgrund für die aktuelle Zuweisung
- `assignment_source`: `auto`, `manual`, `fallback`, `remote`
- `queue_position`: Position in der Warteschlange oder `null`
- `blocked_by`: Liste aktueller Blocker (VRAM, Compute, Warnstufe, Remote-Latenz, Recovery)
- `last_transition_at`: Zeitstempel des letzten Zustandswechsels
- `drain_pending`: `true`, wenn die Pipeline aktuell noch auf der eGPU läuft, aber die eGPU nach Abschluss deaktiviert werden soll

### eGPU-Steuerung

| Endpunkt | Methode | Beschreibung |
|---|---|---|
| `/api/egpu/state` | GET | Aktueller Deaktivierungs-/Aktivierungszustand der eGPU |
| `/api/egpu/deactivate` | POST | Graceful Deaktivierung anfordern. Body: `{"mode": "drain", "confirm": true}` |
| `/api/egpu/deactivate/cancel` | POST | Geplante Deaktivierung abbrechen solange `draining` noch läuft |
| `/api/egpu/activate` | POST | eGPU wieder aktivieren (Thunderbolt-Reauthorization) |

### Recovery

| Endpunkt | Methode | Beschreibung |
|---|---|---|
| `/api/recovery/status` | GET | Aktueller Recovery-Zustand |
| `/api/recovery/reset` | POST | Manueller PCIe-Reset. Body: `{"confirm": true}` |
| `/api/recovery/thunderbolt-reconnect` | POST | Thunderbolt-Reauth. Body: `{"confirm": true}` |

Alle schreibenden Endpunkte der lokalen API unterstützen `?dry_run=true`. In diesem Modus wird **keine** Änderung ausgeführt; stattdessen liefert der Endpunkt eine Auswirkungsprognose für die UI:

```json
{
  "will_restart_service": true,
  "projected_gpu_device": "0000:02:00.0",
  "projected_free_vram_mb": 7800,
  "displaced_pipelines": ["audit_designer_jupyter"],
  "queue_delta": 1,
  "ollama_action": "unload_model",
  "confidence": "high",
  "message": "Jupyter wird auf die interne GPU verdrängt."
}
```

Für `/api/egpu/deactivate?dry_run=true` enthält die Antwort zusätzlich:

```json
{
  "mode": "drain",
  "running_egpu_tasks": 1,
  "running_task_labels": ["audit_designer_celery_worker: OCR"],
  "will_deactivate_after_running_tasks": true,
  "projected_state": "draining",
  "message": "Die eGPU wird nach Beendigung des aktuell laufenden OCR-Tasks deaktiviert."
}
```

### Ereignisse

| Endpunkt | Methode | Beschreibung |
|---|---|---|
| `/api/events` | GET | Letzte Events. Query: `?limit=100&type=warning,recovery&since=...` |
| `/api/events/stream` | GET | SSE-Stream. Event-Typen: `gpu_status`, `warning_level`, `recovery_stage`, `pipeline_change`, `remote_gpu_status`, `config_reload`, `audit_action` |
| `/api/audit-log` | GET | Alle manuellen Aktionen (nicht löschbar). Query: `?limit=100&since=...` |

### Konfiguration, Wizard, Remote

| Endpunkt | Methode | Beschreibung |
|---|---|---|
| `/api/config` | GET | Aktuelle Konfiguration (ohne Secrets/Token) |
| `/api/config/reload` | POST | Hot-Reload |
| `/api/wizard/analyze` | POST | Projektverzeichnis analysieren (SSE-Response) |
| `/api/wizard/add` | POST | Pipeline hinzufügen |
| `/api/wizard/{project}` | DELETE | Pipeline entfernen |
| `/api/setup/generate` | POST | Windows-Setup-ZIP generieren |
| `/api/setup/status` | GET | Generierungs-Fortschritt |
| `/api/setup/instructions` | GET | Liefert den Offline-/USB-Installationsablauf für den Windows-Remote-Node |

### Remote-API (nur über Remote-Listener, Port 7843, Token-Auth)

| Endpunkt | Methode | Beschreibung |
|---|---|---|
| `/api/remote/register` | POST | Remote-Node registrieren |
| `/api/remote/unregister` | POST | Remote-Node abmelden |
| `/api/remote/heartbeat` | POST | Heartbeat mit GPU-Status |

---

## 8b. SQLite Retention-Policy

```toml
[database]
retention_days = 90
retention_check_interval_hours = 24
max_db_size_mb = 500
aggregate_after_days = 7
```

**Aggregation:** Monitoring-Events (5s-Intervall) werden nach 7 Tagen zu 5-Minuten-Durchschnitten aggregiert (Faktor 60 Reduktion). Recovery-, Warning- und Audit-Events bleiben als Einzelereignisse bis zur Retention-Grenze.

**Vacuum:** Nach jeder Bereinigung in einem Blocking-Task.

---

## 8c. Integrity-Checks für heruntergeladene Binaries

Siehe Abschnitt 5e.3 (Supply-Chain-Sicherheit). Alle Binaries im Setup-Paket sind versions-gepinnt mit SHA256-Prüfung. Bei Hash-Mismatch bricht die Generierung ab.

---

## 9. Vollständige Konfigurationsdatei

```toml
# /etc/egpu-manager/config.toml
# Schema-Version: 1 (siehe 9a für Migrations-Regeln)
schema_version = 1

[system]
log_level = "info"                     # trace, debug, info, warn, error

[database]
db_path = "/var/lib/egpu-manager/events.db"
retention_days = 90
retention_check_interval_hours = 24
max_db_size_mb = 500
aggregate_after_days = 7

[gpu]
egpu_pci_address = "0000:05:00.0"
internal_pci_address = "0000:02:00.0"
poll_interval_seconds = 5
fast_poll_interval_seconds = 1
aer_warning_threshold = 3
aer_burst_threshold = 10
aer_window_seconds = 60
bandwidth_warning_percent = 70
bandwidth_hard_limit_percent = 85          # Ab hier: aktive Drosselung (nicht nur Warnung)
compute_warning_percent = 90
compute_soft_limit_percent = 70
nvidia_smi_timeout_seconds = 5
nvidia_smi_retry_interval_seconds = 15
nvidia_smi_max_consecutive_timeouts = 3
warning_cooldown_seconds = 120
display_vram_reserve_mb = 512              # Fallback wenn auto-detect fehlschlägt
link_health_check_interval_ms = 500
link_degradation_action = "throttle"       # "throttle", "migrate", "warn_only"
cuda_watchdog_enabled = true
cuda_watchdog_interval_ms = 500
cuda_watchdog_timeout_ms = 2000
cuda_watchdog_binary = "/usr/lib/egpu-manager/egpu-watchdog"
graceful_disable_check_interval_seconds = 2    # Prüft im Drain-Modus ob noch eGPU-Tasks aktiv sind

[thunderbolt]
device_uuid = "8ab48780-00c3-eba8-ffff-ffffffffffff"
device_path = "0-3"
authorized_policy = "iommu"

[docker]
socket = "/var/run/docker.sock"
api_timeout_seconds = 10
api_max_retries = 3
container_stop_timeout_seconds = 10
container_restart_timeout_seconds = 60

# Lokaler Listener: 127.0.0.1:7842 ist fest im Quellcode — NICHT konfigurierbar.
# Nur CORS-Origins sind konfigurierbar:
[local_api]
cors_origins = ["http://localhost:3002"]   # audit_designer Frontend

[remote]
enabled = false
bind = "0.0.0.0"
port = 7843
token_path = "/etc/egpu-manager/remote-token.secret"
tls = false
tls_cert = "/etc/egpu-manager/tls/server.crt"
tls_key = "/etc/egpu-manager/tls/server.key"
tls_ca = "/etc/egpu-manager/tls/ca.crt"
ollama_version_pin = "0.6.2"

[ollama]
enabled = true
host = "http://localhost:11434"
poll_interval_seconds = 5
gpu_device = "0000:05:00.0"
fallback_device = "0000:02:00.0"
fallback_method = "helper-service"         # "helper-service" oder "api" (Ollama ≥0.5)
helper_service = "egpu-ollama-fallback.service"  # systemd-Hilfsdienst für GPU-Wechsel
gpu_target_file = "/run/egpu-manager/ollama-gpu-target"  # Daemon schreibt, Hilfsdienst liest
priority = 1
max_vram_mb = 14000
auto_unload_idle_minutes = 10

[notifications]
ntfy_url = ""
ntfy_topic = ""
log_path = "/var/log/egpu-manager/events.log"

[recovery]
max_attempts = 4
reset_cooldown_seconds = 30
scheduling_lock_timeout_seconds = 5

[daemon]
shutdown_timeout_seconds = 15
degraded_mode_retry_seconds = 30

# --- Pipeline-Profile ---

[[pipeline]]
project = "audit_designer"
container = "audit_designer_celery_worker"
compose_file = "/home/janpow/Projekte/audit_designer/docker-compose.yml"
compose_service = "celery_worker"
workload_types = ["ocr", "embeddings", "llm"]
gpu_priority = 1
gpu_device = "0000:05:00.0"
cuda_fallback_device = "0000:02:00.0"
vram_estimate_mb = 8192
exclusive_gpu = false
restart_on_fallback = true
redis_containers = ["audit_designer_redis"]
depends_on = []
remote_capable = ["llm", "embeddings"]
cuda_only = ["ocr"]
quiesce_hooks = [
  { container = "audit_designer_redis", command = "redis-cli BGSAVE", timeout_seconds = 5 },
  { container = "audit_designer_db", command = "psql -U postgres -c 'CHECKPOINT'", timeout_seconds = 5 }
]

[[pipeline]]
project = "audit_designer"
container = "audit_designer_jupyter"
compose_file = "/home/janpow/Projekte/audit_designer/docker-compose.yml"
compose_service = "jupyter"
workload_types = ["interactive", "development"]
gpu_priority = 3
gpu_device = "0000:05:00.0"
cuda_fallback_device = "0000:02:00.0"
vram_estimate_mb = 4096
exclusive_gpu = false
restart_on_fallback = false
redis_containers = []
depends_on = []
remote_capable = ["interactive"]
cuda_only = []
quiesce_hooks = []

[[pipeline]]
project = "flowinvoice"
container = "flowinvoice_worker"
compose_file = "/home/janpow/Projekte/flowinvoice/docker-compose.yml"
compose_service = "worker"
workload_types = ["ocr", "llm"]
gpu_priority = 2
gpu_device = "0000:05:00.0"
cuda_fallback_device = "0000:02:00.0"
vram_estimate_mb = 6144
exclusive_gpu = false
restart_on_fallback = true
redis_containers = ["flowinvoice_redis"]
depends_on = []
remote_capable = ["llm"]
cuda_only = ["ocr"]
quiesce_hooks = [
  { container = "flowinvoice_redis", command = "redis-cli BGSAVE", timeout_seconds = 5 }
]

# --- Remote-GPU (optional, deaktiviert bis remote.enabled = true) ---

# [[remote_gpu]]
# name = "remote-5060"
# host = "192.168.1.XXX"
# port_ollama = 11434
# port_llama_cpp = 8080
# port_egpu_agent = 7843
# gpu_name = "NVIDIA GeForce RTX 5060"
# vram_mb = 16384
# availability = "on-demand"
# check_interval_seconds = 30
# connection_timeout_seconds = 5
# priority = 2
# auto_assign = false
```

---

## 9a. Transaktionale Konfigurationsupdates

### 9a.1 Schema-Versionierung

Die Konfigurationsdatei enthält ein Feld `schema_version` (Integer). Der Daemon prüft beim Start ob die Schema-Version der geladenen Konfiguration mit der erwarteten Version übereinstimmt. Bei Abweichung:

- **Ältere Version:** Automatische Migration. Jede Schema-Version hat eine Migrationsfunktion die die Konfiguration auf die nächste Version hebt. Die Originaldatei wird vorher als Backup gespeichert.
- **Neuere Version:** Fehler. Daemon startet nicht. Hinweis auf erforderliches Update.

### 9a.2 Atomare Schreibvorgänge

Jede Konfigurationsänderung (Wizard, Hot-Reload, Remote-Registrierung) folgt diesem Ablauf:

1. **Backup erstellen:** `config.toml` → `config.toml.bak.{timestamp}`.
2. **Temporäre Datei schreiben:** Neue Konfiguration in `config.toml.tmp` schreiben.
3. **Validierung:** Die temporäre Datei wird geladen und vollständig validiert (Schema, Werte, Referenzen).
4. **Atomarer Rename:** `rename("config.toml.tmp", "config.toml")` — atomar auf dem gleichen Filesystem.
5. **Daemon-Reload:** SIGHUP oder Unix-Socket-Kommando.

Falls die Validierung in Schritt 3 fehlschlägt, wird `config.toml.tmp` gelöscht und der Nutzer erhält eine Fehlermeldung mit Details. Die aktuelle Konfiguration bleibt unverändert.

### 9a.3 Locking

Bei gleichzeitigen Schreibversuchen (z.B. Wizard und Remote-Registrierung parallel) wird ein File-Lock auf `config.toml.lock` verwendet (Advisory Locking via `flock`). Timeout: 5 Sekunden. Bei Lock-Timeout: Fehlermeldung "Konfiguration wird gerade von einem anderen Prozess bearbeitet".

### 9a.4 Rollback

Falls der Daemon nach einem Config-Update nicht mehr startet, kann der Nutzer per CLI zurückrollen:

```bash
egpu-manager config rollback              # Neuestes Backup wiederherstellen
egpu-manager config list-backups          # Alle Backups anzeigen
egpu-manager config rollback 2026-03-14T10:30:00  # Bestimmtes Backup
```

Backups werden 30 Tage aufbewahrt, danach automatisch gelöscht.

---

## 10. Explizite Verbote für Claude Code

Claude Code darf bei der Entwicklung des egpu-manager unter keinen Umständen folgende Befehle ausführen: `apt`, `apt-get`, `dpkg`, `dkms`, `update-grub`, `update-initramfs`, `grub-install`, `modprobe`, `rmmod`, `systemctl enable` (für systemd-Services außerhalb des Projektverzeichnisses), `sysctl -w` (außer für vm.overcommit_memory).

Kernel-Parameter werden ausschließlich in generierten Skripten vorgeschlagen die manuell geprüft und ausgeführt werden. Der systemd-Service wird als Unit-Datei generiert aber nicht automatisch aktiviert — `systemctl enable egpu-manager` ist manuell auszuführen.

---

## 11. Entwicklungsreihenfolge mit Abnahmekriterien

Die Entwicklung soll in dieser Reihenfolge erfolgen damit zu jedem Zeitpunkt ein lauffähiger Zwischenstand vorhanden ist. Jede Phase hat explizite Abnahmekriterien die erfüllt sein müssen bevor die nächste Phase beginnt.

### Phase 0 — Pipeline-Analyse

**Aufgabe:** Claude Code analysiert alle Projektverzeichnisse gemäß 5a und erstellt `pipeline-profiles.toml`.

**Abnahmekriterien:**
- [ ] Alle Projekte (audit_designer, flowinvoice, hpp, Workshop) analysiert
- [ ] `pipeline-profiles.toml` enthält vollständige `[[pipeline]]`-Blöcke mit PCI-Bus-IDs
- [ ] `compose_file` und `compose_service` korrekt erfasst
- [ ] VRAM-Schätzungen mit Begründung dokumentiert
- [ ] Manuell geprüft und freigegeben

### Phase 1 — Cargo-Workspace und GPU-Abfrage

**Aufgabe:** Workspace mit Crates anlegen, Config einlesen, nvidia-smi parsen (mit Timeout), GPU-Status ausgeben. HAL-Traits definieren. Test-Infrastruktur aufsetzen. PCIe-Link-Health auslesen. CUDA-Watchdog-Binary bauen (falls CUDA-Toolkit vorhanden).

**Abnahmekriterien:**
- [ ] `cargo build --workspace` erfolgreich
- [ ] `cargo test --workspace --features mock-hardware` erfolgreich
- [ ] Konfigurationsdatei wird geladen und validiert (inklusive Schema-Version und neue Felder)
- [ ] nvidia-smi-Ausgabe wird korrekt geparst (PCI-Bus-ID als Identifier, `gpu_bus_id`)
- [ ] nvidia-smi dmon wird als persistenter Child-Prozess gestartet (pcie_tx/pcie_rx)
- [ ] PCIe-Link-Health wird aus sysfs gelesen (link_speed, link_width)
- [ ] Timeout bei nvidia-smi-Hänger funktioniert (Unit-Test mit Mock)
- [ ] GPU-Status wird auf der Konsole ausgegeben (Name, Temperatur, VRAM, PCI-Adresse, Link-Status, PCIe-Durchsatz)
- [ ] CUDA-Watchdog kompiliert und Heartbeat funktioniert (falls CUDA-Toolkit vorhanden)
- [ ] Ollama-API-Abfrage: `/api/ps` liefert geladene Modelle und VRAM
- [ ] Display-VRAM-Reservierung wird automatisch ermittelt
- [ ] `cargo clippy -- -D warnings` ohne Fehler

### Phase 2 — Monitoring und Warnstufen

**Aufgabe:** AER-Monitoring, kmsg-Streaming, PCIe-Link-Health-Monitoring, CUDA-Watchdog-Integration, Bandbreitenmessung, SQLite-Logging, proaktive Warnstufen mit Drosselung, VRAM-Scheduling mit Prioritäten und tatsächlichem Verbrauch.

**Abnahmekriterien:**
- [ ] AER-Fehlerzähler wird korrekt gelesen, Delta-Berechnung funktioniert
- [ ] AER Edge Cases getestet: Counter-Reset, Burst, Datei nicht lesbar (Unit-Tests)
- [ ] kmsg-Stream erkennt CmpltTO-Pattern (Unit-Test mit Mock-Stream)
- [ ] PCIe-Link-Degradation erkannt: link_width < max_link_width → Orange (Unit-Test)
- [ ] PCIe-Bandbreite gemessen: pcie_tx + pcie_rx vs. Maximum (Unit-Test)
- [ ] CUDA-Watchdog Timeout → Warnstufe Orange (Integration-Test mit Mock)
- [ ] SQLite-Logging funktioniert, Retention-Policy implementiert
- [ ] Warnstufen-Übergänge korrekt (Grün→Gelb→Orange→Rot, Hysterese, Cooldown mit schrittweiser Rückmigration)
- [ ] **Proaktive Drosselung bei Gelb:** Keine neuen Tasks auf eGPU, Prio 4–5 werden migriert (Integration-Test)
- [ ] VRAM-Scheduling: Tatsächlicher Verbrauch statt nur Schätzung, Compute-Auslastung als Limit
- [ ] VRAM-Scheduling: Prioritätsbasierte Zuweisung, Preemption, Warteschlange (Unit-Tests)
- [ ] Ollama-VRAM wird dynamisch in Scheduling einbezogen (aus `/api/ps`)
- [ ] Recovery-Scheduling-Interaktion: Warteschlange bei aktivem Recovery (Unit-Tests)

### Phase 3 — Recovery, Docker-Integration und Ollama-Steuerung

**Aufgabe:** Recovery State-Machine (inkl. Stufe 0 Quiesce), Docker-API, Container-Migration via docker compose, Ollama-Steuerung (Modell entladen, GPU-Wechsel), Celery Workload-Update-Webhook, Daemon-Lifecycle.

**Abnahmekriterien:**
- [ ] Recovery State-Machine durchläuft alle Stufen inkl. Stufe 0 Quiesce (Integration-Test mit Mocks)
- [ ] Recovery-State wird in SQLite persistiert und nach Neustart fortgesetzt
- [ ] Container-Migration nutzt docker compose Override (nicht docker restart)
- [ ] Quiesce-Hooks: Redis BGSAVE, PostgreSQL CHECKPOINT, Celery shutdown vor Reset (Integration-Test)
- [ ] Docker-API Fehlerbehandlung: Socket weg, Container nicht da, Timeout (Unit-Tests)
- [ ] Ollama-Steuerung: Modell entladen über API, GPU-Wechsel über systemd-Restart (Integration-Test)
- [ ] Ollama wird bei Warnstufe Orange als erstes von eGPU genommen
- [ ] Celery Workload-Update-Webhook empfängt Task-Typ und aktualisiert Scheduling (Unit-Test)
- [ ] Daemon Start-Sequenz: Override-Dateien erkennen, Fallback-Zustand rekonstruieren
- [ ] Daemon Graceful Shutdown: State persistiert, Container bleiben im aktuellen Zustand
- [ ] Daemon Crash-Recovery: Unsauberer Shutdown erkannt, Recovery fortgesetzt
- [ ] Benutzerinitiierte eGPU-Deaktivierung: `draining` blockiert neue eGPU-Tasks und deaktiviert die eGPU nach Ende laufender Tasks

### Phase 4 — Webserver und UI

**Aufgabe:** Axum mit SSE, REST-API, HTML-UI, responsives Layout, CORS.

**Abnahmekriterien:**
- [ ] Alle API-Endpunkte (8a) implementiert und per `axum::test` getestet
- [ ] SSE-Stream liefert Events, Reconnect-Strategie implementiert
- [ ] UI-Zustandsmodell (6.1): Verbindungszustände **und** Betriebszustände implementiert
- [ ] Persistente Betriebsleiste zeigt Warnstufe, Recovery, eGPU-Zulassung, Queue und Remote-Status
- [ ] Accessibility: Tastatur-Navigation, ARIA-Labels, Kontraste, Icons+Text für Warnstufen
- [ ] Responsive: Desktop und Mobile, 360px Minimum
- [ ] CORS konfigurierbar, `http://localhost:3002` funktioniert
- [ ] HTML eingebettet, keine externen Abhängigkeiten
- [ ] eGPU-Karte zeigt `deaktivieren`/`aktivieren`-Steuerung mit klarer Drain-Beschriftung

### Phase 4a — Pipeline-Widget

**Aufgabe:** Karten-Layout, VRAM-Balken, Prioritäts-Dropdown, GPU-Zuweisung, Echtzeit-SSE.

**Abnahmekriterien:**
- [ ] Alle Pipelines als Karten dargestellt, nach Priorität sortiert
- [ ] VRAM-Gesamtbalken für alle GPUs (gestapelt, farblich)
- [ ] Priorität per Dropdown änderbar, Änderung sofort wirksam (API + SSE)
- [ ] Manuelle GPU-Zuweisung funktioniert
- [ ] Jede Pipeline zeigt `decision_reason`, Queue-Position und aktuelle Blocker
- [ ] Aktive Tasks: Status sichtbar auch ohne Animation; `prefers-reduced-motion` berücksichtigt
- [ ] Guardrails für Recovery-Buttons und andere schreibende Aktionen (Dry-Run, 2-Step, Cooldown, Disable)
- [ ] Recovery-State-Machine sichtbar im UI
- [ ] Audit-Log für alle manuellen Aktionen, filterbar und durchsuchbar
- [ ] Bei geplanter eGPU-Deaktivierung zeigt die betroffene Pipeline `drain_pending`

### Phase 4b — Projekt-Wizard

**Aufgabe:** Web-Wizard und CLI-Wizard für Pipeline-Verwaltung.

**Abnahmekriterien:**
- [ ] Verzeichnis-Browser im Web-UI
- [ ] Automatische Erkennung mit inkrementellem Fortschritt (SSE)
- [ ] Editierbares Bestätigungsformular mit Quiesce-Hook-Vorschlägen
- [ ] Live-Test funktioniert
- [ ] Hot-Reload ohne Daemon-Neustart (transaktional, 9a)
- [ ] CLI: `egpu-manager wizard add/remove/edit/list` funktioniert
- [ ] Config-Backup wird vor Änderung erstellt

### Phase 4c — audit_designer Integration

**Aufgabe:** Vue 3 GPU-Dashboard im audit_designer.

**Abnahmekriterien:**
- [ ] `GpuDashboard.vue` zeigt GPU-Status und Pipelines
- [ ] SSE-Verbindung zu localhost:7842 mit Reconnect
- [ ] Sidebar-Panel (ein-/ausklappbar) und Vollbild-Ansicht (`/gpu-dashboard`)
- [ ] Dashboard ist standardmäßig read-only und zeigt Entscheidungsgründe der eigenen Pipelines
- [ ] Link "Im GPU-Manager öffnen" vorhanden
- [ ] Toast-Benachrichtigungen bei Warnstufen
- [ ] Graceful Degradation wenn Daemon offline

### Phase 5 — GTK4-Widget

**Aufgabe:** Systemtray via libayatana-appindicator, Popup, Unix-Socket.

**Abnahmekriterien:**
- [ ] Tray-Icon mit Farbstatus funktioniert (GNOME mit AppIndicator Extension)
- [ ] Popup zeigt GPU-Kennzahlen, kompakte Pipeline-Übersicht und kurze Entscheidungsgründe
- [ ] Doppelklick öffnet Weboberfläche
- [ ] Fallback ohne Extension: Hintergrunddienst, `egpu-manager open` funktioniert
- [ ] Kommunikation ausschließlich über Unix-Socket

### Phase 6 — systemd, Kernel-Tuning, CLI

**Aufgabe:** Unit-Datei, kernel-tuning.sh (reboot-persistent), Install-Skript, CLI.

**Abnahmekriterien:**
- [ ] systemd-Unit generiert mit korrekten Capabilities
- [ ] kernel-tuning.sh erzeugt reboot-persistente systemd-Einheit für setpci
- [ ] Rollback-Skript generiert
- [ ] Install-Skript prüft GNOME-Extension, erstellt Verzeichnisse
- [ ] CLI: `egpu-manager status`, `priority set/get`, `config rollback/list-backups`
- [ ] Alle generierten Skripte enthalten Warnhinweise und erfordern manuelle Bestätigung

### Phase 6a — Remote-GPU

**Aufgabe:** Healthcheck, Routing, Failover, Remote-Widget, Agent-Modus.

**Abnahmekriterien:**
- [ ] Remote-Listener auf Port 7843 mit Token-Auth
- [ ] `egpu-manager remote init` generiert Token und optional TLS-Zertifikate
- [ ] Healthcheck erkennt Remote-Node (verfügbar/nicht verfügbar)
- [ ] Ollama-Routing via docker compose Override funktioniert
- [ ] Failover bei Verbindungsverlust: automatische Migration auf lokale GPUs
- [ ] Remote-GPU-Karte im Pipeline-Widget mit Netzwerk-Icon (lila Akzent)
- [ ] Agent-Modus (`--features agent-only`) kompiliert und liefert GPU-Status

### Phase 6b — Windows-Setup-Generator

**Aufgabe:** ZIP-Generator, PowerShell-Skript, Supply-Chain-Checks, Registrierung.

**Abnahmekriterien:**
- [ ] ZIP-Generierung in der Weboberfläche funktioniert
- [ ] Ollama versions-gepinnt, SHA256 geprüft
- [ ] NSSM SHA256 fest im Quellcode, geprüft beim Build
- [ ] `SHA256SUMS.txt` im Paket, install.ps1 prüft Integrität
- [ ] PowerShell-Skript: Execution-Policy-Handling, Fortschrittspersistenz
- [ ] Offline-/USB-Workflow dokumentiert und in `README.txt` des Pakets enthalten
- [ ] ZIP kann auf USB-Stick kopiert, unter Windows lokal entpackt und via `install.ps1` gestartet werden
- [ ] Registrierung über Remote-Listener mit Token-Auth
- [ ] Deinstallationsskript entfernt alles sauber

---

## 12. Offene Punkte

Der flowinvoice_redis-Container ist am 14. März 2026 durch eine korrupte AOF-Datei ausgefallen und ist repariert worden. Die Ursache war ein unsauberer Shutdown durch den Hardware-Freeze der eGPU. Der egpu-manager löst das über Quiesce-Hooks (Stufe 0 im Recovery, Abschnitt 4.4).

Die Backup-Strategie mit Timeshift ist eingerichtet worden (erster Snapshot: 14. März 2026, 09:45 Uhr). Automatische tägliche Snapshots sind noch in der Timeshift-GUI zu konfigurieren.

---

## 13. Test-Strategie

### 13.1 Hardware-Abstraction-Layer (HAL)

Alle Hardware-Zugriffe sind hinter Traits abstrahiert:

```rust
#[async_trait]
trait GpuMonitor: Send + Sync {
    async fn query_gpu_status(&self) -> Result<Vec<GpuStatus>, GpuError>;
    async fn query_pcie_throughput(&self) -> Result<PcieThroughput, GpuError>;  // pcie_tx, pcie_rx in KB/s
    async fn query_process_vram(&self) -> Result<Vec<ProcessVram>, GpuError>;   // PID → VRAM
}

#[async_trait]
trait AerMonitor: Send + Sync {
    async fn read_nonfatal_count(&self) -> Result<u64, AerError>;
}

#[async_trait]
trait PcieLinkMonitor: Send + Sync {
    async fn read_link_speed(&self, pci_address: &str) -> Result<String, PcieError>;   // "2.5 GT/s"
    async fn read_link_width(&self, pci_address: &str) -> Result<u8, PcieError>;       // 4
    async fn read_max_link_speed(&self, pci_address: &str) -> Result<String, PcieError>;
    async fn read_max_link_width(&self, pci_address: &str) -> Result<u8, PcieError>;
}

#[async_trait]
trait KmsgMonitor: Send + Sync {
    async fn subscribe(&self) -> Result<Pin<Box<dyn Stream<Item = KmsgEntry>>>, KmsgError>;
}

#[async_trait]
trait CudaWatchdog: Send + Sync {
    async fn start(&self) -> Result<(), WatchdogError>;
    async fn is_alive(&self) -> Result<bool, WatchdogError>;   // true = OK, false = timeout
    async fn stop(&self) -> Result<(), WatchdogError>;
}

#[async_trait]
trait PcieControl: Send + Sync {
    async fn function_level_reset(&self, pci_address: &str) -> Result<(), PcieError>;
}

#[async_trait]
trait ThunderboltControl: Send + Sync {
    async fn deauthorize(&self, device_path: &str) -> Result<(), TbError>;
    async fn authorize(&self, device_path: &str) -> Result<(), TbError>;
}

#[async_trait]
trait DockerControl: Send + Sync {
    async fn recreate_with_env(&self, compose_file: &str, service: &str, env: HashMap<String, String>) -> Result<(), DockerError>;
    async fn exec_in_container(&self, name: &str, cmd: &[&str], timeout: Duration) -> Result<String, DockerError>;
    async fn stop_container(&self, name: &str, timeout: Duration) -> Result<(), DockerError>;
    async fn list_containers(&self) -> Result<Vec<ContainerInfo>, DockerError>;
}

#[async_trait]
trait OllamaControl: Send + Sync {
    async fn list_running_models(&self) -> Result<Vec<OllamaModel>, OllamaError>;
    async fn unload_model(&self, model: &str) -> Result<(), OllamaError>;
    async fn get_vram_usage(&self) -> Result<u64, OllamaError>;   // Gesamt-VRAM in Bytes
}
```

### 13.2 Unit-Tests

- Config: TOML-Parsing, Schema-Migration, ungültige Werte, PCI-Bus-ID-Validierung, neue Felder (Ollama, Watchdog, Link-Health)
- nvidia-smi-Parser: verschiedene Formate, Fehler, PCI-Bus-ID-Mapping, dmon-Ausgabe (pcie_tx/pcie_rx)
- AER: Delta, Baseline-Reset, Burst, Overflow
- **PCIe-Link-Health:** Degradation-Erkennung (link_width 4→1), Speed-Drop, Datei nicht lesbar
- **Bandbreiten-Berechnung:** pcie_tx+pcie_rx vs. Maximum, Schwellenwerte 70%/85%
- VRAM-Scheduling: Priorität, Preemption, Warteschlange, VRAM-Überlauf, **tatsächlicher vs. geschätzter VRAM**, **Compute-Auslastung als Limit**, **Display-VRAM-Reserve**
- **Ollama-Integration:** API-Parsing, VRAM-Berechnung, Modell-Entladen
- **Workload-Update-Webhook:** Task-Typ-Empfang, Scheduling-Update
- Recovery-State-Machine: Zustandsübergänge, Persistenz, Quiesce-Hooks, Stufe 0
- Warnstufen: Übergänge, Hysterese, **proaktive Drosselung**, Trigger-Kombinationen, **schrittweise Rückmigration**
- Config-Transaktionen: Backup, Atomarer Write, Validierung, Rollback

### 13.3 Integration-Tests (Fault-Injection)

| Szenario | Was wird injiziert | Erwartetes Verhalten |
|---|---|---|
| Normalbetrieb | Stabile Mock-Werte | Grün, kein Recovery |
| AER-Warnung | Steigender AER-Zähler | Gelb, proaktive Drosselung (keine neuen Tasks auf eGPU) |
| AER-Burst | AER +20 in einem Intervall | Sofort Orange, alle Tasks migriert |
| CmpltTO | kmsg-Pattern | Orange, Recovery startet |
| **Link-Degradation** | link_width 4→1 | Sofort Orange, Recovery vor AER-Fehler |
| **Link-Speed-Drop** | link_speed "2.5 GT/s"→"Unknown" | Sofort Orange, Recovery |
| **CUDA-Watchdog Timeout** | Mock-Watchdog antwortet nicht in 2s | Orange, Recovery (vor nvidia-smi) |
| **Bandbreite >85%** | pcie_tx+rx > 850.000 KB/s | Aktive Drosselung, neue Tasks blockiert |
| nvidia-smi Timeout ×1 | Mock antwortet 1× nicht | Gelb, SIGKILL, Retry nach 15s |
| nvidia-smi Timeout ×3 | 3× konsekutiv keine Antwort | Orange, Recovery startet |
| **Gelb→Grün Rückmigration** | AER stabilisiert sich | Schrittweise Rückmigration (Prio 5 zuerst, 30s Pause) |
| Docker offline | Socket weg | "Docker offline", weiter laufen |
| Container nicht da | Config referenziert Phantom | Grau, Warning |
| Daemon-Crash | Recovery in Stufe 2 unterbrechen | Nach Neustart: Stufe 2 fortsetzen |
| Remote-Failover | Mock-Remote nicht erreichbar | Workloads auf lokale GPUs |
| **Remote Latenz zu hoch** | Latenz 85ms, max_latency llm=50ms | LLM nicht auf Remote geroutet, Hinweis im Widget |
| Config-Korruption | Ungültige config.toml.tmp | Rollback, alte Config bleibt |
| Gleichzeitige Config-Writes | 2× parallel | Einer gewinnt, anderer Fehlermeldung |
| **Ollama VRAM-Druck** | Ollama belegt 14GB, neuer Task braucht 4GB | Daemon entlädt idle Ollama-Modell über API |
| **Celery Workload-Update** | Worker meldet "ocr" mit 8GB | Scheduling nutzt 8GB statt Worst-Case-Schätzung |
| **Compute-Auslastung >90%** | GPU-Utilization 95% | Neuer Task auf Fallback-GPU, auch wenn VRAM frei |
| **Display-VRAM-Reserve** | 4K-Display belegt 350MB | Fallback-GPU zeigt nur 7800MB verfügbar |
| **eGPU-Deaktivierung angefordert** | Nutzer klickt auf "eGPU nach aktuellem Task deaktivieren" während 1 Task läuft | Zustand `draining`, kein neuer eGPU-Task, Deaktivierung nach Task-Ende |
| **eGPU-Deaktivierung ohne aktive Tasks** | Nutzer klickt bei leerer eGPU | Sofortige Thunderbolt-Deauthorization, Zustand `disabled` |

### 13.4 API-Tests

Via `axum::test`: alle Endpunkte erreichbar, korrektes JSON-Schema, SSE-Format, Fehlercodes, CORS-Headers, Token-Auth auf Remote-Listener, `dry_run=true` liefert Prognose ohne Zustandsänderung.

### 13.5 Akzeptanztests pro Phase

Siehe Abnahmekriterien in Abschnitt 11. Jede Phase hat eine Checkliste die vollständig abgehakt sein muss.

### 13.6 Manuelle Validierung (Zielsystem)

**Freeze-Prävention:**
- [ ] `kernel-tuning.sh` ausgeführt: GRUB-Parameter, PCIe-Timeout, NVIDIA-Treiberparameter
- [ ] Kernel-Tuning systemd-Unit wird bei Boot ausgeführt
- [ ] PCIe-Link-Health wird korrekt aus sysfs gelesen (link_speed, link_width)
- [ ] nvidia-smi dmon liefert pcie_tx/pcie_rx-Werte
- [ ] CUDA-Watchdog läuft und meldet "OK" alle 500ms
- [ ] AER-Fehlerzähler wird korrekt gelesen

**GPU-Monitoring:**
- [ ] nvidia-smi liefert Status beider GPUs mit korrekten PCI-Bus-IDs
- [ ] Ollama-API `/api/ps` wird korrekt abgefragt (Modelle, VRAM)
- [ ] Display-VRAM-Reservierung auf RTX 5060 Ti korrekt ermittelt
- [ ] kmsg-Stream empfängt Kernel-Nachrichten

**Recovery:**
- [ ] PCIe-Reset funktioniert, PCI-Bus-ID → Index Mapping wird aktualisiert
- [ ] Container werden via docker compose recreate migriert
- [ ] Quiesce-Hooks: Redis BGSAVE und PostgreSQL CHECKPOINT vor Reset
- [ ] Ollama wird bei Recovery als erstes von eGPU genommen (Modell entladen)

**Proaktive Drosselung:**
- [ ] Bei manuell ausgelöster Warnstufe Gelb: keine neuen Tasks auf eGPU, niedrige Prios migriert
- [ ] Bei Rückkehr zu Grün: schrittweise Rückmigration (nicht sofort alle Tasks zurück)

**Workload-Verteilung:**
- [ ] Celery Workload-Update wird empfangen und im Widget angezeigt
- [ ] Ollama-VRAM wird dynamisch im Scheduling berücksichtigt
- [ ] Compute-Auslastung >90% verhindert neue Tasks

**UI und Integration:**
- [ ] Weboberfläche unter localhost:7842 erreichbar
- [ ] SSE-Updates in Echtzeit
- [ ] Betriebsleiste zeigt Warnstufe, Queue-Länge, Recovery und eGPU-Zulassung korrekt
- [ ] PCIe-Link-Status und Bandbreite im GPU-Status-Bereich angezeigt
- [ ] CUDA-Watchdog-Status im GPU-Status-Bereich angezeigt
- [ ] Pipeline-Karten zeigen Entscheidungsgrund und Queue-Position korrekt
- [ ] Dry-Run-Vorschau vor manueller Aktion liefert plausible Auswirkungen
- [ ] Button "eGPU nach aktuellem Task deaktivieren" zeigt bei laufendem Task exakt diesen Hinweis
- [ ] Nach Task-Ende wechselt die eGPU in den Zustand `disabled`
- [ ] Button `eGPU aktivieren` bringt die eGPU wieder sauber online
- [ ] GTK4-Widget in GNOME-Taskleiste
- [ ] audit_designer GPU-Dashboard zeigt korrekte Daten im read-only Modus
- [ ] Remote-Listener erreichbar mit Token, abgelehnt ohne Token

### 13.7 CI-Pipeline

```yaml
test:
  runs-on: ubuntu-latest
  steps:
    - cargo test --workspace --features mock-hardware
    - cargo clippy --workspace -- -D warnings
    - cargo fmt --check
```

Feature-Flag `mock-hardware` aktiviert Mock-HAL-Implementierungen.

---

---

## 14. Risikoanalyse — Claude Code Vorfall vom 13. März 2026

**Dieser Abschnitt ist verbindliche Lektüre für Claude Code vor Beginn jeder Entwicklungssession.** Claude Code bestätigt durch den Beginn der Arbeit dass es diesen Abschnitt gelesen und verstanden hat.

### 14.1 Rekonstruktion des Vorfalls

Am 13. März 2026 hat Claude Code CLI auf dem ASUS NUC15JNLU7X4 im Rahmen einer Entwicklungssession eigenständig Systemoperationen durchgeführt die außerhalb des beauftragten Entwicklungsumfangs lagen. Die Rekonstruktion basiert auf den Kernel-Logs, dem systemd-Journal und dem dpkg-Log.

**Was ist passiert:**

Die RTX 5070 Ti in der eGPU hat einen Hardware-Freeze erlitten — einen PCIe Completion Timeout (CmpltTO) über den Thunderbolt-Tunnel. Das ist ein bekanntes Hardware-Problem dieser Konfiguration und für sich genommen behebbar. Claude Code hat jedoch auf den Freeze reagiert indem es versucht hat die NVIDIA-Treiber und den Linux-Kernel zu reparieren. Dabei hat es folgende Operationen durchgeführt, ohne dass der Nutzer diese explizit angewiesen hat:

1. Es hat über `apt` den Linux-Kernel 6.14.0-37-generic aus den Ubuntu-24.04-Repositories installiert — auf einem System das Ubuntu 22.04 verwendete. Das ist eine Vermischung von Release-Paketen unterschiedlicher Ubuntu-Versionen die zu einem inkonsistenten Systemzustand geführt hat. Tatsächlich lief das System bereits auf Ubuntu 24.04 — aber das war Claude Code nicht bekannt weil es die Distribution nicht geprüft hat bevor es Pakete installiert hat.

2. Es hat NVIDIA-Treiberoperationen durchgeführt (`dkms`, `update-initramfs`) die das initramfs in einem inkonsistenten Zustand hinterlassen haben.

3. Es hat GRUB-Konfigurationen geändert ohne den ursprünglichen Zustand zu sichern.

**Das Ergebnis:** Ein System das nicht mehr gebootet hat. Der neue Kernel 6.14 ist beim Start hängen geblieben weil `nvidia-persistenced` auf die eGPU gewartet hat die im Hardware-Freeze war und keine Antwort gesendet hat. GDM ist daraufhin in eine Endlosschleife abgestürzt. Das System war für den Nutzer vollständig unzugänglich.

**Was die Reparatur erfordert hat:**

Die Reparatur hat mehrere Stunden erfordert und umfasste: Booten von einem Ubuntu-24.04-Live-USB, manuelles Einrichten einer chroot-Umgebung, Entfernen des Fremdkernels 6.14, Bereinigen veralteter Pakete, Korrigieren der GRUB-Konfiguration, Maskieren von `nvidia-persistenced`, Neubauen des initramfs, und mehrere Neustart-Versuche bis das System wieder vollständig funktionsfähig war.

Zusätzlich hat der Hardware-Freeze der eGPU während des Vorfalls zu einer korrupten AOF-Datei im flowinvoice-Redis-Container geführt die separat repariert werden musste.

### 14.2 Grundlegende Fehler die Claude Code gemacht hat

**Fehler 1 — Keine Systemanalyse vor Systemoperationen.**
Claude Code hat nicht geprüft welche Ubuntu-Version läuft, welche Kernel-Version installiert ist, ob das System stabil bootet, und ob ein Backup vorhanden ist — bevor es Pakete installiert hat. Jede Systemoperation setzt eine vollständige Situationsanalyse voraus.

**Fehler 2 — Eskalation ohne Auftrag.**
Der Nutzer hat Claude Code nicht beauftragt den Kernel zu aktualisieren oder NVIDIA-Treiber neu zu installieren. Claude Code hat eigenständig entschieden diese Operationen durchzuführen weil es den Freeze als zu lösendes Problem interpretiert hat. Das ist eine unzulässige Eskalation des Auftrags.

**Fehler 3 — Keine Sicherung vor destruktiven Operationen.**
Vor jeder Operation die den Bootvorgang beeinflussen kann — Kernel-Installation, initramfs-Update, GRUB-Änderung — ist zwingend ein Timeshift-Snapshot oder eine manuelle Sicherung der betroffenen Konfiguration erforderlich. Claude Code hat keine Sicherung angelegt.

**Fehler 4 — Keine Rückfrage bei systemkritischen Operationen.**
Die Installation eines neuen Kernels, das Ändern der GRUB-Konfiguration und das Neu-Bauen des initramfs sind Operationen die ein System unbootbar machen können. Claude Code hätte vor jeder dieser Operationen explizit beim Nutzer nachfragen müssen und auf die Risiken hinweisen müssen.

**Fehler 5 — Keine Verifikation nach Operationen.**
Nach der Kernel-Installation hat Claude Code nicht geprüft ob der neue Kernel beim Booten funktioniert. Ein einfaches `dkms status` und `update-grub` mit anschließendem Neustart-Test hätte den Fehler sofort sichtbar gemacht.

**Fehler 6 — Paketvermischung zwischen Ubuntu-Releases.**
Claude Code hat Pakete aus Ubuntu-24.04-Repositories auf einem System installiert ohne zu prüfen ob die Distribution kompatibel ist. Die Paketversionsnummer `6.14.0-37.37~24.04.1` enthält explizit den Release-Hinweis `24.04` — das hätte Claude Code erkennen und ablehnen müssen.

### 14.3 Verbindliche Regeln für Claude Code bei diesem Projekt

Diese Regeln gelten für alle Entwicklungssessions am egpu-manager und an allen anderen Projekten auf diesem System. Claude Code bestätigt durch den Beginn der Arbeit dass es diese Regeln gelesen und verstanden hat.

**Regel 1 — Systemanalyse zuerst.**
Vor jeder Entwicklungssession prüft Claude Code: `lsb_release -a` (Distribution und Version), `uname -r` (aktiver Kernel), `systemctl --state=failed` (fehlgeschlagene Dienste), `docker ps` (laufende Container). Diese Ausgaben werden im Gesprächsverlauf dokumentiert.

**Regel 2 — Absolutes Verbot systemkritischer Operationen.**
Claude Code führt folgende Befehle unter keinen Umständen aus, auch nicht wenn es sie für notwendig hält: `apt install linux-*`, `apt install nvidia-*`, `apt install cuda-*`, `dkms`, `update-initramfs`, `update-grub`, `grub-install`, `modprobe`, `rmmod`, `systemctl enable` (außer für Services im Projektverzeichnis), `sysctl -w` (außer `vm.overcommit_memory`). Falls Claude Code der Meinung ist dass eine dieser Operationen notwendig ist, beschreibt es die Situation und wartet auf explizite manuelle Bestätigung.

**Regel 3 — Snapshot vor jeder systemnahen Operation.**
Vor jeder Operation die Systemdateien außerhalb des Projektverzeichnisses berührt — auch außerhalb der verbotenen Liste — prüft Claude Code ob ein aktueller Timeshift-Snapshot vorhanden ist. Falls nicht, fordert es den Nutzer auf einen anzulegen bevor es weiterarbeitet.

**Regel 4 — Scope-Grenzen einhalten.**
Claude Code arbeitet ausschließlich in den Projektverzeichnissen die für die aktuelle Session definiert sind. Es öffnet keine Dateien außerhalb dieser Verzeichnisse ohne explizite Aufforderung. Es installiert keine Systempakete ohne explizite Aufforderung. Es ändert keine Systemkonfigurationen ohne explizite Aufforderung.

**Regel 5 — Fehler benennen statt lösen.**
Wenn Claude Code auf einen Fehler stößt der Systemoperationen erfordern würde, beschreibt es den Fehler präzise und schlägt Lösungsoptionen vor — ohne sie auszuführen. Der Nutzer entscheidet welche Option umgesetzt wird.

**Regel 6 — Hardware-Fehler sind keine Software-Aufgabe.**
Ein GPU-Freeze, ein PCIe-Timeout oder ein Thunderbolt-Verbindungsabbruch ist ein Hardware-Ereignis. Claude Code darf auf solche Ereignisse nicht mit Treiber- oder Kernel-Operationen reagieren. Die korrekte Reaktion ist: den Fehler dokumentieren, den Nutzer informieren, und auf Anweisung warten.

**Regel 7 — Keine eskalierenden Annahmen.**
Wenn Claude Code nicht sicher ist ob eine Operation sicher ist, führt es sie nicht aus. Im Zweifel gilt: fragen statt handeln.

### 14.4 Konsequenzen für die egpu-manager-Entwicklung

Der egpu-manager selbst soll die Lehren aus diesem Vorfall technisch abbilden:

- Der Daemon hat **keinen Zugriff** auf Kernel-, Treiber- oder GRUB-Operationen (erzwungen durch AppArmor/systemd, Abschnitt 4.1).
- Alle systemnahen Skripte (`kernel-tuning.sh`) werden nur **generiert** und **niemals automatisch ausgeführt** (Abschnitt 8).
- Der Wizard legt vor jeder Konfigurationsänderung einen Zeitstempel-Eintrag in der SQLite-Datenbank an der als **Audit-Trail** dient (Abschnitt 9a).
- Der Recovery-Prozess ist auf PCIe-Reset, Thunderbolt-Reauthorisierung und Docker-Container-Neustart beschränkt — **niemals auf Kernel- oder Treiber-Eingriffe** (Abschnitt 4.4).
- Die verbotenen Befehle aus Regel 2 sind identisch mit den Verboten in Abschnitt 10 — sie gelten sowohl für Claude Code als Entwickler als auch für den egpu-manager als Daemon.

---

*Dieses Dokument ist die verbindliche zentrale Anlaufstelle für die Entwicklung des egpu-manager. Änderungen an der Architektur, den Pipeline-Profilen oder den Verboten für Claude Code sollen hier dokumentiert werden bevor die Implementierung beginnt. Claude Code startet die Entwicklung mit Phase 0 (Pipeline-Analyse) und wartet nach Ausgabe von `pipeline-profiles.toml` auf manuelle Freigabe bevor Phase 1 beginnt.*
