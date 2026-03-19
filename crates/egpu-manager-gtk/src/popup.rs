use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{
    Align, Box as GtkBox, Button, CssProvider, Image, Label, LevelBar, Notebook, Orientation,
    Separator, StyleContext, Window, WindowType, ScrolledWindow,
};

use crate::state::{ConnectionState, GpuInfo, HealthScoreInfo, PipelineInfo, RemoteGpuInfo, WidgetState};

// Shared state to remember the active tab across redraws.
thread_local! {
    static ACTIVE_TAB: Rc<Cell<u32>> = Rc::new(Cell::new(0));
}

const CSS: &str = r#"
window {
    background-color: #1a1a18;
    color: #e8e7e0;
}
.popup-container {
    padding: 12px;
}
.section-title {
    font-size: 10px;
    font-weight: bold;
    color: #9c9a92;
    margin-bottom: 8px;
}
.status-bar {
    padding: 8px 12px;
    border-radius: 8px;
    margin-bottom: 8px;
}
.status-green { background-color: rgba(34,197,94,0.15); }
.status-yellow { background-color: rgba(234,179,8,0.15); }
.status-orange { background-color: rgba(249,115,22,0.15); }
.status-red { background-color: rgba(239,68,68,0.15); }
.status-gray { background-color: rgba(107,114,128,0.15); }
.status-label {
    font-size: 11px;
    font-weight: bold;
}
.green { color: #22c55e; }
.yellow { color: #eab308; }
.orange { color: #f97316; }
.red { color: #ef4444; }
.muted { color: #9c9a92; }
.gpu-card {
    background-color: #2a2a27;
    border-radius: 8px;
    padding: 10px 12px;
    margin-bottom: 6px;
}
.gpu-name {
    font-size: 11px;
    font-weight: bold;
}
.gpu-badge {
    font-size: 9px;
    font-weight: bold;
    padding: 2px 8px;
    border-radius: 10px;
}
.badge-egpu { background-color: rgba(0,176,240,0.15); color: #00b0f0; }
.badge-internal { background-color: rgba(118,185,0,0.15); color: #76b900; }
.badge-remote { background-color: rgba(168,85,247,0.15); color: #a855f7; }
.gpu-stat {
    font-size: 10px;
    color: #9c9a92;
}
.gpu-stat-val {
    font-size: 10px;
    font-weight: bold;
    color: #e8e7e0;
}
.pipe-card {
    background-color: #2a2a27;
    border-radius: 6px;
    padding: 8px 10px;
    margin-bottom: 4px;
}
.pipe-name { font-size: 10px; font-weight: bold; }
.pipe-project { font-size: 9px; color: #9c9a92; background-color: #1e1e1c; padding: 1px 6px; border-radius: 4px; }
.pipe-reason { font-size: 9px; color: #9c9a92; font-style: italic; }
.prio-badge { font-size: 9px; font-weight: bold; padding: 1px 6px; border-radius: 8px; }
.prio-1 { background-color: rgba(239,68,68,0.2); color: #ef4444; }
.prio-2 { background-color: rgba(249,115,22,0.2); color: #f97316; }
.prio-3 { background-color: rgba(234,179,8,0.2); color: #eab308; }
.prio-4 { background-color: rgba(118,185,0,0.2); color: #76b900; }
.prio-5 { background-color: rgba(107,114,128,0.2); color: #9c9a92; }
.open-btn { background-color: rgba(0,176,240,0.15); color: #00b0f0; border-radius: 8px; padding: 8px 16px; font-size: 11px; font-weight: bold; }
.conn-status { font-size: 9px; color: #9c9a92; }
.health-card { background-color: #2a2a27; border-radius: 8px; padding: 8px 12px; margin-bottom: 6px; }
.health-label { font-size: 10px; color: #9c9a92; }
.health-score { font-size: 14px; font-weight: bold; }
.health-events { font-size: 9px; color: #9c9a92; }
.tab-label { font-size: 10px; font-weight: bold; }
.llm-card { background-color: #2a2a27; border-radius: 6px; padding: 10px 12px; margin-bottom: 6px; }
.llm-endpoint { font-size: 9px; color: #9c9a92; font-family: monospace; }
.llm-stat { font-size: 10px; color: #e8e7e0; }
notebook header { background-color: #1e1e1c; }
notebook tab { padding: 4px 12px; color: #9c9a92; font-size: 10px; }
notebook tab:checked { color: #00b0f0; border-bottom: 2px solid #00b0f0; }
levelbar trough { min-height: 3px; border-radius: 2px; background-color: #1e1e1c; }
levelbar block.filled { border-radius: 2px; background-color: #76b900; min-height: 3px; }
separator { background-color: #444440; min-height: 1px; margin-top: 8px; margin-bottom: 8px; }
"#;

pub fn build_popup() -> Window {
    let provider = CssProvider::new();
    provider
        .load_from_data(CSS.as_bytes())
        .expect("CSS load");
    StyleContext::add_provider_for_screen(
        &gtk::gdk::Screen::default().expect("screen"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let window = Window::new(WindowType::Toplevel);
    window.set_title("eGPU Manager");
    window.set_default_size(380, 520);
    window.set_resizable(false);
    window.set_type_hint(gtk::gdk::WindowTypeHint::Dialog);
    window.set_skip_taskbar_hint(true);

    window.connect_delete_event(|w, _| {
        w.hide();
        glib::Propagation::Stop
    });

    let container = GtkBox::new(Orientation::Vertical, 0);
    container.style_context().add_class("popup-container");

    let loading = Label::new(Some("Verbinde mit Daemon..."));
    loading.style_context().add_class("muted");
    container.pack_start(&loading, false, false, 0);

    window.add(&container);
    window
}

pub fn update_popup(window: &Window, state: &WidgetState) {
    // Save current tab before destroying
    let saved_tab = ACTIVE_TAB.with(|t| t.get());

    if let Some(child) = window.children().first() {
        window.remove(child);
    }

    let outer = GtkBox::new(Orientation::Vertical, 0);
    outer.style_context().add_class("popup-container");

    // ── Status bar (always visible, above tabs) ──
    let status_box = GtkBox::new(Orientation::Horizontal, 8);
    let color = state.warning_color();
    status_box.style_context().add_class("status-bar");
    status_box.style_context().add_class(&format!("status-{color}"));

    let dot = Label::new(Some(match color {
        "green" => "\u{25CF}",
        "yellow" | "orange" => "\u{26A0}",
        "red" => "\u{26D4}",
        _ => "\u{25CB}",
    }));
    dot.style_context().add_class(color);
    status_box.pack_start(&dot, false, false, 0);

    if let Some(ref d) = state.daemon {
        let lvl = Label::new(Some(&format!("Warnstufe: {}", d.warning_level)));
        lvl.style_context().add_class("status-label");
        status_box.pack_start(&lvl, false, false, 0);

        if let Some(ref hs) = state.health_score {
            let score_lbl = Label::new(Some(&format!("Health: {:.0}", hs.score)));
            score_lbl.style_context().add_class("gpu-stat-val");
            if hs.score >= 80.0 {
                score_lbl.style_context().add_class("green");
            } else if hs.score >= 60.0 {
                score_lbl.style_context().add_class("yellow");
            } else {
                score_lbl.style_context().add_class("red");
            }
            status_box.pack_start(&score_lbl, false, false, 0);
        }

        let queue = Label::new(Some(&format!("Q:{}", d.scheduler_queue_length)));
        queue.style_context().add_class("muted");
        queue.set_halign(Align::End);
        queue.set_hexpand(true);
        status_box.pack_start(&queue, true, true, 0);
    } else {
        let lbl = Label::new(Some("Daemon nicht verbunden"));
        lbl.style_context().add_class("muted");
        status_box.pack_start(&lbl, false, false, 0);
    }
    outer.pack_start(&status_box, false, false, 0);

    // ── Notebook (Tabs) ──
    let notebook = Notebook::new();
    notebook.set_tab_pos(gtk::PositionType::Top);

    // Tab 1: GPU Status
    let tab1 = build_tab_status(state);
    let tab1_box = make_tab_label("video-display", "GPU Status");
    notebook.append_page(&tab1, Some(&tab1_box));

    // Tab 2: Pipelines
    let tab2 = build_tab_pipelines(state);
    let tab2_box = make_tab_label("system-run", &format!("Pipelines ({})", state.pipelines.len()));
    notebook.append_page(&tab2, Some(&tab2_box));

    // Tab 3: LLM Gateway
    let tab3 = build_tab_llm(state);
    let tab3_box = make_tab_label("network-server", "LLM Gateway");
    notebook.append_page(&tab3, Some(&tab3_box));

    notebook.set_vexpand(true);

    // Restore saved tab
    notebook.set_current_page(Some(saved_tab));

    // Save tab on switch
    notebook.connect_switch_page(|_, _, page_num| {
        ACTIVE_TAB.with(|t| t.set(page_num));
    });

    outer.pack_start(&notebook, true, true, 0);

    // ── Footer: Open Web UI + Connection status ──
    let footer = GtkBox::new(Orientation::Horizontal, 8);
    footer.set_margin_top(8);

    let btn = Button::with_label("Web-UI");
    btn.style_context().add_class("open-btn");
    btn.connect_clicked(|_| {
        let _ = open::that("http://127.0.0.1:7842");
    });
    footer.pack_start(&btn, false, false, 0);

    let conn_text = match &state.connection {
        ConnectionState::Connected => "\u{25CF} Verbunden".to_string(),
        ConnectionState::Connecting => "\u{25CB} Verbinde...".to_string(),
        ConnectionState::Reconnecting(n) => format!("\u{21BB} Reconnect #{n}"),
        ConnectionState::Error(e) => format!("\u{2715} {e}"),
    };
    let conn_label = Label::new(Some(&conn_text));
    conn_label.style_context().add_class("conn-status");
    conn_label.set_halign(Align::End);
    conn_label.set_hexpand(true);
    footer.pack_start(&conn_label, true, true, 0);

    outer.pack_start(&footer, false, false, 0);

    window.add(&outer);
    window.show_all();
}

// ── Tab 1: GPU Status ──

fn build_tab_status(state: &WidgetState) -> ScrolledWindow {
    let content = GtkBox::new(Orientation::Vertical, 4);
    content.set_margin_start(8);
    content.set_margin_end(8);
    content.set_margin_top(8);

    // Health Score bar
    if let Some(ref hs) = state.health_score {
        content.pack_start(&build_health_score_card(hs), false, false, 0);
    }

    // GPUs
    for gpu in &state.gpus {
        content.pack_start(&build_gpu_card(gpu), false, false, 0);
    }

    // Remote GPUs
    for rgpu in &state.remote_gpus {
        content.pack_start(&build_remote_gpu_card(rgpu), false, false, 0);
    }

    if state.remote_gpus.is_empty() {
        let placeholder = GtkBox::new(Orientation::Horizontal, 6);
        placeholder.style_context().add_class("gpu-card");
        let icon = Image::from_icon_name(Some("network-offline"), gtk::IconSize::Menu);
        placeholder.pack_start(&icon, false, false, 0);
        let lbl = Label::new(Some("LanGPU — nicht registriert"));
        lbl.style_context().add_class("muted");
        placeholder.pack_start(&lbl, false, false, 0);
        content.pack_start(&placeholder, false, false, 0);
    }

    let scroll = ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    scroll.add(&content);
    scroll
}

// ── Tab 2: Pipelines (ALLE) ──

fn build_tab_pipelines(state: &WidgetState) -> ScrolledWindow {
    let content = GtkBox::new(Orientation::Vertical, 4);
    content.set_margin_start(8);
    content.set_margin_end(8);
    content.set_margin_top(8);

    if state.pipelines.is_empty() {
        let empty = Label::new(Some("Keine Pipelines konfiguriert"));
        empty.style_context().add_class("muted");
        empty.set_margin_top(20);
        content.pack_start(&empty, false, false, 0);
    } else {
        let mut sorted: Vec<&PipelineInfo> = state.pipelines.iter().collect();
        sorted.sort_by_key(|p| p.priority);

        for pipe in &sorted {
            content.pack_start(&build_pipeline_card(pipe), false, false, 0);
        }

        // Summary
        content.pack_start(&Separator::new(Orientation::Horizontal), false, false, 0);

        let summary = GtkBox::new(Orientation::Horizontal, 12);
        let total = Label::new(Some(&format!("{} Pipelines", state.pipelines.len())));
        total.style_context().add_class("gpu-stat");
        summary.pack_start(&total, false, false, 0);

        let on_egpu = sorted.iter().filter(|p| p.gpu_type == "egpu").count();
        let on_internal = sorted.iter().filter(|p| p.gpu_type == "internal").count();
        let status_text = format!("eGPU: {}  Intern: {}", on_egpu, on_internal);
        let status_lbl = Label::new(Some(&status_text));
        status_lbl.style_context().add_class("gpu-stat-val");
        status_lbl.set_halign(Align::End);
        status_lbl.set_hexpand(true);
        summary.pack_start(&status_lbl, true, true, 0);

        content.pack_start(&summary, false, false, 0);
    }

    let scroll = ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    scroll.add(&content);
    scroll
}

// ── Tab 3: LLM Gateway ──

fn build_tab_llm(state: &WidgetState) -> ScrolledWindow {
    let content = GtkBox::new(Orientation::Vertical, 4);
    content.set_margin_start(8);
    content.set_margin_end(8);
    content.set_margin_top(8);

    let card = GtkBox::new(Orientation::Vertical, 4);
    card.style_context().add_class("llm-card");

    let header = GtkBox::new(Orientation::Horizontal, 8);
    let title = Label::new(Some("LLM Gateway"));
    title.style_context().add_class("gpu-name");
    header.pack_start(&title, false, false, 0);

    let connected = state.daemon.is_some();
    let status_text = if connected { "\u{25CF} Aktiv" } else { "\u{25CB} Offline" };
    let status = Label::new(Some(status_text));
    status.style_context().add_class(if connected { "green" } else { "muted" });
    status.set_halign(Align::End);
    status.set_hexpand(true);
    header.pack_start(&status, true, true, 0);
    card.pack_start(&header, false, false, 0);

    let endpoint = Label::new(Some("http://localhost:7842/api/llm/chat/completions"));
    endpoint.style_context().add_class("llm-endpoint");
    endpoint.set_halign(Align::Start);
    endpoint.set_ellipsize(gtk::pango::EllipsizeMode::End);
    endpoint.set_max_width_chars(45);
    card.pack_start(&endpoint, false, false, 0);

    content.pack_start(&card, false, false, 0);

    // Connected apps
    let apps_title = Label::new(Some("ANGEBUNDENE APPS"));
    apps_title.style_context().add_class("section-title");
    apps_title.set_halign(Align::Start);
    apps_title.set_margin_top(8);
    content.pack_start(&apps_title, false, false, 0);

    let apps = [
        ("audit_designer", "Vue 3", "Prüfungs-Designer"),
        ("flowinvoice", "React", "Rechnungsverarbeitung"),
        ("flownavigator", "Vue 3", "Modul-Konverter"),
        ("regulierung", "React", "Regulierungs-Analyse"),
        ("flowaudit", "React", "Vorhabensprüfung"),
    ];

    for (app_id, framework, desc) in &apps {
        let row = GtkBox::new(Orientation::Horizontal, 8);
        row.style_context().add_class("pipe-card");

        let name = Label::new(Some(app_id));
        name.style_context().add_class("pipe-name");
        row.pack_start(&name, false, false, 0);

        let fw = Label::new(Some(framework));
        fw.style_context().add_class("pipe-project");
        row.pack_start(&fw, false, false, 0);

        let description = Label::new(Some(desc));
        description.style_context().add_class("gpu-stat");
        description.set_halign(Align::End);
        description.set_hexpand(true);
        description.set_ellipsize(gtk::pango::EllipsizeMode::End);
        row.pack_start(&description, true, true, 0);

        content.pack_start(&row, false, false, 0);
    }

    // Supported providers
    content.pack_start(&Separator::new(Orientation::Horizontal), false, false, 0);

    let prov_title = Label::new(Some("LLM PROVIDER"));
    prov_title.style_context().add_class("section-title");
    prov_title.set_halign(Align::Start);
    content.pack_start(&prov_title, false, false, 0);

    let providers = [
        ("Ollama (Lokal)", "qwen3:14b", "badge-egpu"),
        ("Anthropic", "Claude Sonnet/Opus", "muted"),
        ("OpenAI", "GPT-4o", "muted"),
        ("Gemini", "1.5 Pro/Flash", "muted"),
    ];

    for (name, model, badge_class) in &providers {
        let row = GtkBox::new(Orientation::Horizontal, 8);

        let name_lbl = Label::new(Some(name));
        name_lbl.style_context().add_class("gpu-stat-val");
        row.pack_start(&name_lbl, false, false, 0);

        let model_lbl = Label::new(Some(model));
        model_lbl.style_context().add_class("gpu-stat");
        model_lbl.set_halign(Align::End);
        model_lbl.set_hexpand(true);
        row.pack_start(&model_lbl, true, true, 0);

        let dot = Label::new(Some("\u{25CF}"));
        dot.style_context().add_class(badge_class);
        row.pack_start(&dot, false, false, 0);

        content.pack_start(&row, false, false, 0);
    }

    let scroll = ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    scroll.add(&content);
    scroll
}

// ── Helpers ──

fn make_tab_label(icon_name: &str, text: &str) -> GtkBox {
    let hbox = GtkBox::new(Orientation::Horizontal, 4);
    let icon = Image::from_icon_name(Some(icon_name), gtk::IconSize::SmallToolbar);
    hbox.pack_start(&icon, false, false, 0);
    let label = Label::new(Some(text));
    label.style_context().add_class("tab-label");
    hbox.pack_start(&label, false, false, 0);
    hbox.show_all();
    hbox
}

fn gpu_icon_name(gpu_type: &str) -> &'static str {
    match gpu_type {
        "egpu" => "thunderbolt-symbolic",
        "internal" => "video-display",
        "remote" => "network-workgroup",
        _ => "computer",
    }
}

// ── Card builders ──

fn build_gpu_card(gpu: &GpuInfo) -> GtkBox {
    let card = GtkBox::new(Orientation::Vertical, 4);
    card.style_context().add_class("gpu-card");

    let header = GtkBox::new(Orientation::Horizontal, 6);

    // GPU icon
    let icon = Image::from_icon_name(Some(gpu_icon_name(&gpu.gpu_type)), gtk::IconSize::Menu);
    header.pack_start(&icon, false, false, 0);

    let name = Label::new(Some(&gpu.name.replace("NVIDIA GeForce ", "")));
    name.style_context().add_class("gpu-name");
    name.set_hexpand(true);
    name.set_halign(Align::Start);
    header.pack_start(&name, true, true, 0);

    let badge_text = match gpu.gpu_type.as_str() {
        "egpu" => "eGPU",
        "internal" => "Intern",
        "remote" => "Remote",
        _ => "\u{2014}",
    };
    let badge = Label::new(Some(badge_text));
    badge.style_context().add_class("gpu-badge");
    badge.style_context().add_class(match gpu.gpu_type.as_str() {
        "egpu" => "badge-egpu",
        "internal" => "badge-internal",
        "remote" => "badge-remote",
        _ => "muted",
    });
    header.pack_start(&badge, false, false, 0);
    card.pack_start(&header, false, false, 0);

    let stats = GtkBox::new(Orientation::Horizontal, 12);

    let temp_lbl = Label::new(Some(&format!("{}\u{00B0}C", gpu.temperature_c)));
    temp_lbl.style_context().add_class("gpu-stat-val");
    if gpu.temperature_c >= 80 {
        temp_lbl.style_context().add_class("red");
    } else if gpu.temperature_c >= 65 {
        temp_lbl.style_context().add_class("orange");
    } else {
        temp_lbl.style_context().add_class("green");
    }
    stats.pack_start(&temp_lbl, false, false, 0);

    let util_lbl = Label::new(Some(&format!("{}%", gpu.utilization_gpu_percent)));
    util_lbl.style_context().add_class("gpu-stat-val");
    stats.pack_start(&util_lbl, false, false, 0);

    let pwr_lbl = Label::new(Some(&format!("{:.0}W", gpu.power_draw_w)));
    pwr_lbl.style_context().add_class("gpu-stat");
    stats.pack_start(&pwr_lbl, false, false, 0);

    let vram_text = Label::new(Some(&format!(
        "{}/{}MB",
        gpu.memory_used_mb, gpu.memory_total_mb
    )));
    vram_text.style_context().add_class("gpu-stat");
    vram_text.set_halign(Align::End);
    vram_text.set_hexpand(true);
    stats.pack_start(&vram_text, true, true, 0);

    card.pack_start(&stats, false, false, 0);

    if gpu.memory_total_mb > 0 {
        let pct = gpu.memory_used_mb as f64 / gpu.memory_total_mb as f64;
        let bar = LevelBar::for_interval(0.0, 1.0);
        bar.set_value(pct);
        bar.set_margin_top(2);
        card.pack_start(&bar, false, false, 0);
    }

    card
}

fn build_pipeline_card(pipe: &PipelineInfo) -> GtkBox {
    let card = GtkBox::new(Orientation::Vertical, 3);
    card.style_context().add_class("pipe-card");

    let header = GtkBox::new(Orientation::Horizontal, 6);

    let icon = Image::from_icon_name(Some("system-run-symbolic"), gtk::IconSize::Menu);
    header.pack_start(&icon, false, false, 0);

    let name = Label::new(Some(&pipe.container));
    name.style_context().add_class("pipe-name");
    name.set_hexpand(true);
    name.set_halign(Align::Start);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.set_max_width_chars(20);
    header.pack_start(&name, true, true, 0);

    let proj = Label::new(Some(&pipe.project));
    proj.style_context().add_class("pipe-project");
    header.pack_start(&proj, false, false, 0);

    let prio = Label::new(Some(&format!("P{}", pipe.priority)));
    prio.style_context().add_class("prio-badge");
    prio.style_context().add_class(&format!("prio-{}", pipe.priority.min(5)));
    header.pack_start(&prio, false, false, 0);

    let gpu_lbl = Label::new(Some(match pipe.gpu_type.as_str() {
        "egpu" => "eGPU",
        "internal" => "Intern",
        "remote" => "Remote",
        _ => "\u{2014}",
    }));
    gpu_lbl.style_context().add_class("gpu-badge");
    gpu_lbl.style_context().add_class(match pipe.gpu_type.as_str() {
        "egpu" => "badge-egpu",
        "internal" => "badge-internal",
        "remote" => "badge-remote",
        _ => "muted",
    });
    header.pack_start(&gpu_lbl, false, false, 0);

    card.pack_start(&header, false, false, 0);

    let info_row = GtkBox::new(Orientation::Horizontal, 8);
    let wl_text = if pipe.workload_types.is_empty() {
        "\u{2014}".to_string()
    } else {
        pipe.workload_types.join(", ")
    };
    let wl = Label::new(Some(&wl_text));
    wl.style_context().add_class("gpu-stat");
    info_row.pack_start(&wl, false, false, 0);

    let vram = pipe.actual_vram_mb.unwrap_or(pipe.vram_estimate_mb);
    let vram_lbl = Label::new(Some(&format!("{vram} MB")));
    vram_lbl.style_context().add_class("gpu-stat-val");
    vram_lbl.set_halign(Align::End);
    vram_lbl.set_hexpand(true);
    info_row.pack_start(&vram_lbl, true, true, 0);

    card.pack_start(&info_row, false, false, 0);

    if let Some(ref reason) = pipe.decision_reason {
        if reason != "n/a" {
            let source = pipe.assignment_source.as_deref().unwrap_or("auto");
            let reason_lbl = Label::new(Some(&format!("{source}: {reason}")));
            reason_lbl.style_context().add_class("pipe-reason");
            reason_lbl.set_halign(Align::Start);
            reason_lbl.set_ellipsize(gtk::pango::EllipsizeMode::End);
            reason_lbl.set_max_width_chars(40);
            card.pack_start(&reason_lbl, false, false, 0);
        }
    }

    card
}

fn build_health_score_card(hs: &HealthScoreInfo) -> GtkBox {
    let card = GtkBox::new(Orientation::Horizontal, 8);
    card.style_context().add_class("health-card");

    let label = Label::new(Some("Link Health"));
    label.style_context().add_class("health-label");
    card.pack_start(&label, false, false, 0);

    let score_text = format!("{:.0}", hs.score);
    let score_lbl = Label::new(Some(&score_text));
    score_lbl.style_context().add_class("health-score");
    if hs.score >= 80.0 {
        score_lbl.style_context().add_class("green");
    } else if hs.score >= 60.0 {
        score_lbl.style_context().add_class("yellow");
    } else if hs.score >= 40.0 {
        score_lbl.style_context().add_class("orange");
    } else {
        score_lbl.style_context().add_class("red");
    }
    card.pack_start(&score_lbl, false, false, 0);

    let bar = LevelBar::for_interval(0.0, 100.0);
    bar.set_value(hs.score);
    bar.set_hexpand(true);
    card.pack_start(&bar, true, true, 0);

    if hs.event_count > 0 {
        let events_lbl = Label::new(Some(&format!("{} Events", hs.event_count)));
        events_lbl.style_context().add_class("health-events");
        card.pack_start(&events_lbl, false, false, 0);
    }

    card
}

fn build_remote_gpu_card(rgpu: &RemoteGpuInfo) -> GtkBox {
    let card = GtkBox::new(Orientation::Vertical, 4);
    card.style_context().add_class("gpu-card");

    let header = GtkBox::new(Orientation::Horizontal, 6);

    let icon = Image::from_icon_name(Some("network-workgroup"), gtk::IconSize::Menu);
    header.pack_start(&icon, false, false, 0);

    let name_text = if rgpu.gpu_name.is_empty() {
        rgpu.name.clone()
    } else {
        rgpu.gpu_name.replace("NVIDIA GeForce ", "")
    };
    let name = Label::new(Some(&name_text));
    name.style_context().add_class("gpu-name");
    name.set_hexpand(true);
    name.set_halign(Align::Start);
    header.pack_start(&name, true, true, 0);

    let badge = Label::new(Some("LanGPU"));
    badge.style_context().add_class("gpu-badge");
    badge.style_context().add_class("badge-remote");
    header.pack_start(&badge, false, false, 0);

    let is_online = rgpu.status == "online" || rgpu.status == "available";
    let status_lbl = Label::new(Some(if is_online { "\u{25CF} Online" } else { "\u{25CB} Offline" }));
    status_lbl.style_context().add_class(if is_online { "green" } else { "muted" });
    header.pack_start(&status_lbl, false, false, 0);

    card.pack_start(&header, false, false, 0);

    let stats = GtkBox::new(Orientation::Horizontal, 12);

    let host_lbl = Label::new(Some(&format!("Host: {}", rgpu.host)));
    host_lbl.style_context().add_class("gpu-stat");
    stats.pack_start(&host_lbl, false, false, 0);

    if let Some(lat) = rgpu.latency_ms {
        let lat_lbl = Label::new(Some(&format!("{}ms", lat)));
        lat_lbl.style_context().add_class("gpu-stat-val");
        if lat > 50 {
            lat_lbl.style_context().add_class("orange");
        }
        stats.pack_start(&lat_lbl, false, false, 0);
    }

    let vram_lbl = Label::new(Some(&format!("{} MB", rgpu.vram_mb)));
    vram_lbl.style_context().add_class("gpu-stat");
    vram_lbl.set_halign(Align::End);
    vram_lbl.set_hexpand(true);
    stats.pack_start(&vram_lbl, true, true, 0);

    card.pack_start(&stats, false, false, 0);

    card
}
