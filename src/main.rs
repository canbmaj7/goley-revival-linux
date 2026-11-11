mod api;
mod config;
mod desktop;
mod prefix;
mod runner;
mod ui;
mod userdata;

use adw::prelude::*;
use std::env;

const APP_ID: &str = "io.github.fuwia.GoleyRevivalLinux";

fn main() -> gtk::glib::ExitCode {
    desktop::ensure_game_window_entry();

    let raw_args: Vec<String> = env::args().collect();
    let mut snapshot_target = None;
    let mut filtered_args = Vec::new();

    let mut i = 0;
    while i < raw_args.len() {
        if raw_args[i] == "--snapshot" && i + 1 < raw_args.len() {
            snapshot_target = Some(raw_args[i + 1].clone());
            i += 2;
        } else {
            filtered_args.push(raw_args[i].clone());
            i += 1;
        }
    }

    let app_id = if snapshot_target.is_some() {
        "io.github.fuwia.GoleyRevivalLinux.Snapshot"
    } else {
        APP_ID
    };

    let mut builder = adw::Application::builder().application_id(app_id);
    if snapshot_target.is_some() {
        builder = builder.flags(adw::gio::ApplicationFlags::NON_UNIQUE);
    }
    let app = builder.build();

    let snap_target_clone = snapshot_target.clone();
    app.connect_activate(move |app| {
        ui::window::build_ui(app, snap_target_clone.clone());
    });

    app.run_with_args(&filtered_args)
}

