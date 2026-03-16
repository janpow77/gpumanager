/**
 * eGPU Pipeline Widget - React 18+
 *
 * Eigenstaendige Komponente zur Visualisierung des eGPU Manager Pipelines.
 * Uebertraegt das PCB-Karten-Design aus der eingebetteten UI (ui.rs).
 *
 * Keine externen Abhaengigkeiten ausser React.
 *
 * Nutzung:
 *   <EgpuPipelineWidget />
 *   <EgpuPipelineWidget gatewayUrl="http://nuc:7842" appId="meine-app" />
 *   <EgpuPipelineWidget compact />
 */

import React, {
  useState,
  useEffect,
  useRef,
  useCallback,
  useMemo,
  type CSSProperties,
  type FC,
} from "react";

// ---------------------------------------------------------------------------
// Typen
// ---------------------------------------------------------------------------

export interface EgpuPipelineWidgetProps {
  /** URL des eGPU Manager Daemons (default: http://localhost:7842) */
  gatewayUrl?: string;
  /** Optionale App-ID fuer LLM-Kostenanzeige */
  appId?: string;
  /** Kompaktdarstellung ohne Pipeline-Details */
  compact?: boolean;
}

interface GpuStatus {
  pci_address: string;
  name: string;
  type?: "internal" | "egpu" | "remote";
  gpu_type?: "internal" | "egpu" | "remote";
  temperature_c: number;
  utilization_gpu_percent: number;
  memory_used_mb: number;
  memory_total_mb: number;
  power_draw_w: number;
  pstate: string;
  status: "online" | "offline" | "timeout" | "available";
  host?: string;
  latency_ms?: number | null;
}

interface DaemonStatus {
  warning_level?: string;
  uptime_seconds?: number;
  recovery_active?: boolean;
  recovery_stage?: string;
}

interface StatusResponse {
  daemon?: DaemonStatus;
  gpus?: GpuStatus[];
  remote_gpus?: Array<{
    gpu_name?: string;
    name?: string;
    status?: string;
    vram_mb?: number;
    host?: string;
    latency_ms?: number | null;
  }>;
}

interface SystemStatus {
  hostname: string;
  cpu_percent: number;
  memory_used_mb: number;
  memory_total_mb: number;
  swap_used_mb?: number;
  swap_total_mb?: number;
  uptime_seconds: number;
}

interface PipelineInfo {
  container: string;
  project: string;
  gpu_device: string;
  status: string;
  vram_estimate_mb: number;
  workload_types?: string[];
  priority?: number;
}

interface LlmProvider {
  name: string;
  status: string;
  model?: string;
  requests_total?: number;
  cost_total_usd?: number;
}

interface LlmUsage {
  total_cost_usd: number;
  total_requests: number;
  providers: Record<string, { cost_usd: number; requests: number }>;
}

type WarningLevel = "green" | "yellow" | "orange" | "red";

// ---------------------------------------------------------------------------
// Farbschema (aus ui.rs)
// ---------------------------------------------------------------------------

const COLORS = {
  nv: "#76b900",
  nvGlow: "rgba(118,185,0,.35)",
  tb: "#00b0f0",
  tbGlow: "rgba(0,176,240,.3)",
  amber: "#f59e0b",
  amberGlow: "rgba(245,158,11,.3)",
  red: "#ef4444",
  warm: "#f97316",
  purple: "#a855f7",
  purpleGlow: "rgba(168,85,247,.3)",
} as const;

// Dark- und Light-Mode Tokens
const darkTokens = {
  bg: "#1a1a18",
  bgCard: "#2a2a27",
  bgDeep: "#1e1e1c",
  border: "#444440",
  text: "#e8e7e0",
  muted: "#9c9a92",
};

const lightTokens = {
  bg: "#ffffff",
  bgCard: "#f5f4f0",
  bgDeep: "#e8e7e2",
  border: "#c8c7c0",
  text: "#1a1a18",
  muted: "#6b6b66",
};

type ThemeTokens = typeof darkTokens;

// ---------------------------------------------------------------------------
// Hilfsfunktionen
// ---------------------------------------------------------------------------

function useMediaQuery(query: string): boolean {
  const [matches, setMatches] = useState(() =>
    typeof window !== "undefined" ? window.matchMedia(query).matches : false,
  );
  useEffect(() => {
    if (typeof window === "undefined") return;
    const mql = window.matchMedia(query);
    const handler = (e: MediaQueryListEvent) => setMatches(e.matches);
    mql.addEventListener("change", handler);
    return () => mql.removeEventListener("change", handler);
  }, [query]);
  return matches;
}

function useTheme(): ThemeTokens {
  const prefersDark = useMediaQuery("(prefers-color-scheme: dark)");
  return prefersDark ? darkTokens : lightTokens;
}

function tempColor(t: number): string {
  if (t >= 80) return COLORS.red;
  if (t >= 65) return COLORS.warm;
  return COLORS.nv;
}

function formatUptime(secs: number): string {
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  if (h > 0) return `${h}h ${m}m`;
  return `${m}m`;
}

function warningFromString(s?: string): WarningLevel {
  if (!s) return "green";
  const lower = s.toLowerCase();
  if (lower.includes("rot") || lower.includes("red")) return "red";
  if (lower.includes("orange")) return "orange";
  if (lower.includes("gelb") || lower.includes("yellow")) return "yellow";
  return "green";
}

