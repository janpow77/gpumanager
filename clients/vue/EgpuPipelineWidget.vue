<!--
  eGPU Pipeline Widget - Vue 3 SFC
  Eigenstaendige Komponente fuer den eGPU Manager.
  Copy-paste-faehig in jedes Vue 3 Projekt.

  Props:
    gatewayUrl - URL zum eGPU Manager Gateway (default: http://localhost:7842)
    appId      - App-ID fuer LLM-Usage-Tracking (optional)
    compact    - Kompakte Darstellung (optional)
-->
<script setup lang="ts">
import {
  ref,
  computed,
  onMounted,
  onUnmounted,
  watch,
  type Ref,
} from 'vue'

// --- TypeScript Interfaces ---

interface GpuStatus {
  pci_address: string
  name: string
  gpu_type: 'internal' | 'egpu' | 'remote'
  temperature_c: number
  utilization_gpu_percent: number
  memory_used_mb: number
  memory_total_mb: number
  power_draw_w: number
  pstate: string
  status: 'online' | 'offline' | 'timeout'
}

interface SystemStatus {
  hostname: string
  cpu_percent: number
  memory_used_mb: number
  memory_total_mb: number
  uptime_seconds: number
}

interface PipelineInfo {
  container: string
  project: string
  gpu_device: string
  status: string
  vram_estimate_mb: number
}

interface ProviderStatus {
  name: string
  type: string
  status: string
  models?: string[]
}

interface LlmUsage {
  app_id: string
  total_cost_usd: number
  month_cost_usd: number
}

interface StatusResponse {
  gpus: GpuStatus[]
  warning_level: string
}

// --- Props ---

const props = withDefaults(
  defineProps<{
    gatewayUrl?: string
    appId?: string
    compact?: boolean
  }>(),
  {
    gatewayUrl: 'http://localhost:7842',
    compact: false,
  }
)

// --- Reaktiver State ---

const connected: Ref<boolean> = ref(false)
const connecting: Ref<boolean> = ref(true)
const gpus: Ref<GpuStatus[]> = ref([])
const warningLevel: Ref<string> = ref('green')
const system: Ref<SystemStatus | null> = ref(null)
const pipelines: Ref<PipelineInfo[]> = ref([])
const providers: Ref<ProviderStatus[]> = ref([])
const llmUsage: Ref<LlmUsage | null> = ref(null)
const lastUpdate: Ref<Date | null> = ref(null)
const isDarkMode: Ref<boolean> = ref(false)

let eventSource: EventSource | null = null
let reconnectTimer: ReturnType<typeof setTimeout> | null = null
let pollTimer: ReturnType<typeof setInterval> | null = null

// --- Berechnete Werte ---

const warningColor = computed(() => {
  const map: Record<string, string> = {
    green: '#76b900',
    yellow: '#facc15',
    orange: '#f97316',
    red: '#ef4444',
  }
  return map[warningLevel.value] || '#76b900'
})

const uptimeFormatted = computed(() => {
  if (!system.value) return '--'
  const s = system.value.uptime_seconds
  const d = Math.floor(s / 86400)
  const h = Math.floor((s % 86400) / 3600)
  const m = Math.floor((s % 3600) / 60)
  if (d > 0) return `${d}d ${h}h ${m}m`
  if (h > 0) return `${h}h ${m}m`
  return `${m}m`
})

const memoryPercent = computed(() => {
  if (!system.value || !system.value.memory_total_mb) return 0
  return Math.round(
    (system.value.memory_used_mb / system.value.memory_total_mb) * 100
  )
})

// --- GPU-Typ Farben und Labels ---

function gpuTypeBadge(type: string): { label: string; color: string } {
  switch (type) {
    case 'egpu':
      return { label: 'eGPU', color: '#00b0f7' }
    case 'remote':
      return { label: 'Remote', color: '#a855f7' }
    default:
      return { label: 'Internal', color: '#6b7280' }
  }
}

function statusDotColor(status: string): string {
  switch (status) {
    case 'online':
      return '#76b900'
    case 'timeout':
      return '#facc15'
    default:
      return '#ef4444'
  }
}

// Temperatur-Farbe basierend auf Grad
function tempColor(temp: number): string {
  if (temp < 50) return '#76b900'
  if (temp < 70) return '#facc15'
  if (temp < 85) return '#f97316'
  return '#ef4444'
}

// VRAM / Util Farbe
function barColor(percent: number): string {
  if (percent < 60) return '#76b900'
  if (percent < 80) return '#facc15'
  if (percent < 95) return '#f97316'
  return '#ef4444'
}

// Pipeline-Status Farbe
function pipelineStatusColor(status: string): string {
  switch (status.toLowerCase()) {
    case 'running':
      return '#76b900'
    case 'stopped':
    case 'exited':
      return '#ef4444'
    case 'paused':
      return '#facc15'
    default:
      return '#6b7280'
  }
}

// MB formatieren
function formatMb(mb: number): string {
  if (mb >= 1024) return `${(mb / 1024).toFixed(1)} GB`
  return `${Math.round(mb)} MB`
}

// --- API-Aufrufe ---

async function fetchJson<T>(path: string): Promise<T | null> {
  try {
    const res = await fetch(`${props.gatewayUrl}${path}`, {
      signal: AbortSignal.timeout(5000),
    })
    if (!res.ok) return null
    return (await res.json()) as T
  } catch {
    return null
  }
}

async function fetchAll(): Promise<void> {
  const [statusRes, systemRes, pipelinesRes] = await Promise.allSettled([
    fetchJson<StatusResponse>('/api/status'),
    fetchJson<SystemStatus>('/api/system'),
    fetchJson<{ pipelines: PipelineInfo[] }>('/api/pipelines'),
  ])

  if (statusRes.status === 'fulfilled' && statusRes.value) {
    gpus.value = statusRes.value.gpus
    warningLevel.value = statusRes.value.warning_level
    connected.value = true
  }

  if (systemRes.status === 'fulfilled' && systemRes.value) {
    system.value = systemRes.value
  }

  if (pipelinesRes.status === 'fulfilled' && pipelinesRes.value) {
    pipelines.value = pipelinesRes.value.pipelines
  }

  // LLM-Daten nur laden wenn appId gesetzt
  if (props.appId) {
    const [providersRes, usageRes] = await Promise.allSettled([
      fetchJson<{ providers: ProviderStatus[] }>('/api/llm/providers'),
      fetchJson<LlmUsage>(`/api/llm/usage/${props.appId}`),
    ])

    if (providersRes.status === 'fulfilled' && providersRes.value) {
      providers.value = providersRes.value.providers
    }

    if (usageRes.status === 'fulfilled' && usageRes.value) {
      llmUsage.value = usageRes.value
    }
  }

  lastUpdate.value = new Date()
  connecting.value = false
}

// --- SSE-Verbindung mit Auto-Reconnect ---

function connectSSE(): void {
  if (eventSource) {
    eventSource.close()
  }

  try {
    eventSource = new EventSource(`${props.gatewayUrl}/api/events/stream`)

    eventSource.onopen = () => {
      connected.value = true
      connecting.value = false
    }

    eventSource.onmessage = (event) => {
      try {
        const data = JSON.parse(event.data)
        handleSSEEvent(data)
        lastUpdate.value = new Date()
      } catch {
        // Ungueltiges JSON ignorieren
      }
    }

    eventSource.onerror = () => {
      connected.value = false
      eventSource?.close()
      eventSource = null
      // Reconnect nach 3 Sekunden
      scheduleReconnect()
    }
  } catch {
    connected.value = false
    scheduleReconnect()
  }
}

function handleSSEEvent(data: Record<string, unknown>): void {
  // SSE-Events koennen verschiedene Typen haben
  if (data.gpus && Array.isArray(data.gpus)) {
    gpus.value = data.gpus as GpuStatus[]
  }
  if (typeof data.warning_level === 'string') {
    warningLevel.value = data.warning_level
  }
  if (data.system && typeof data.system === 'object') {
    system.value = data.system as SystemStatus
  }
  if (data.pipelines && Array.isArray(data.pipelines)) {
    pipelines.value = data.pipelines as PipelineInfo[]
  }
}

function scheduleReconnect(): void {
  if (reconnectTimer) clearTimeout(reconnectTimer)
  reconnectTimer = setTimeout(() => {
    fetchAll().then(() => connectSSE())
  }, 3000)
}

// --- Polling als Fallback (alle 10s) ---

function startPolling(): void {
  stopPolling()
  pollTimer = setInterval(() => {
    fetchAll()
  }, 10000)
}

function stopPolling(): void {
  if (pollTimer) {
    clearInterval(pollTimer)
    pollTimer = null
  }
}

// --- Dark Mode Erkennung ---

function detectDarkMode(): void {
  if (typeof window !== 'undefined' && window.matchMedia) {
    const mq = window.matchMedia('(prefers-color-scheme: dark)')
    isDarkMode.value = mq.matches
    mq.addEventListener('change', (e) => {
      isDarkMode.value = e.matches
    })
  }
}

// --- Lifecycle ---

onMounted(async () => {
  detectDarkMode()
  await fetchAll()

  if (connected.value) {
    connectSSE()
  } else {
    // Fallback: Polling wenn erste Verbindung fehlschlaegt
    scheduleReconnect()
  }

  // Zusaetzliches Polling fuer Daten die nicht via SSE kommen
  startPolling()
})

onUnmounted(() => {
  if (eventSource) {
    eventSource.close()
    eventSource = null
  }
  if (reconnectTimer) {
    clearTimeout(reconnectTimer)
    reconnectTimer = null
  }
  stopPolling()
})

// Gateway-URL Aenderung: Neu verbinden
watch(
  () => props.gatewayUrl,
  () => {
    connected.value = false
    connecting.value = true
    fetchAll().then(() => connectSSE())
  }
)
</script>

<template>
  <div
    class="egpu-widget"
    :class="{ dark: isDarkMode, compact: props.compact }"
  >
    <!-- Header -->
    <div class="widget-header">
      <div class="header-left">
        <svg class="logo-icon" viewBox="0 0 24 24" width="20" height="20">
          <rect x="2" y="6" width="20" height="12" rx="2" fill="#76b900" />
          <rect x="5" y="9" width="4" height="6" rx="1" fill="#fff" opacity="0.9" />
          <rect x="11" y="9" width="4" height="6" rx="1" fill="#fff" opacity="0.7" />
          <circle cx="19" cy="12" r="1.5" fill="#fff" opacity="0.5" />
        </svg>
        <span class="header-title">eGPU Manager</span>
        <span
          class="warning-badge"
          :style="{ backgroundColor: warningColor }"
        >
          {{ warningLevel.toUpperCase() }}
        </span>
      </div>
      <div class="header-right">
        <span v-if="system" class="hostname">{{ system.hostname }}</span>
        <span
          class="status-dot"
          :class="{
            online: connected,
            connecting: connecting && !connected,
            offline: !connected && !connecting,
          }"
          :title="
            connected
              ? 'Verbunden'
              : connecting
                ? 'Verbinde...'
                : 'Offline'
          "
        />
      </div>
    </div>

    <!-- Offline-Banner -->
    <div v-if="!connected && !connecting" class="offline-banner">
      <span>Gateway nicht erreichbar</span>
      <button class="retry-btn" @click="fetchAll().then(() => connectSSE())">
        Erneut versuchen
      </button>
    </div>

    <!-- GPU-Karten -->
    <div v-if="gpus.length > 0" class="section">
      <div class="section-title">GPUs</div>
      <div class="gpu-grid">
        <div
          v-for="gpu in gpus"
          :key="gpu.pci_address"
          class="gpu-card"
          :class="{ offline: gpu.status !== 'online' }"
        >
          <!-- PCB oberer Rand -->
          <div class="pcb-edge top" />

          <div class="gpu-card-header">
            <div class="gpu-name-row">
              <!-- Luefter-SVG mit CSS-Animation -->
              <svg
                class="fan-svg"
                :class="{ spinning: gpu.status === 'online' && gpu.utilization_gpu_percent > 0 }"
                viewBox="0 0 24 24"
                width="20"
                height="20"
              >
                <circle cx="12" cy="12" r="10" fill="none" stroke="currentColor" stroke-width="1" opacity="0.3" />
                <circle cx="12" cy="12" r="2" fill="currentColor" />
                <!-- Luefter-Blaetter -->
                <path d="M12 4 C14 7, 16 9, 12 12" fill="currentColor" opacity="0.7" />
                <path d="M20 12 C17 14, 15 16, 12 12" fill="currentColor" opacity="0.7" />
                <path d="M12 20 C10 17, 8 15, 12 12" fill="currentColor" opacity="0.7" />
                <path d="M4 12 C7 10, 9 8, 12 12" fill="currentColor" opacity="0.7" />
              </svg>
              <span class="gpu-name">{{ gpu.name }}</span>
              <span
                class="gpu-status-dot"
                :style="{ backgroundColor: statusDotColor(gpu.status) }"
              />
            </div>
            <span
              class="type-badge"
              :style="{ backgroundColor: gpuTypeBadge(gpu.gpu_type).color }"
            >
              {{ gpuTypeBadge(gpu.gpu_type).label }}
            </span>
          </div>

          <div class="gpu-stats">
            <!-- Temperatur -->
            <div class="stat-row">
              <span class="stat-label">Temp</span>
              <div class="stat-bar-wrap">
                <div
                  class="stat-bar"
                  :style="{
                    width: Math.min(gpu.temperature_c, 100) + '%',
                    backgroundColor: tempColor(gpu.temperature_c),
                  }"
                />
              </div>
              <span class="stat-value" :style="{ color: tempColor(gpu.temperature_c) }">
                {{ gpu.temperature_c }}&deg;C
              </span>
            </div>

            <!-- GPU-Auslastung -->
            <div class="stat-row">
              <span class="stat-label">Util</span>
              <div class="stat-bar-wrap">
                <div
                  class="stat-bar"
                  :style="{
                    width: gpu.utilization_gpu_percent + '%',
                    backgroundColor: barColor(gpu.utilization_gpu_percent),
                  }"
                />
              </div>
              <span class="stat-value">{{ gpu.utilization_gpu_percent }}%</span>
            </div>

            <!-- VRAM -->
            <div class="stat-row">
              <span class="stat-label">VRAM</span>
              <div class="stat-bar-wrap">
                <div
                  class="stat-bar"
                  :style="{
                    width:
                      gpu.memory_total_mb > 0
                        ? (gpu.memory_used_mb / gpu.memory_total_mb) * 100 + '%'
                        : '0%',
                    backgroundColor: barColor(
                      gpu.memory_total_mb > 0
                        ? (gpu.memory_used_mb / gpu.memory_total_mb) * 100
                        : 0
                    ),
                  }"
                />
              </div>
              <span class="stat-value">
                {{ formatMb(gpu.memory_used_mb) }} / {{ formatMb(gpu.memory_total_mb) }}
              </span>
            </div>

            <!-- Power & PState -->
            <div v-if="!props.compact" class="stat-footer">
              <span class="power">{{ gpu.power_draw_w.toFixed(0) }}W</span>
              <span class="pstate">{{ gpu.pstate }}</span>
            </div>
          </div>

          <!-- PCB unterer Rand -->
          <div class="pcb-edge bottom" />
        </div>
      </div>
    </div>

    <!-- Pipelines -->
    <div v-if="pipelines.length > 0 && !props.compact" class="section">
      <div class="section-title">Pipelines</div>
      <div class="pipeline-table-wrap">
        <table class="pipeline-table">
          <thead>
            <tr>
              <th>Container</th>
              <th>Projekt</th>
              <th>GPU</th>
              <th>VRAM</th>
              <th>Status</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="p in pipelines" :key="p.container">
              <td class="cell-container">{{ p.container }}</td>
              <td>{{ p.project }}</td>
              <td class="cell-gpu">{{ p.gpu_device }}</td>
              <td>{{ formatMb(p.vram_estimate_mb) }}</td>
              <td>
                <span
                  class="pipeline-status"
                  :style="{ color: pipelineStatusColor(p.status) }"
                >
                  {{ p.status }}
                </span>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </div>

    <!-- System Stats -->
    <div v-if="system" class="section">
      <div class="section-title">System</div>
      <div class="system-grid">
        <div class="system-stat">
          <span class="sys-label">CPU</span>
          <div class="stat-bar-wrap">
            <div
              class="stat-bar"
              :style="{
                width: system.cpu_percent + '%',
                backgroundColor: barColor(system.cpu_percent),
              }"
            />
          </div>
          <span class="stat-value">{{ system.cpu_percent.toFixed(0) }}%</span>
        </div>
        <div class="system-stat">
          <span class="sys-label">RAM</span>
          <div class="stat-bar-wrap">
            <div
              class="stat-bar"
              :style="{
                width: memoryPercent + '%',
                backgroundColor: barColor(memoryPercent),
              }"
            />
          </div>
          <span class="stat-value">
            {{ formatMb(system.memory_used_mb) }} / {{ formatMb(system.memory_total_mb) }}
          </span>
        </div>
        <div v-if="!props.compact" class="system-stat">
          <span class="sys-label">Uptime</span>
          <span class="stat-value uptime-value">{{ uptimeFormatted }}</span>
        </div>
      </div>
    </div>

    <!-- LLM Status (nur bei gesetzter appId) -->
    <div v-if="props.appId && providers.length > 0 && !props.compact" class="section">
      <div class="section-title">LLM Providers</div>
      <div class="providers-list">
        <div
          v-for="prov in providers"
          :key="prov.name"
          class="provider-item"
        >
          <span
            class="provider-dot"
            :style="{
              backgroundColor:
                prov.status === 'online' ? '#76b900' : '#ef4444',
            }"
          />
          <span class="provider-name">{{ prov.name }}</span>
          <span class="provider-type">{{ prov.type }}</span>
        </div>
      </div>
      <div v-if="llmUsage" class="llm-costs">
        <span>Kosten (Monat): ${{ llmUsage.month_cost_usd.toFixed(4) }}</span>
        <span>Gesamt: ${{ llmUsage.total_cost_usd.toFixed(4) }}</span>
      </div>
    </div>

    <!-- Footer -->
    <div class="widget-footer">
      <span v-if="lastUpdate" class="last-update">
        {{ lastUpdate.toLocaleTimeString('de-DE') }}
      </span>
    </div>
  </div>
