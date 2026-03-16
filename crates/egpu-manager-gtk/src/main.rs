mod api_client;
mod popup;
mod state;
mod tray;

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use tracing::info;

use crate::state::WidgetState;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    info!("eGPU Manager Widget startet");

    gtk::init().expect("GTK init");

    // Create popup window (hidden)
    let popup_window = popup::build_popup();
    let popup_ref = Rc::new(RefCell::new(popup_window));

    // Channel: tokio -> GTK main loop
    let (tx, rx) = async_channel::unbounded::<WidgetState>();

    // Tray icon
    let popup_toggle = Rc::clone(&popup_ref);
    let tray_indicator: Rc<RefCell<Option<libappindicator::AppIndicator>>> =
        Rc::new(RefCell::new(None));

    let indicator = tray::create_tray(
        move || {
            let win = popup_toggle.borrow();
            if win.is_visible() {
                win.hide();
            } else {
                win.show_all();
                win.present();
            }
        },
        || {
            let _ = open::that("http://127.0.0.1:7842");
        },
        || {
            gtk::main_quit();
        },
    );

    *tray_indicator.borrow_mut() = indicator;

    // Background polling thread
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        rt.block_on(api_client::poll_loop(tx));
    });

    // Receive state updates on GTK main loop via idle_add
    let popup_update = Rc::clone(&popup_ref);
    let tray_update = Rc::clone(&tray_indicator);
    let last_color: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));

    glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
        while let Ok(state) = rx.try_recv() {
            // Update tray icon color
            let color = state.warning_color().to_string();
            {
                let mut lc = last_color.borrow_mut();
                if *lc != color {
                    if let Some(ref mut ind) = *tray_update.borrow_mut() {
                        tray::update_tray_icon(ind, &color);
                    }
                    *lc = color;
                }
            }

            // Update popup if visible
            let win = popup_update.borrow();
            if win.is_visible() {
                popup::update_popup(&win, &state);
            }
        }
        glib::ControlFlow::Continue
    });

    info!("Widget gestartet — Tray-Icon aktiv");

    // Run GTK main loop
    gtk::main();
}