function gpuTypeOf(g: GpuStatus): "internal" | "egpu" | "remote" {
  return g.gpu_type ?? g.type ?? "internal";
}

function gpuBorderColor(gpuType: string): string {
  if (gpuType === "egpu") return `rgba(0,176,240,.3)`;
  if (gpuType === "remote") return `rgba(168,85,247,.3)`;
  return `rgba(118,185,0,.5)`;
}

// ---------------------------------------------------------------------------
// CSS-Keyframes (einmalig injiziert)
// ---------------------------------------------------------------------------

const KEYFRAMES_ID = "egpu-widget-keyframes";

function injectKeyframes(): void {
  if (typeof document === "undefined") return;
  if (document.getElementById(KEYFRAMES_ID)) return;
  const style = document.createElement("style");
  style.id = KEYFRAMES_ID;
  style.textContent = `
@keyframes egpu-spin { to { transform: rotate(360deg) } }
@keyframes egpu-blink-nv {
  0%,100% { opacity:1; box-shadow: 0 0 8px ${COLORS.nvGlow} }
  50% { opacity:.2; box-shadow:none }
}
@keyframes egpu-blink-amb {
  0%,100% { opacity:1 }
  50% { opacity:.3 }
}
@keyframes egpu-flow {
  0% { left:-6px; opacity:0 }
  10% { opacity:1 }
  90% { opacity:1 }
  100% { left:calc(100% + 6px); opacity:0 }
}
@media(prefers-reduced-motion:reduce){
  .egpu-fan { animation:none!important }
  .egpu-dot { animation:none!important; opacity:0!important }
}
  `;
  document.head.appendChild(style);
}

// ---------------------------------------------------------------------------
// Datenhooks
// ---------------------------------------------------------------------------

function useEgpuData(gatewayUrl: string, appId?: string) {
  const [connected, setConnected] = useState(false);
  const [status, setStatus] = useState<StatusResponse | null>(null);
  const [system, setSystem] = useState<SystemStatus | null>(null);
  const [pipelines, setPipelines] = useState<PipelineInfo[]>([]);
  const [llmProviders, setLlmProviders] = useState<LlmProvider[] | null>(null);
  const [llmUsage, setLlmUsage] = useState<LlmUsage | null>(null);

  const sseRef = useRef<EventSource | null>(null);
  const reconnectRef = useRef(0);
  const mountedRef = useRef(true);

  const fetchJson = useCallback(
    async <T,>(path: string): Promise<T | null> => {
      try {
        const r = await fetch(`${gatewayUrl}${path}`);
        if (!r.ok) return null;
        return (await r.json()) as T;
      } catch {
        return null;
      }
    },
    [gatewayUrl],
  );

  const fetchAll = useCallback(async () => {
    const [statusData, sysData, pipeData] = await Promise.all([
      fetchJson<StatusResponse>("/api/status"),
      fetchJson<SystemStatus>("/api/system"),
      fetchJson<PipelineInfo[]>("/api/pipelines"),
    ]);
    if (!mountedRef.current) return;
    if (statusData) setStatus(statusData);
    if (sysData) setSystem(sysData);
    if (pipeData) setPipelines(pipeData);
    setConnected(statusData !== null);

    // LLM-Daten nur wenn erreichbar
    const providers = await fetchJson<LlmProvider[]>("/api/llm/providers");
    if (!mountedRef.current) return;
    if (providers) setLlmProviders(providers);

    if (appId) {
      const usage = await fetchJson<LlmUsage>(`/api/llm/usage/${appId}`);
      if (!mountedRef.current) return;
      if (usage) setLlmUsage(usage);
    }
  }, [fetchJson, appId]);

  // SSE-Verbindung
  const connectSSE = useCallback(() => {
    if (sseRef.current) sseRef.current.close();

    const sse = new EventSource(`${gatewayUrl}/api/events/stream`);
    sseRef.current = sse;

    sse.onopen = () => {
      reconnectRef.current = 0;
      if (mountedRef.current) {
        setConnected(true);
        fetchAll();
      }
    };

    sse.onerror = () => {
      sse.close();
      sseRef.current = null;
      if (!mountedRef.current) return;
      setConnected(false);

      // Exponentielles Backoff
      reconnectRef.current++;
      const delays = [1000, 2000, 4000, 8000, 16000];
      const delay =
        reconnectRef.current <= delays.length
          ? delays[reconnectRef.current - 1]
          : null;
      if (delay) {
        setTimeout(() => {
          if (mountedRef.current) connectSSE();
        }, delay);
      }
    };

    // Bei relevanten Events neu laden
    const refreshEvents = [
      "gpu_status",
      "warning_level",
      "recovery_stage",
      "pipeline_change",
      "config_reload",
    ];
    for (const evt of refreshEvents) {
      sse.addEventListener(evt, () => {
        if (mountedRef.current) fetchAll();
      });
    }
  }, [gatewayUrl, fetchAll]);

  useEffect(() => {
    mountedRef.current = true;
    injectKeyframes();

    // Initialer Fetch, dann SSE
    fetchAll().then(() => {
      if (mountedRef.current) connectSSE();
    });

    // Polling-Fallback alle 15 Sekunden
    const interval = setInterval(() => {
      if (mountedRef.current) fetchAll();
    }, 15000);

    return () => {
      mountedRef.current = false;
      clearInterval(interval);
      if (sseRef.current) {
        sseRef.current.close();
        sseRef.current = null;
      }
    };
  }, [fetchAll, connectSSE]);

  // Alle GPUs (inkl. Remote) als einheitliche Liste
  const allGpus = useMemo((): GpuStatus[] => {
    const gpus = [...(status?.gpus ?? [])];
    const remotes = status?.remote_gpus ?? [];
    for (const r of remotes) {
      gpus.push({
        pci_address: "remote",
        name: r.gpu_name ?? r.name ?? "Remote GPU",
        gpu_type: "remote",
        temperature_c: 0,
        utilization_gpu_percent: 0,
        memory_used_mb: 0,
        memory_total_mb: r.vram_mb ?? 0,
        power_draw_w: 0,
        pstate: "--",
        status: (r.status as GpuStatus["status"]) ?? "offline",
        host: r.host,
        latency_ms: r.latency_ms,
      });
    }
    return gpus;
  }, [status]);

  const warningLevel = useMemo(
    (): WarningLevel => warningFromString(status?.daemon?.warning_level),
    [status],
  );

  return {
    connected,
    status,
    system,
    pipelines,
    allGpus,
    warningLevel,
    daemon: status?.daemon ?? null,
    llmProviders,
    llmUsage,
  };
}