</template>

<style scoped>
/* --- CSS-Variablen fuer Light/Dark Mode --- */

.egpu-widget {
  --bg: #ffffff;
  --bg-card: #f8fafc;
  --bg-card-hover: #f1f5f9;
  --bg-section: #f1f5f9;
  --border: #e2e8f0;
  --text: #1e293b;
  --text-secondary: #64748b;
  --text-muted: #94a3b8;
  --bar-bg: #e2e8f0;
  --pcb-color: #2d5016;
  --pcb-trace: rgba(118, 185, 0, 0.2);
  --shadow: 0 1px 3px rgba(0, 0, 0, 0.1);

  font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto,
    'Helvetica Neue', Arial, sans-serif;
  font-size: 13px;
  color: var(--text);
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: 12px;
  overflow: hidden;
  max-width: 720px;
  box-shadow: var(--shadow);
  line-height: 1.4;
}

.egpu-widget.dark {
  --bg: #0f172a;
  --bg-card: #1e293b;
  --bg-card-hover: #334155;
  --bg-section: #1e293b;
  --border: #334155;
  --text: #e2e8f0;
  --text-secondary: #94a3b8;
  --text-muted: #64748b;
  --bar-bg: #334155;
  --pcb-color: #1a3a08;
  --pcb-trace: rgba(118, 185, 0, 0.15);
  --shadow: 0 1px 3px rgba(0, 0, 0, 0.4);
}