// ---------------------------------------------------------------------------
// Sub-Komponenten
// ---------------------------------------------------------------------------

/** Animierter Luefter (PCB-Stil) */
const Fan: FC<{
  left: number;
  color: string;
  spinning: boolean;
  reverse?: boolean;
}> = ({ left, color, spinning, reverse }) => (
  <div
    className="egpu-fan"
    style={{
      width: 14,
      height: 14,
      borderRadius: "50%",
      border: `1.5px solid ${color}`,
      position: "absolute",
      top: 5,
      left,
      animation: spinning
        ? `egpu-spin 2s linear infinite ${reverse ? "reverse" : ""}`
        : "none",
    }}
  >
    <div
      style={{
        position: "absolute",
        inset: 2,
        borderRadius: "50%",
        borderTop: `1.5px solid ${color}`,
        borderBottom: "1.5px solid transparent",
        borderLeft: "1.5px solid transparent",
        borderRight: "1.5px solid transparent",
      }}
    />
  </div>
);

/** VRAM-Balken */
const VramBar: FC<{
  used: number;
  total: number;
  color: string;
  theme: ThemeTokens;
}> = ({ used, total, color, theme }) => {
  const pct = total > 0 ? Math.min(100, Math.round((used / total) * 100)) : 0;
  return (
    <div>
      <div
        style={{
          width: "100%",
          height: 3,
          background: theme.bgDeep,
          borderRadius: 2,
          overflow: "hidden",
          marginTop: 3,
        }}
      >
        <div
          style={{
            height: "100%",
            width: `${pct}%`,
            borderRadius: 2,
            background: `linear-gradient(90deg, ${color}, ${color}88)`,
            transition: "width .6s ease",
          }}
        />
      </div>
      <div style={{ fontSize: 8, color: theme.muted, marginTop: 1 }}>
        VRAM {used > 0 ? `${Math.round(used)} / ` : ""}
        {total > 0 ? `${Math.round(total)} MB` : "N/A"}{" "}
        {pct > 0 && `(${pct}%)`}
      </div>
    </div>
  );
};

/** Stats-Balken (Temperatur, Auslastung, Power) */
const StatBar: FC<{
  label: string;
  value: string;
  pct: number;
  fillClass: string;
  valueColor?: string;
  theme: ThemeTokens;
}> = ({ label, value, pct, fillClass, valueColor, theme }) => {
  const fillColor =
    fillClass === "pwr"
      ? COLORS.tb
      : fillClass === "tmp"
        ? COLORS.amber
        : COLORS.nv;
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 2 }}>
      <div
        style={{
          fontSize: 8,
          color: theme.muted,
          letterSpacing: ".04em",
          textTransform: "uppercase" as const,
        }}
      >
        {label}
      </div>
      <div
        style={{
          fontSize: 12,
          fontWeight: 600,
          color: valueColor ?? theme.text,
          transition: "color .4s",
        }}
      >
        {value}
      </div>
      <div
        style={{
          height: 2,
          borderRadius: 1,
          background: theme.bgDeep,
          overflow: "hidden",
          marginTop: 2,
        }}
      >
        <div
          style={{
            height: "100%",
            borderRadius: 1,
            background: fillColor,
            width: `${Math.min(100, pct)}%`,
            transition: "width .6s ease",
          }}
        />
      </div>
    </div>
  );
};

/** Status-LED */
const StatusLed: FC<{
  active: boolean;
  utilization: number;
  color: string;
}> = ({ active, utilization, color }) => {
  let animation = "none";
  let boxShadow = "none";
  if (active) {
    if (utilization > 50) {
      animation = "egpu-blink-nv .6s ease-in-out infinite";
      boxShadow = `0 0 6px ${COLORS.nvGlow}`;
    } else {
      animation = "egpu-blink-nv 1.4s ease-in-out infinite";
    }
  }
  return (
    <div
      style={{
        width: 6,
        height: 6,
        borderRadius: "50%",
        background: active ? color : darkTokens.muted,
        flexShrink: 0,
        animation,
        boxShadow,
      }}
    />
  );
};

/** Verbindungslinie mit animierten Datenpunkten */
const ConnectionLine: FC<{ active: boolean }> = ({ active }) => (
  <div
    style={{ display: "flex", alignItems: "center", flexShrink: 0, width: 28 }}
  >
    <div
      style={{
        width: "100%",
        height: 1.5,
        background: darkTokens.border,
        position: "relative",
        overflow: "visible",
      }}
    >
      {active &&
        [0, 0.7, 1.4].map((delay, i) => (
          <div
            key={i}
            className="egpu-dot"
            style={{
              width: 5,
              height: 5,
              borderRadius: "50%",
              background: COLORS.tb,
              position: "absolute",
              top: "50%",
              transform: "translateY(-50%)",
              animation: `egpu-flow 2s linear infinite`,
              animationDelay: `${delay}s`,
              opacity: 0,
            }}
          />
        ))}
    </div>
  </div>
);

/** GPU-Karte im PCB-Stil */
const GpuCard: FC<{ gpu: GpuStatus; theme: ThemeTokens }> = ({
  gpu,
  theme,
}) => {
  const gType = gpuTypeOf(gpu);
  const isOnline = gpu.status === "online" || gpu.status === "available";
  const accentColor =
    gType === "egpu" ? COLORS.tb : gType === "remote" ? COLORS.purple : COLORS.nv;
  const fanColor =
    gType === "egpu"
      ? `rgba(0,176,240,.5)`
      : `rgba(118,185,0,.5)`;
  const vramPct =
    gpu.memory_total_mb > 0
      ? Math.round((gpu.memory_used_mb / gpu.memory_total_mb) * 100)
      : 0;

  return (
    <div
      style={{
        background: theme.bgCard,
        borderRadius: 10,
        padding: "10px 12px",
        display: "flex",
        flexDirection: "column",
        gap: 8,
        minWidth: 240,
        border: `.5px solid ${gpuBorderColor(gType)}`,
        opacity: isOnline ? 1 : 0.4,
        transition: "border-color .3s, opacity .3s",
      }}
    >
      {/* Oberer Bereich: PCB + Info */}
      <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
        {/* PCB Miniatur */}
        <div
          style={{
            width: 52,
            height: 36,
            borderRadius: 4,
            background: theme.bgDeep,
            border: `1px solid ${accentColor}44`,
            position: "relative",
            flexShrink: 0,
            overflow: "hidden",
          }}
        >
          <Fan left={5} color={fanColor} spinning={isOnline} />
          <Fan left={33} color={fanColor} spinning={isOnline} reverse />
          {/* PCIe-Kontakte */}
          <div
            style={{
              position: "absolute",
              bottom: 0,
              left: 0,
              right: 0,
              height: 6,
              background: `${accentColor}22`,
              borderTop: `.5px solid ${accentColor}44`,
            }}
          />
        </div>

        {/* GPU-Infos */}
        <div style={{ flex: 1, minWidth: 0 }}>
          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: 6,
              flexWrap: "wrap",
            }}
          >
            <div
              style={{
                fontSize: 8,
                fontWeight: 600,
                color: accentColor,
                letterSpacing: ".1em",
                textTransform: "uppercase" as const,
              }}
            >
              {gType === "remote" ? "LANGPU" : "NVIDIA"}
            </div>
            {gType === "egpu" && (
              <span
                style={{
                  fontSize: 8,
                  fontWeight: 600,
                  background: "rgba(0,176,240,.12)",
                  color: COLORS.tb,
                  border: ".5px solid rgba(0,176,240,.35)",
                  borderRadius: 4,
                  padding: "1px 5px",
                  letterSpacing: ".06em",
                }}
              >
                eGPU
              </span>
            )}
            {gType === "remote" && (
              <span
                style={{
                  fontSize: 8,
                  fontWeight: 600,
                  background: "rgba(168,85,247,.12)",
                  color: COLORS.purple,
                  border: ".5px solid rgba(168,85,247,.35)",
                  borderRadius: 4,
                  padding: "1px 5px",
                  letterSpacing: ".06em",
                }}
              >
                REMOTE
              </span>
            )}
          </div>
          <div
            style={{
              fontSize: 11,
              fontWeight: 700,
              color: theme.text,
              lineHeight: 1.2,
              margin: "2px 0",
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap" as const,
            }}
          >
            {gpu.name}
          </div>
          <div style={{ fontSize: 9, color: theme.muted }}>
            {gpu.pci_address !== "remote" ? gpu.pci_address : gpu.host ?? "N/A"}
            {gpu.latency_ms != null && ` · ${gpu.latency_ms}ms`}
          </div>
        </div>

        {/* Status-LED */}
        <StatusLed
          active={isOnline}
          utilization={gpu.utilization_gpu_percent}
          color={accentColor}
        />
      </div>

      {/* VRAM-Balken */}
      <VramBar
        used={gpu.memory_used_mb}
        total={gpu.memory_total_mb}
        color={accentColor}
        theme={theme}
      />

      {/* Stats-Zeile: Temp | Util | Power */}
      {isOnline && (
        <div
          style={{
            display: "grid",
            gridTemplateColumns: "1fr 1fr 1fr",
            gap: 6,
            borderTop: `.5px solid ${theme.border}`,
            paddingTop: 7,
          }}
        >
          <StatBar
            label="TEMP"
            value={`${gpu.temperature_c}\u00b0C`}
            pct={gpu.temperature_c}
            fillClass="tmp"
            valueColor={tempColor(gpu.temperature_c)}
            theme={theme}
          />
          <StatBar
            label="UTIL"
            value={`${gpu.utilization_gpu_percent}%`}
            pct={gpu.utilization_gpu_percent}
            fillClass="nv"
            theme={theme}
          />
          <StatBar
            label="POWER"
            value={`${Math.round(gpu.power_draw_w)}W`}
            pct={Math.min(100, (gpu.power_draw_w / 300) * 100)}
            fillClass="pwr"
            theme={theme}
          />
        </div>
      )}

      {/* Status-Text */}
      <div style={{ display: "flex", alignItems: "center", gap: 4, marginTop: 2 }}>
        <div
          style={{
            fontSize: 9,
            color: isOnline ? accentColor : theme.muted,
            letterSpacing: ".04em",
          }}
        >
          {isOnline
            ? `P${gpu.pstate?.replace("P", "") ?? "?"} · ${vramPct}% VRAM`
            : "Offline"}
        </div>
      </div>
    </div>
  );
};