.egpu-widget.compact {
  max-width: 400px;
  font-size: 12px;
}

/* --- Header --- */

.widget-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 14px;
  border-bottom: 1px solid var(--border);
  background: var(--bg-section);
}

.header-left {
  display: flex;
  align-items: center;
  gap: 8px;
}

.logo-icon {
  flex-shrink: 0;
}

.header-title {
  font-weight: 700;
  font-size: 14px;
}

.warning-badge {
  font-size: 10px;
  font-weight: 700;
  color: #fff;
  padding: 2px 8px;
  border-radius: 9999px;
  text-transform: uppercase;
  letter-spacing: 0.5px;
}

.header-right {
  display: flex;
  align-items: center;
  gap: 8px;
}

.hostname {
  font-size: 11px;
  color: var(--text-muted);
  font-family: 'SF Mono', 'Fira Code', monospace;
}

.status-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex-shrink: 0;
}

.status-dot.online {
  background-color: #76b900;
  box-shadow: 0 0 6px rgba(118, 185, 0, 0.6);
}

.status-dot.connecting {
  background-color: #facc15;
  animation: pulse 1.5s ease-in-out infinite;
}

.status-dot.offline {
  background-color: #ef4444;
}

/* --- Offline-Banner --- */

.offline-banner {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 12px;
  padding: 8px 14px;
  background: rgba(239, 68, 68, 0.1);
  border-bottom: 1px solid rgba(239, 68, 68, 0.2);
  color: #ef4444;
  font-size: 12px;
  font-weight: 500;
}