/** Warning-Banner */
const WarningBanner: FC<{ level: WarningLevel; theme: ThemeTokens }> = ({
  level,
  theme,
}) => {
  if (level === "green") return null;

  const config: Record<
    Exclude<WarningLevel, "green">,
    { bg: string; color: string; border: string; text: string }
  > = {
    yellow: {
      bg: "rgba(245,158,11,.15)",
      color: COLORS.amber,
      border: "rgba(245,158,11,.3)",
      text: "\u26A0 Warnstufe Gelb: Erhoehte Fehlerrate auf eGPU",
    },
    orange: {
      bg: "rgba(249,115,22,.15)",
      color: COLORS.warm,
      border: "rgba(249,115,22,.3)",
      text: "\u26A0 Warnstufe Orange: GPU-Migration aktiv",
    },
    red: {
      bg: "rgba(239,68,68,.15)",
      color: COLORS.red,
      border: "rgba(239,68,68,.3)",
      text: "\u26D4 Warnstufe Rot: Recovery fehlgeschlagen",
    },
  };

  const c = config[level];
  return (
    <div
      style={{
        textAlign: "center" as const,
        padding: "6px 12px",
        borderRadius: 6,
        fontSize: 11,
        fontWeight: 600,
        marginBottom: 12,
        background: c.bg,
        color: c.color,
        border: `1px solid ${c.border}`,
      }}
    >
      {c.text}
    </div>
  );
};

/** Pipeline-Detail-Karte */
const PipelineCard: FC<{ pipeline: PipelineInfo; theme: ThemeTokens }> = ({
  pipeline,
  theme,
}) => {
  // GPU-Typ aus device-string ableiten
  const isEgpu = pipeline.gpu_device?.includes("egpu");
  const isRemote = pipeline.gpu_device?.includes("remote");
  const borderLeftColor = isEgpu
    ? COLORS.tb
    : isRemote
      ? COLORS.purple
      : COLORS.nv;

  const prio = pipeline.priority ?? 5;
  const prioColors: Record<number, { bg: string; color: string }> = {
    1: { bg: "rgba(239,68,68,.2)", color: COLORS.red },
    2: { bg: "rgba(249,115,22,.2)", color: COLORS.warm },
    3: { bg: "rgba(245,158,11,.2)", color: COLORS.amber },
    4: { bg: "rgba(118,185,0,.2)", color: COLORS.nv },
    5: { bg: "rgba(100,100,100,.2)", color: theme.muted },
  };
  const pc = prioColors[prio] ?? prioColors[5];

  return (
    <div
      style={{
        background: theme.bgCard,
        border: `.5px solid ${theme.border}`,
        borderRadius: 10,
        padding: "12px 14px",
        borderLeft: `3px solid ${borderLeftColor}`,
        transition: "border-color .3s",
      }}
    >
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 8,
          marginBottom: 8,
        }}
      >
        <div style={{ fontSize: 11, fontWeight: 700, flex: 1, color: theme.text }}>
          {pipeline.container}
        </div>
        {pipeline.project && (
          <div
            style={{
              fontSize: 8,
              color: theme.muted,
              background: theme.bgDeep,
              padding: "2px 6px",
              borderRadius: 3,
            }}
          >
            {pipeline.project}
          </div>
        )}
        <div
          style={{
            fontSize: 8,
            fontWeight: 700,
            padding: "2px 6px",
            borderRadius: 10,
            background: pc.bg,
            color: pc.color,
          }}
        >
          P{prio}
        </div>
      </div>

      {/* Workload-Tags */}
      {pipeline.workload_types && pipeline.workload_types.length > 0 && (
        <div
          style={{
            display: "flex",
            gap: 4,
            flexWrap: "wrap" as const,
            marginBottom: 6,
          }}
        >
          {pipeline.workload_types.map((w) => (
            <span
              key={w}
              style={{
                fontSize: 8,
                padding: "2px 8px",
                borderRadius: 10,
                background: theme.bgDeep,
                color: theme.muted,
                border: `.5px solid ${theme.border}`,
              }}
            >
              {w}
            </span>
          ))}
        </div>
      )}

      {/* Stats */}
      <div
        style={{
          display: "grid",
          gridTemplateColumns: "1fr 1fr",
          gap: "3px 10px",
          fontSize: 9,
          color: theme.muted,
        }}
      >
        <div>
          GPU:{" "}
          <span style={{ color: theme.text, fontWeight: 600 }}>
            {pipeline.gpu_device || "none"}
          </span>
        </div>
        <div>
          Status:{" "}
          <span style={{ color: theme.text, fontWeight: 600 }}>
            {pipeline.status}
          </span>
        </div>
        <div>
          VRAM:{" "}
          <span style={{ color: theme.text, fontWeight: 600 }}>
            {pipeline.vram_estimate_mb > 0
              ? `${pipeline.vram_estimate_mb} MB`
              : "N/A"}
          </span>
        </div>
      </div>
    </div>
  );
};