.retry-btn {
  font-size: 11px;
  padding: 3px 10px;
  border-radius: 6px;
  border: 1px solid #ef4444;
  background: transparent;
  color: #ef4444;
  cursor: pointer;
  transition: background 0.15s;
}

.retry-btn:hover {
  background: rgba(239, 68, 68, 0.15);
}

/* --- Sections --- */

.section {
  padding: 10px 14px;
  border-bottom: 1px solid var(--border);
}

.section:last-of-type {
  border-bottom: none;
}

.section-title {
  font-size: 11px;
  font-weight: 700;
  text-transform: uppercase;
  letter-spacing: 0.8px;
  color: var(--text-muted);
  margin-bottom: 8px;
}

/* --- GPU-Karten --- */

.gpu-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
  gap: 10px;
}

.compact .gpu-grid {
  grid-template-columns: 1fr;
}

.gpu-card {
  background: var(--bg-card);
  border: 1px solid var(--border);
  border-radius: 8px;
  overflow: hidden;
  transition: background 0.15s;
  position: relative;
}

.gpu-card:hover {
  background: var(--bg-card-hover);
}

.gpu-card.offline {
  opacity: 0.5;
}

/* PCB-Stil: oberer und unterer Rand als Platinen-Kante */
.pcb-edge {
  height: 4px;
  background: var(--pcb-color);
  background-image: repeating-linear-gradient(
    90deg,
    var(--pcb-trace) 0px,
    var(--pcb-trace) 2px,
    transparent 2px,
    transparent 8px
  );
}

.gpu-card-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 8px 10px 4px;
}

.gpu-name-row {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}

.fan-svg {
  flex-shrink: 0;
  color: var(--text-secondary);
}

.fan-svg.spinning {
  animation: spin 1.2s linear infinite;
}

.gpu-name {
  font-weight: 600;
  font-size: 12px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.gpu-status-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  flex-shrink: 0;
}

.type-badge {
  font-size: 9px;
  font-weight: 700;
  color: #fff;
  padding: 1px 6px;
  border-radius: 4px;
  text-transform: uppercase;
  letter-spacing: 0.3px;
  flex-shrink: 0;
}

.gpu-stats {
  padding: 4px 10px 8px;
}

/* --- Stat-Zeilen (Balken) --- */

.stat-row {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-bottom: 4px;
}

.stat-label,
.sys-label {
  font-size: 10px;
  font-weight: 600;
  color: var(--text-muted);
  width: 34px;
  flex-shrink: 0;
  text-transform: uppercase;
}

.stat-bar-wrap {
  flex: 1;
  height: 6px;
  background: var(--bar-bg);
  border-radius: 3px;
  overflow: hidden;
}

.stat-bar {
  height: 100%;
  border-radius: 3px;
  transition: width 0.4s ease, background-color 0.3s ease;
  min-width: 0;
}