/** System-Stats-Panel */
const SystemPanel: FC<{
  system: SystemStatus | null;
  daemon: DaemonStatus | null;
  theme: ThemeTokens;
}> = ({ system, daemon, theme }) => {
  if (!system) return null;

  const ramPct =
    system.memory_total_mb > 0
      ? Math.round((system.memory_used_mb / system.memory_total_mb) * 100)
      : 0;

  const cells: Array<{
    label: string;
    value: string;
    sub?: string;
    pct?: number;
    fillColor: string;
  }> = [
    {
      label: "CPU",
      value: `${Math.round(system.cpu_percent)}%`,
      pct: system.cpu_percent,
      fillColor: COLORS.nv,
    },
    {
      label: "RAM",
      value: `${Math.round(system.memory_used_mb)} MB`,
      sub: `von ${Math.round(system.memory_total_mb)} MB (${ramPct}%)`,
      pct: ramPct,
      fillColor: COLORS.tb,
    },
  ];

  if (system.swap_total_mb && system.swap_total_mb > 0) {
    const swapPct = Math.round(
      ((system.swap_used_mb ?? 0) / system.swap_total_mb) * 100,
    );
    cells.push({
      label: "SWAP",
      value: `${Math.round(system.swap_used_mb ?? 0)} MB`,
      sub: `von ${Math.round(system.swap_total_mb)} MB`,
      pct: swapPct,
      fillColor: COLORS.amber,
    });
  }

  cells.push({
    label: "UPTIME",
    value: formatUptime(system.uptime_seconds),
    sub: system.hostname,
    fillColor: COLORS.purple,
  });

  if (daemon?.recovery_active) {
    cells.push({
      label: "RECOVERY",
      value: daemon.recovery_stage ?? "aktiv",
      fillColor: COLORS.red,
    });
  }

  return (
    <div
      style={{
        marginTop: 20,
        background: theme.bgCard,
        border: `.5px solid ${theme.border}`,
        borderRadius: 12,
        overflow: "hidden",
      }}
    >
      {/* Header */}
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 8,
          padding: "10px 16px",
          borderBottom: `.5px solid ${theme.border}`,
          background: theme.bgDeep,
        }}
      >
        <svg
          width="12"
          height="12"
          viewBox="0 0 24 24"
          fill="none"
          style={{ flexShrink: 0 }}
        >
          <rect
            x="2"
            y="3"
            width="20"
            height="14"
            rx="2"
            stroke="currentColor"
            strokeWidth="1.5"
          />
          <path
            d="M8 21h8M12 17v4"
            stroke="currentColor"
            strokeWidth="1.5"
            strokeLinecap="round"
          />
        </svg>
        <div
          style={{
            fontSize: 10,
            fontWeight: 600,
            letterSpacing: ".1em",
            textTransform: "uppercase" as const,
            color: theme.muted,
          }}
        >
          System
        </div>
      </div>

      {/* Grid */}
      <div
        style={{
          display: "grid",
          gridTemplateColumns: "repeat(auto-fit, minmax(130px, 1fr))",
          gap: 0,
        }}
      >
        {cells.map((cell, i) => (
          <div
            key={cell.label}
            style={{
              padding: "12px 14px",
              borderRight:
                i < cells.length - 1
                  ? `.5px solid ${theme.border}`
                  : "none",
              borderBottom: `.5px solid ${theme.border}`,
              display: "flex",
              flexDirection: "column",
              gap: 4,
            }}
          >
            <div
              style={{
                fontSize: 8,
                color: theme.muted,
                letterSpacing: ".06em",
                textTransform: "uppercase" as const,
              }}
            >
              {cell.label}
            </div>
            <div
              style={{
                fontSize: 13,
                fontWeight: 600,
                color: theme.text,
                whiteSpace: "nowrap" as const,
              }}
            >
              {cell.value}
            </div>
            {cell.sub && (
              <div
                style={{
                  fontSize: 9,
                  color: theme.muted,
                  whiteSpace: "nowrap" as const,
                }}
              >
                {cell.sub}
              </div>
            )}
            {cell.pct !== undefined && (
              <div
                style={{
                  height: 2,
                  borderRadius: 1,
                  background: theme.bgDeep,
                  overflow: "hidden",
                  marginTop: 3,
                }}
              >
                <div
                  style={{
                    height: "100%",
                    borderRadius: 1,
                    width: `${Math.min(100, cell.pct)}%`,
                    background: cell.fillColor,
                    transition: "width .6s ease",
                  }}
                />
              </div>
            )}
          </div>
        ))}
      </div>
    </div>
  );
};

/** LLM-Provider-Statusanzeige */
const LlmPanel: FC<{
  providers: LlmProvider[] | null;
  usage: LlmUsage | null;
  appId: string;
  theme: ThemeTokens;
}> = ({ providers, usage, appId, theme }) => {
  if (!providers || providers.length === 0) return null;

  return (
    <div
      style={{
        marginTop: 16,
        background: theme.bgCard,
        border: `.5px solid ${theme.border}`,
        borderRadius: 12,
        overflow: "hidden",
      }}
    >
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 8,
          padding: "10px 16px",
          borderBottom: `.5px solid ${theme.border}`,
          background: theme.bgDeep,
        }}
      >
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none">
          <path
            d="M21 11.5a8.4 8.4 0 01-.9 3.8 8.5 8.5 0 01-7.6 4.7 8.4 8.4 0 01-3.8-.9L3 21l1.9-5.7a8.4 8.4 0 01-.9-3.8 8.5 8.5 0 014.7-7.6 8.4 8.4 0 013.8-.9h.5a8.5 8.5 0 018 8v.5z"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.5"
          />
        </svg>
        <div
          style={{
            fontSize: 10,
            fontWeight: 600,
            letterSpacing: ".1em",
            textTransform: "uppercase" as const,
            color: theme.muted,
          }}
        >
          LLM Gateway
        </div>
        {usage && (
          <div
            style={{
              marginLeft: "auto",
              fontSize: 9,
              color: COLORS.purple,
              fontWeight: 600,
            }}
          >
            {appId}: ${usage.total_cost_usd.toFixed(4)} ({usage.total_requests}{" "}
            req)
          </div>
        )}
      </div>

      <div
        style={{
          display: "grid",
          gridTemplateColumns: "repeat(auto-fit, minmax(160px, 1fr))",
          gap: 0,
        }}
      >
        {providers.map((p) => {
          const isActive =
            p.status === "online" || p.status === "active" || p.status === "ready";
          return (
            <div
              key={p.name}
              style={{
                padding: "10px 14px",
                borderRight: `.5px solid ${theme.border}`,
                borderBottom: `.5px solid ${theme.border}`,
                display: "flex",
                flexDirection: "column",
                gap: 3,
              }}
            >
              <div
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: 6,
                }}
              >
                <div
                  style={{
                    width: 5,
                    height: 5,
                    borderRadius: "50%",
                    background: isActive ? COLORS.nv : theme.muted,
                  }}
                />
                <div
                  style={{
                    fontSize: 10,
                    fontWeight: 600,
                    color: theme.text,
                  }}
                >
                  {p.name}
                </div>
              </div>
              {p.model && (
                <div style={{ fontSize: 8, color: theme.muted }}>{p.model}</div>
              )}
              <div style={{ fontSize: 8, color: theme.muted }}>
                {p.status}
                {p.requests_total != null && ` · ${p.requests_total} req`}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
};

// ---------------------------------------------------------------------------
// Haupt-Komponente
// ---------------------------------------------------------------------------

export const EgpuPipelineWidget: FC<EgpuPipelineWidgetProps> = ({
  gatewayUrl = "http://localhost:7842",
  appId,
  compact = false,
}) => {
  const theme = useTheme();
  const {
    connected,
    system,
    pipelines,
    allGpus,
    warningLevel,
    daemon,
    llmProviders,
    llmUsage,
  } = useEgpuData(gatewayUrl, appId);

  const rootStyle: CSSProperties = {
    width: "100%",
    maxWidth: compact ? 600 : 1100,
    margin: "0 auto",
    fontFamily:
      "'Segoe UI', system-ui, -apple-system, sans-serif",
    color: theme.text,
  };

  const monoFont =
    "'JetBrains Mono', 'Cascadia Code', 'Fira Code', monospace";

  return (
    <div style={rootStyle}>
      {/* ── Header ─────────────────────────────────────────── */}
      <div
        style={{
          textAlign: "center" as const,
          fontSize: 13,
          fontWeight: 600,
          letterSpacing: ".12em",
          textTransform: "uppercase" as const,
          color: theme.muted,
          marginBottom: 4,
        }}
      >
        eGPU Manager · Pipeline Monitor
      </div>

      {/* Live-Status */}
      <div
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          gap: 8,
          marginBottom: 18,
          fontSize: 10,
          color: theme.muted,
        }}
      >
        {/* Live-Dot */}
        <div
          style={{
            width: 6,
            height: 6,
            borderRadius: "50%",
            background: connected ? COLORS.nv : theme.muted,
            animation: connected
              ? "egpu-blink-nv .8s ease-in-out infinite"
              : "none",
            boxShadow: connected ? `0 0 5px ${COLORS.nvGlow}` : "none",
          }}
        />
        <span style={{ fontFamily: monoFont, fontSize: 10 }}>
          {connected
            ? "Live \u00b7 GPU-Pipeline aktiv"
            : "Offline \u00b7 Daemon nicht erreichbar"}
        </span>
        {daemon?.uptime_seconds != null && (
          <span
            style={{
              fontFamily: monoFont,
              fontSize: 10,
              opacity: 0.5,
            }}
          >
            Uptime: {formatUptime(daemon.uptime_seconds)}
          </span>
        )}
      </div>

      {/* Warning-Banner */}
      <WarningBanner level={warningLevel} theme={theme} />

      {/* ── GPU-Karten ─────────────────────────────────────── */}
      <div
        style={{
          display: "grid",
          gridTemplateColumns: compact
            ? "1fr"
            : "repeat(auto-fit, minmax(280px, 1fr))",
          gap: 10,
          marginBottom: 16,
        }}
      >
        {allGpus.map((gpu, i) => (
          <GpuCard key={gpu.pci_address + i} gpu={gpu} theme={theme} />
        ))}
      </div>

      {/* Keine GPUs gefunden */}
      {allGpus.length === 0 && connected && (
        <div
          style={{
            textAlign: "center" as const,
            padding: 24,
            color: theme.muted,
            fontSize: 12,
          }}
        >
          Keine GPUs erkannt.
        </div>
      )}

      {/* Offline-Hinweis */}
      {!connected && (
        <div
          style={{
            textAlign: "center" as const,
            padding: "24px 16px",
            background: theme.bgCard,
            borderRadius: 10,
            border: `.5px solid ${theme.border}`,
            color: theme.muted,
            fontSize: 11,
            marginBottom: 16,
          }}
        >
          <div style={{ fontSize: 24, marginBottom: 8 }}>&#9888;</div>
          <div style={{ fontWeight: 600, marginBottom: 4 }}>
            eGPU Manager nicht erreichbar
          </div>
          <div>
            Verbindung zu{" "}
            <span style={{ fontFamily: monoFont, color: COLORS.tb }}>
              {gatewayUrl}
            </span>{" "}
            fehlgeschlagen.
          </div>
          <div style={{ marginTop: 4 }}>
            Automatische Wiederverbindung aktiv...
          </div>
        </div>
      )}

      {/* ── Pipeline-Details (nur wenn nicht compact) ──────── */}
      {!compact && pipelines.length > 0 && (
        <div style={{ marginTop: 20 }}>
          <div
            style={{
              fontSize: 10,
              fontWeight: 600,
              letterSpacing: ".1em",
              textTransform: "uppercase" as const,
              color: theme.muted,
              marginBottom: 10,
              display: "flex",
              alignItems: "center",
              gap: 8,
            }}
          >
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none">
              <rect
                x="2"
                y="3"
                width="8"
                height="5"
                rx="1"
                stroke="currentColor"
                strokeWidth="1.5"
              />
              <rect
                x="14"
                y="3"
                width="8"
                height="5"
                rx="1"
                stroke="currentColor"
                strokeWidth="1.5"
              />
              <rect
                x="2"
                y="16"
                width="8"
                height="5"
                rx="1"
                stroke="currentColor"
                strokeWidth="1.5"
              />
              <rect
                x="14"
                y="16"
                width="8"
                height="5"
                rx="1"
                stroke="currentColor"
                strokeWidth="1.5"
              />
              <path
                d="M6 8v3h12V8M12 11v5"
                stroke="currentColor"
                strokeWidth="1.5"
              />
            </svg>
            Pipeline-Details ({pipelines.length})
          </div>
          <div
            style={{
              display: "grid",
              gridTemplateColumns:
                "repeat(auto-fit, minmax(280px, 1fr))",
              gap: 10,
            }}
          >
            {pipelines.map((p, i) => (
              <PipelineCard key={p.container + i} pipeline={p} theme={theme} />
            ))}
          </div>
        </div>
      )}

      {/* ── System-Stats ───────────────────────────────────── */}
      <SystemPanel system={system} daemon={daemon} theme={theme} />

      {/* ── LLM-Provider (nur wenn appId gesetzt) ─────────── */}
      {appId && (
        <LlmPanel
          providers={llmProviders}
          usage={llmUsage}
          appId={appId}
          theme={theme}
        />
      )}

      {/* ── Legende ────────────────────────────────────────── */}
      {!compact && (
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 16,
            justifyContent: "center",
            marginTop: 16,
            flexWrap: "wrap" as const,
          }}
        >
          {[
            { color: COLORS.tb, label: "Datenfluss" },
            { color: COLORS.nv, label: "GPU aktiv (NVIDIA)" },
            { color: COLORS.amber, label: "Host (NUC)" },
            { color: COLORS.purple, label: "Remote (LanGPU)" },
          ].map((item) => (
            <div
              key={item.label}
              style={{
                display: "flex",
                alignItems: "center",
                gap: 5,
                fontSize: 10,
                color: theme.muted,
              }}
            >
              <div
                style={{
                  width: 6,
                  height: 6,
                  borderRadius: "50%",
                  background: item.color,
                  flexShrink: 0,
                }}
              />
              {item.label}
            </div>
          ))}
        </div>
      )}
    </div>
  );
};

export default EgpuPipelineWidget;