.stat-value {
  font-size: 10px;
  font-weight: 600;
  color: var(--text-secondary);
  min-width: 60px;
  text-align: right;
  white-space: nowrap;
  font-family: 'SF Mono', 'Fira Code', monospace;
}

.stat-footer {
  display: flex;
  justify-content: space-between;
  margin-top: 4px;
  padding-top: 4px;
  border-top: 1px solid var(--border);
}

.power,
.pstate {
  font-size: 10px;
  color: var(--text-muted);
  font-family: 'SF Mono', 'Fira Code', monospace;
}

/* --- Pipeline-Tabelle --- */

.pipeline-table-wrap {
  overflow-x: auto;
  -webkit-overflow-scrolling: touch;
}

.pipeline-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 11px;
}

.pipeline-table th {
  text-align: left;
  font-size: 10px;
  font-weight: 700;
  text-transform: uppercase;
  letter-spacing: 0.5px;
  color: var(--text-muted);
  padding: 4px 8px;
  border-bottom: 1px solid var(--border);
}

.pipeline-table td {
  padding: 5px 8px;
  border-bottom: 1px solid var(--border);
  color: var(--text-secondary);
}

.pipeline-table tr:last-child td {
  border-bottom: none;
}

.cell-container {
  font-family: 'SF Mono', 'Fira Code', monospace;
  font-weight: 500;
  color: var(--text);
}

.cell-gpu {
  font-family: 'SF Mono', 'Fira Code', monospace;
  font-size: 10px;
}

.pipeline-status {
  font-weight: 600;
  text-transform: capitalize;
}

/* --- System Stats --- */

.system-grid {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.system-stat {
  display: flex;
  align-items: center;
  gap: 6px;
}

.uptime-value {
  margin-left: auto;
}

/* --- LLM Providers --- */

.providers-list {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.provider-item {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
}

.provider-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  flex-shrink: 0;
}

.provider-name {
  font-weight: 600;
  color: var(--text);
}

.provider-type {
  font-size: 10px;
  color: var(--text-muted);
  font-family: 'SF Mono', 'Fira Code', monospace;
}

.llm-costs {
  display: flex;
  gap: 16px;
  margin-top: 6px;
  padding-top: 6px;
  border-top: 1px solid var(--border);
  font-size: 11px;
  color: var(--text-secondary);
  font-family: 'SF Mono', 'Fira Code', monospace;
}

/* --- Footer --- */

.widget-footer {
  padding: 4px 14px;
  text-align: right;
  border-top: 1px solid var(--border);
}

.last-update {
  font-size: 10px;
  color: var(--text-muted);
  font-family: 'SF Mono', 'Fira Code', monospace;
}

/* --- Animationen --- */

@keyframes spin {
  from {
    transform: rotate(0deg);
  }
  to {
    transform: rotate(360deg);
  }
}

@keyframes pulse {
  0%,
  100% {
    opacity: 1;
  }
  50% {
    opacity: 0.3;
  }
}

/* --- Responsive --- */

@media (max-width: 480px) {
  .egpu-widget {
    font-size: 12px;
    border-radius: 8px;
  }

  .gpu-grid {
    grid-template-columns: 1fr;
  }

  .stat-value {
    min-width: 50px;
    font-size: 9px;
  }

  .llm-costs {
    flex-direction: column;
    gap: 2px;
  }
}

/* --- Prefers Color Scheme Fallback --- */

@media (prefers-color-scheme: dark) {
  .egpu-widget:not(.dark) {
    /* JS erkennt den Modus, dies ist nur ein Fallback */
  }
}
</style>
