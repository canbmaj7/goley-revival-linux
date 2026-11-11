use crate::api::{
    check_github_launcher_update, fetch_news_detail, format_turkish_date,
    perform_launcher_self_update, LauncherReleaseInfo, NewsItem,
};
use crate::config::{which_binary, AppConfig};
use crate::prefix::{
    discover_runners, install_ncrypt_shims, revival_game_dir, revival_launcher_log_path,
};
use crate::runner::LoginCredentials;
use crate::ui::style::apply_color_scheme;
use adw::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::RefCell;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::rc::Rc;

pub fn open_uri_or_path(parent: &impl IsA<gtk::Window>, uri: &str) {
    let launcher = gtk::UriLauncher::new(uri);
    launcher.launch(Some(parent), gio::Cancellable::NONE, |_| {});
}

pub fn open_local_folder(parent: &impl IsA<gtk::Window>, path: &Path) {
    let _ = fs::create_dir_all(path);
    // Masaüstü portalı (UriLauncher) KDE'de klasörleri sessizce açamayabiliyor;
    // önce xdg-open dene, olmazsa eski yönteme dön.
    let spawned = std::process::Command::new("xdg-open")
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok();
    if !spawned {
        let uri = format!("file://{}", path.display());
        open_uri_or_path(parent, &uri);
    }
}

pub fn present_login_dialog<F>(
    window: &adw::ApplicationWindow,
    initial_username: Option<String>,
    error_reason: Option<String>,
    on_submit: F,
) where
    F: Fn(LoginCredentials) + 'static,
{
    let dialog = adw::Dialog::builder()
        .title("Goley Revival Hesap Girişi")
        .content_width(430)
        .build();

    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    toolbar.add_top_bar(&header);

    let outer = gtk::Box::new(gtk::Orientation::Vertical, 16);
    outer.set_margin_top(8);
    outer.set_margin_bottom(22);
    outer.set_margin_start(22);
    outer.set_margin_end(22);

    if let Some(reason) = error_reason {
        let banner = adw::Banner::new(&reason);
        banner.set_revealed(true);
        outer.append(&banner);
    }

    let intro = gtk::Label::new(Some(
        "Hesap bilgileriniz doğrudan resmi Goley Revival kimlik doğrulama istemcisine iletilir. Şifreniz diske kaydedilmez.",
    ));
    intro.set_wrap(true);
    intro.set_xalign(0.0);
    intro.add_css_class("dim-label");
    intro.add_css_class("caption");
    outer.append(&intro);

    let group = adw::PreferencesGroup::new();

    let user_row = adw::EntryRow::builder()
        .title("Kullanıcı Adı")
        .show_apply_button(false)
        .build();
    if let Some(u) = initial_username {
        user_row.set_text(&u);
    }

    let pass_row = adw::PasswordEntryRow::builder().title("Şifre").build();

    let remember_row = adw::SwitchRow::builder()
        .title("Beni Hatırla")
        .subtitle("Oturum anahtarını ve kullanıcı adını sonraki girişler için sakla")
        .active(true)
        .build();

    group.add(&user_row);
    group.add(&pass_row);
    group.add(&remember_row);
    outer.append(&group);

    let btn_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    btn_row.set_halign(gtk::Align::Fill);

    let reg_btn = gtk::Button::with_label("Hesap Oluştur");
    reg_btn.add_css_class("flat");
    {
        let win_weak = window.downgrade();
        reg_btn.connect_clicked(move |_| {
            if let Some(w) = win_weak.upgrade() {
                open_uri_or_path(&w, "https://playrevival.co/register");
            }
        });
    }

    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);

    let submit_btn = gtk::Button::with_label("Giriş Yap ve Oyna");
    submit_btn.add_css_class("suggested-action");
    submit_btn.add_css_class("pill");

    btn_row.append(&reg_btn);
    btn_row.append(&spacer);
    btn_row.append(&submit_btn);
    outer.append(&btn_row);

    let on_submit_rc = Rc::new(on_submit);

    let trigger_submit = {
        let dialog_weak = dialog.downgrade();
        let u_row = user_row.clone();
        let p_row = pass_row.clone();
        let r_row = remember_row.clone();
        let cb = Rc::clone(&on_submit_rc);
        move || {
            let username = u_row.text().trim().to_string();
            let password = p_row.text().to_string();
            if username.is_empty() || password.is_empty() {
                return;
            }
            if let Some(d) = dialog_weak.upgrade() {
                d.close();
            }
            cb(LoginCredentials {
                username,
                password,
                remember: r_row.is_active(),
            });
        }
    };

    let ts_click = trigger_submit.clone();
    submit_btn.connect_clicked(move |_| ts_click());

    let ts_enter = trigger_submit;
    pass_row.connect_entry_activated(move |_| ts_enter());

    toolbar.set_content(Some(&outer));
    dialog.set_child(Some(&toolbar));
    dialog.present(Some(window));
}

pub fn present_news_dialog(window: &adw::ApplicationWindow, item: &NewsItem) {
    let dialog = adw::Dialog::builder()
        .title(&item.title)
        .content_width(640)
        .content_height(500)
        .build();

    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    toolbar.add_top_bar(&header);

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .build();

    let body_box = gtk::Box::new(gtk::Orientation::Vertical, 14);
    body_box.set_margin_top(16);
    body_box.set_margin_bottom(24);
    body_box.set_margin_start(24);
    body_box.set_margin_end(24);

    let date_lbl = gtk::Label::new(Some(&format_turkish_date(item.published_at.as_deref())));
    date_lbl.set_xalign(0.0);
    date_lbl.add_css_class("dim-label");
    date_lbl.add_css_class("caption-heading");
    body_box.append(&date_lbl);

    let title_lbl = gtk::Label::new(Some(&item.title));
    title_lbl.set_wrap(true);
    title_lbl.set_xalign(0.0);
    title_lbl.add_css_class("title-2");
    body_box.append(&title_lbl);

    let sep = gtk::Separator::new(gtk::Orientation::Horizontal);
    body_box.append(&sep);

    let content_lbl = gtk::Label::new(Some(
        item.summary
            .as_deref()
            .unwrap_or("Duyuru içeriği yükleniyor..."),
    ));
    content_lbl.set_wrap(true);
    content_lbl.set_selectable(true);
    content_lbl.set_xalign(0.0);
    content_lbl.set_yalign(0.0);
    body_box.append(&content_lbl);

    let spinner = gtk::Spinner::new();
    spinner.set_halign(gtk::Align::Center);
    spinner.start();
    body_box.append(&spinner);

    scroller.set_child(Some(&body_box));
    toolbar.set_content(Some(&scroller));
    dialog.set_child(Some(&toolbar));
    dialog.present(Some(window));

    let news_id = item.id.clone();
    let fallback_summary = item.summary.clone().unwrap_or_default();
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let res = fetch_news_detail(&news_id);
        let _ = tx.send_blocking(res);
    });

    glib::spawn_future_local(async move {
        if let Ok(result) = rx.recv().await {
            spinner.stop();
            spinner.set_visible(false);
            match result {
                Ok(detail) => {
                    let text = detail
                        .body
                        .filter(|b| !b.trim().is_empty())
                        .or(detail.summary)
                        .unwrap_or(fallback_summary);
                    content_lbl.set_text(&strip_simple_html(&text));
                }
                Err(err) => {
                    if fallback_summary.is_empty() {
                        content_lbl.set_text(&err);
                    }
                }
            }
        }
    });
}

fn strip_simple_html(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut in_tag = false;
    let normalized = raw
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("<br />", "\n")
        .replace("</p>", "\n\n")
        .replace("</li>", "\n")
        .replace("<li>", "  • ");

    for ch in normalized.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.trim().to_string()
}

pub fn present_logs_dialog(
    window: &adw::ApplicationWindow,
    prefix: &Path,
    session_logs: &Rc<RefCell<Vec<String>>>,
    toast_overlay: &adw::ToastOverlay,
) {
    let dialog = adw::Dialog::builder()
        .title("Canlı Günlükler (Logs)")
        .content_width(760)
        .content_height(520)
        .build();

    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();

    let log_file_path = revival_launcher_log_path(prefix);

    let copy_btn = gtk::Button::from_icon_name("edit-copy-symbolic");
    copy_btn.set_tooltip_text(Some("Günlükleri Panoya Kopyala"));

    let folder_btn = gtk::Button::from_icon_name("folder-open-symbolic");
    folder_btn.set_tooltip_text(Some("Log Klasörünü Aç"));

    header.pack_start(&folder_btn);
    header.pack_end(&copy_btn);
    toolbar.add_top_bar(&header);

    let scroller = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(true)
        .build();

    let text_view = gtk::TextView::builder()
        .editable(false)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::WordChar)
        .top_margin(12)
        .bottom_margin(12)
        .left_margin(14)
        .right_margin(14)
        .build();
    text_view.add_css_class("log-view");

    let runner_log_path = AppConfig::cache_dir().join("runner.log");
    let combined_text =
        build_combined_logs(&log_file_path, &runner_log_path, &session_logs.borrow());
    text_view.buffer().set_text(&combined_text);

    // Keep the support report current while the launcher is still waiting.
    {
        let tv_weak = text_view.downgrade();
        let logs = Rc::clone(session_logs);
        let launcher_path = log_file_path.clone();
        let runner_path = runner_log_path.clone();
        glib::timeout_add_seconds_local(1, move || {
            let Some(tv) = tv_weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let text = build_combined_logs(&launcher_path, &runner_path, &logs.borrow());
            let buf = tv.buffer();
            if buf.text(&buf.start_iter(), &buf.end_iter(), false).as_str() != text {
                buf.set_text(&text);
            }
            glib::ControlFlow::Continue
        });
    }

    {
        let tv = text_view.clone();
        let logs = Rc::clone(session_logs);
        let launcher_path = log_file_path.clone();
        let toast_ov = toast_overlay.clone();
        copy_btn.connect_clicked(move |_| {
            let text = build_combined_logs(&launcher_path, &runner_log_path, &logs.borrow());
            tv.buffer().set_text(&text);
            if let Some(display) = gdk::Display::default() {
                display.clipboard().set_text(&text);
                toast_ov.add_toast(adw::Toast::new("Günlükler panoya kopyalandı"));
            }
        });
    }

    {
        let win_weak = window.downgrade();
        let log_dir = log_file_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| prefix.to_path_buf());
        folder_btn.connect_clicked(move |_| {
            if let Some(w) = win_weak.upgrade() {
                open_local_folder(&w, &log_dir);
            }
        });
    }

    scroller.set_child(Some(&text_view));
    toolbar.set_content(Some(&scroller));
    dialog.set_child(Some(&toolbar));
    dialog.present(Some(window));
}

fn build_combined_logs(log_path: &Path, runner_log_path: &Path, mem_lines: &[String]) -> String {
    let mut out = String::new();
    if !mem_lines.is_empty() {
        out.push_str("=== Oturum Kayıtları ===\n");
        for l in mem_lines {
            out.push_str(l);
            out.push('\n');
        }
        out.push('\n');
    }
    for path in [log_path, runner_log_path] {
        if let Ok(disk_log) = fs::read_to_string(path) {
            out.push_str(&format!("=== {} ===\n", path.display()));
            let lines: Vec<&str> = disk_log.lines().collect();
            let start = lines.len().saturating_sub(250);
            for l in &lines[start..] {
                out.push_str(l);
                out.push('\n');
            }
            out.push('\n');
        }
    }
    if out.is_empty() {
        out.push_str("Henüz günlük kaydı oluşmadı.");
    }
    out
}

pub fn present_preferences_dialog<FRepair, FVerify>(
    window: &adw::ApplicationWindow,
    config_rc: &Rc<RefCell<AppConfig>>,
    toast_overlay: &adw::ToastOverlay,
    on_config_changed: Rc<dyn Fn()>,
    on_repair_shims: FRepair,
    on_verify_files: FVerify,
) where
    FRepair: Fn() + 'static,
    FVerify: Fn() + 'static,
{
    let prefs = adw::PreferencesDialog::builder()
        .title("Başlatıcı Ayarları")
        .build();

    // ── PAGE 1: Oyun ve Uyumluluk ──────────────────────────────
    let page_compat = adw::PreferencesPage::builder()
        .title("Uyumluluk")
        .icon_name("applications-games-symbolic")
        .build();

    let group_runner = adw::PreferencesGroup::builder()
        .title("Uyumluluk Katmanı (Wine / Proton / UMU)")
        .description("Goley Revival'ın çalışacağı uyumluluk katmanını ve Wine önekini (prefix) yapılandırın.")
        .build();

    let discovered = discover_runners();
    let mut runner_ids = vec!["auto".to_string()];
    let mut runner_labels = vec!["Otomatik Seç (Önerilen)".to_string()];
    for r in &discovered {
        runner_ids.push(r.id.clone());
        runner_labels.push(r.label.clone());
    }
    let label_refs: Vec<&str> = runner_labels.iter().map(|s| s.as_str()).collect();
    let model = gtk::StringList::new(&label_refs);

    let current_runner_id = config_rc.borrow().runner_id.clone();
    let selected_idx = runner_ids
        .iter()
        .position(|id| id == &current_runner_id)
        .unwrap_or(0);

    let runner_combo = adw::ComboRow::builder()
        .title("Çalıştırıcı (Runner)")
        .subtitle("Proton-GE + UMU-Run en yüksek uyumluluğu sağlar")
        .model(&model)
        .selected(selected_idx as u32)
        .build();

    {
        let cfg = Rc::clone(config_rc);
        let ids = runner_ids.clone();
        let notify_cb = Rc::clone(&on_config_changed);
        runner_combo.connect_selected_notify(move |combo| {
            let idx = combo.selected() as usize;
            if let Some(chosen) = ids.get(idx) {
                cfg.borrow_mut().runner_id = chosen.clone();
                let _ = cfg.borrow().save();
                notify_cb();
            }
        });
    }
    group_runner.add(&runner_combo);

    // Prefix row
    let prefix_str = config_rc.borrow().prefix_path.display().to_string();
    let prefix_row = adw::ActionRow::builder()
        .title("Wine Prefix Dizini")
        .subtitle(&prefix_str)
        .build();

    let btn_open_pfx = gtk::Button::from_icon_name("folder-open-symbolic");
    btn_open_pfx.add_css_class("flat");
    btn_open_pfx.set_valign(gtk::Align::Center);
    btn_open_pfx.set_tooltip_text(Some("Prefix Klasörünü Aç"));

    let btn_pick_pfx = gtk::Button::with_label("Değiştir...");
    btn_pick_pfx.set_valign(gtk::Align::Center);

    prefix_row.add_suffix(&btn_open_pfx);
    prefix_row.add_suffix(&btn_pick_pfx);

    {
        let win_weak = window.downgrade();
        let cfg = Rc::clone(config_rc);
        btn_open_pfx.connect_clicked(move |_| {
            if let Some(w) = win_weak.upgrade() {
                open_local_folder(&w, &cfg.borrow().prefix_path);
            }
        });
    }

    {
        let win_weak = window.downgrade();
        let cfg = Rc::clone(config_rc);
        let row_clone = prefix_row.clone();
        let notify_cb = Rc::clone(&on_config_changed);
        btn_pick_pfx.connect_clicked(move |_| {
            let Some(win) = win_weak.upgrade() else {
                return;
            };
            let file_dlg = gtk::FileDialog::builder()
                .title("Wine Prefix Klasörünü Seçin")
                .build();
            let cfg_inner = Rc::clone(&cfg);
            let row_inner = row_clone.clone();
            let cb_inner = Rc::clone(&notify_cb);
            file_dlg.select_folder(Some(&win), gio::Cancellable::NONE, move |res| {
                if let Ok(folder) = res {
                    if let Some(path) = folder.path() {
                        row_inner.set_subtitle(&path.display().to_string());
                        cfg_inner.borrow_mut().prefix_path = path;
                        let _ = cfg_inner.borrow().save();
                        cb_inner();
                    }
                }
            });
        });
    }
    group_runner.add(&prefix_row);

    // Game directory quick open row
    let game_dir_str = revival_game_dir(&config_rc.borrow().prefix_path)
        .display()
        .to_string();
    let game_dir_row = adw::ActionRow::builder()
        .title("Oyun Dosyaları Dizini")
        .subtitle(&game_dir_str)
        .build();
    let btn_open_game = gtk::Button::from_icon_name("folder-open-symbolic");
    btn_open_game.add_css_class("flat");
    btn_open_game.set_valign(gtk::Align::Center);
    {
        let win_weak = window.downgrade();
        let cfg = Rc::clone(config_rc);
        btn_open_game.connect_clicked(move |_| {
            if let Some(w) = win_weak.upgrade() {
                let p = revival_game_dir(&cfg.borrow().prefix_path);
                open_local_folder(&w, &p);
            }
        });
    }
    game_dir_row.add_suffix(&btn_open_game);
    group_runner.add(&game_dir_row);
    page_compat.add(&group_runner);

    // Performance group
    let group_perf = adw::PreferencesGroup::builder()
        .title("Performans ve Görüntü")
        .build();

    let has_gamemode = which_binary("gamemoderun").is_some();
    let sw_gamemode = adw::SwitchRow::builder()
        .title("Feral GameMode")
        .subtitle(if has_gamemode {
            "Oyun sırasında işlemci valisini (CPU governor) performans moduna alır"
        } else {
            "Sistemde 'gamemode' paketi bulunamadı"
        })
        .sensitive(has_gamemode)
        .active(config_rc.borrow().use_gamemode && has_gamemode)
        .build();
    {
        let cfg = Rc::clone(config_rc);
        sw_gamemode.connect_active_notify(move |sw| {
            cfg.borrow_mut().use_gamemode = sw.is_active();
            let _ = cfg.borrow().save();
        });
    }
    group_perf.add(&sw_gamemode);

    let has_mangohud = which_binary("mangohud").is_some();
    let sw_mangohud = adw::SwitchRow::builder()
        .title("MangoHud Göster")
        .subtitle(if has_mangohud {
            "Oyun içinde FPS, kare süresi ve donanım kullanımını gösterir"
        } else {
            "Sistemde 'mangohud' paketi bulunamadı"
        })
        .sensitive(has_mangohud)
        .active(config_rc.borrow().use_mangohud && has_mangohud)
        .build();
    {
        let cfg = Rc::clone(config_rc);
        sw_mangohud.connect_active_notify(move |sw| {
            cfg.borrow_mut().use_mangohud = sw.is_active();
            let _ = cfg.borrow().save();
        });
    }
    group_perf.add(&sw_mangohud);

    let has_gamescope = which_binary("gamescope").is_some();
    let sw_gamescope = adw::SwitchRow::builder()
        .title("Gamescope Mikro-Kompozitör")
        .subtitle(if has_gamescope {
            "Oyunu izole Wayland mikro-kompozitörü içinde çalıştırır"
        } else {
            "Sistemde 'gamescope' paketi bulunamadı"
        })
        .sensitive(has_gamescope)
        .active(config_rc.borrow().use_gamescope && has_gamescope)
        .build();
    {
        let cfg = Rc::clone(config_rc);
        sw_gamescope.connect_active_notify(move |sw| {
            cfg.borrow_mut().use_gamescope = sw.is_active();
            let _ = cfg.borrow().save();
        });
    }
    group_perf.add(&sw_gamescope);

    let env_row = adw::EntryRow::builder()
        .title("Ek Ortam Değişkenleri (Örn: DXVK_HUD=fps)")
        .text(&config_rc.borrow().extra_env)
        .build();
    {
        let cfg = Rc::clone(config_rc);
        env_row.connect_changed(move |row| {
            cfg.borrow_mut().extra_env = row.text().to_string();
            let _ = cfg.borrow().save();
        });
    }
    group_perf.add(&env_row);

    // Theme selector
    let theme_model = gtk::StringList::new(&["Koyu (Varsayılan)", "Sistem Teması", "Açık"]);
    let cur_scheme = config_rc.borrow().color_scheme.clone();
    let scheme_idx = match cur_scheme.as_str() {
        "system" => 1,
        "light" => 2,
        _ => 0,
    };
    let theme_combo = adw::ComboRow::builder()
        .title("Arayüz Görünümü")
        .model(&theme_model)
        .selected(scheme_idx)
        .build();
    {
        let cfg = Rc::clone(config_rc);
        theme_combo.connect_selected_notify(move |combo| {
            let val = match combo.selected() {
                1 => "system",
                2 => "light",
                _ => "dark",
            };
            apply_color_scheme(val);
            cfg.borrow_mut().color_scheme = val.to_string();
            let _ = cfg.borrow().save();
        });
    }
    group_perf.add(&theme_combo);
    page_compat.add(&group_perf);
    prefs.add(&page_compat);

    // ── PAGE 2: Bakım ve Sorun Giderme ─────────────────────────
    let page_maint = adw::PreferencesPage::builder()
        .title("Bakım ve Onarım")
        .icon_name("emblem-system-symbolic")
        .build();

    let group_fixes = adw::PreferencesGroup::builder()
        .title("Linux Uyumluluk Yamaları ve Dosya Onarımı")
        .description("Goley Revival'ın Linux'ta kesintisiz çalışması için gereken bileşenler.")
        .build();

    let row_shim = adw::ActionRow::builder()
        .title("Linux Uyumluluk Köprüsü (ncrypt.dll)")
        .subtitle("ECDSA_P256 cihaz kimliği ve oturum imzalama köprüsü")
        .build();
    let btn_shim = gtk::Button::with_label("Yeniden Kur");
    btn_shim.set_valign(gtk::Align::Center);
    {
        let cfg = Rc::clone(config_rc);
        let toast_ov = toast_overlay.clone();
        btn_shim.connect_clicked(
            move |_| match install_ncrypt_shims(&cfg.borrow().prefix_path) {
                Ok(_) => {
                    on_repair_shims();
                    toast_ov.add_toast(adw::Toast::new("ncrypt.dll köprüsü başarıyla kuruldu"));
                }
                Err(e) => {
                    toast_ov.add_toast(adw::Toast::new(&format!("Hata: {}", e)));
                }
            },
        );
    }
    row_shim.add_suffix(&btn_shim);
    group_fixes.add(&row_shim);

    let row_verify = adw::ActionRow::builder()
        .title("Oyun Dosyalarını Doğrula (SHA-256)")
        .subtitle(
            "Tüm 168 oyun dosyasını resmi sunucu özetleriyle karşılaştırır ve eksikleri indirir",
        )
        .build();
    let btn_verify = gtk::Button::with_label("Şimdi Doğrula");
    btn_verify.set_valign(gtk::Align::Center);
    {
        let prefs_weak = prefs.downgrade();
        btn_verify.connect_clicked(move |_| {
            if let Some(p) = prefs_weak.upgrade() {
                p.close();
            }
            on_verify_files();
        });
    }
    row_verify.add_suffix(&btn_verify);
    group_fixes.add(&row_verify);

    let has_winetricks = which_binary("winetricks").is_some();
    let row_physx = adw::ActionRow::builder()
        .title("NVIDIA PhysX Runtime Kurulumu")
        .subtitle(if has_winetricks {
            "Oyun açılışta PhysX hatası verirse tek tıkla prefix içine PhysX kurar"
        } else {
            "Bu özellik için sisteminize 'winetricks' paketini kurun"
        })
        .build();
    let btn_physx = gtk::Button::with_label("PhysX Kur");
    btn_physx.set_valign(gtk::Align::Center);
    btn_physx.set_sensitive(has_winetricks);
    {
        let cfg = Rc::clone(config_rc);
        let toast_ov = toast_overlay.clone();
        btn_physx.connect_clicked(move |btn| {
            btn.set_sensitive(false);
            btn.set_label("Kuruluyor...");
            let pfx = cfg.borrow().prefix_path.clone();
            let wine_user = crate::prefix::resolve_wine_user_dir(&pfx);
            let user_name = wine_user
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("steamuser")
                .to_string();
            let (tx, rx) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let res = Command::new("winetricks")
                    .env("WINEPREFIX", &pfx)
                    .env("USER", &user_name)
                    .env("LOGNAME", &user_name)
                    .args(["-q", "physx"])
                    .status();
                let _ = tx.send_blocking(res.map(|s| s.success()).unwrap_or(false));
            });
            let btn_clone = btn.clone();
            let toast_clone = toast_ov.clone();
            glib::spawn_future_local(async move {
                let ok = rx.recv().await.unwrap_or(false);
                btn_clone.set_sensitive(true);
                btn_clone.set_label("PhysX Kur");
                if ok {
                    toast_clone.add_toast(adw::Toast::new("NVIDIA PhysX başarıyla kuruldu!"));
                } else {
                    toast_clone.add_toast(adw::Toast::new("PhysX kurulumu tamamlanamadı"));
                }
            });
        });
    }
    row_physx.add_suffix(&btn_physx);
    group_fixes.add(&row_physx);

    let row_self_upd = adw::ActionRow::builder()
        .title(&format!(
            "Başlatıcı Güncelleme (Mevcut: v{})",
            env!("CARGO_PKG_VERSION")
        ))
        .subtitle("GitHub Releases üzerinden en son başlatıcı sürümünü ve ncrypt yamalarını çeker")
        .build();
    let btn_self_upd = gtk::Button::with_label("Güncellemeleri Denetle");
    btn_self_upd.set_valign(gtk::Align::Center);
    {
        let prefs_weak = prefs.downgrade();
        let win_weak = window.downgrade();
        let toast_ov = toast_overlay.clone();
        btn_self_upd.connect_clicked(move |_| {
            if let Some(p) = prefs_weak.upgrade() {
                p.close();
            }
            if let Some(w) = win_weak.upgrade() {
                present_updater_dialog(&w, None, &toast_ov);
            }
        });
    }
    row_self_upd.add_suffix(&btn_self_upd);
    group_fixes.add(&row_self_upd);

    let row_health = adw::ActionRow::builder()
        .title("Sistem Kontrolü")
        .subtitle("Wine, UMU, Proton, Vulkan ve sistem bağımlılıklarının sağlık durumu")
        .build();
    let btn_health = gtk::Button::with_label("Denetle");
    btn_health.set_valign(gtk::Align::Center);
    {
        let win_weak = window.downgrade();
        btn_health.connect_clicked(move |_| {
            if let Some(w) = win_weak.upgrade() {
                crate::ui::health::show_health_dialog(&w);
            }
        });
    }
    row_health.add_suffix(&btn_health);
    group_fixes.add(&row_health);

    page_maint.add(&group_fixes);

    let group_debug = adw::PreferencesGroup::builder()
        .title("Geliştirici / Hata Ayıklama")
        .build();

    let sw_debug_win = adw::SwitchRow::builder()
        .title("Arka Plan Windows Başlatıcısını Göster")
        .subtitle("Yalnızca sorun giderme için açın; normalde gizli çalışır ve %0 CPU tüketir")
        .active(config_rc.borrow().show_debug_window)
        .build();
    {
        let cfg = Rc::clone(config_rc);
        let notify_cb = Rc::clone(&on_config_changed);
        sw_debug_win.connect_active_notify(move |sw| {
            cfg.borrow_mut().show_debug_window = sw.is_active();
            let _ = cfg.borrow().save();
            notify_cb();
        });
    }
    group_debug.add(&sw_debug_win);

    let row_log_pack = adw::ActionRow::builder()
        .title("Log Paketini Kopyala")
        .subtitle("Sistem bilgileri, launcher.log ve runner.log kayıtlarını gizlilik korumalı olarak panoya kopyalar")
        .build();
    let btn_log_pack = gtk::Button::with_label("Kopyala");
    btn_log_pack.set_valign(gtk::Align::Center);
    {
        let cfg = Rc::clone(config_rc);
        let toast_ov = toast_overlay.clone();
        btn_log_pack.connect_clicked(move |_| {
            let c = cfg.borrow();
            crate::ui::log_pack::copy_log_package_to_clipboard(&c.prefix_path, &c.runner_id);
            toast_ov.add_toast(adw::Toast::new("Log paketi panoya kopyalandı"));
        });
    }
    row_log_pack.add_suffix(&btn_log_pack);
    group_debug.add(&row_log_pack);
    page_maint.add(&group_debug);

    prefs.add(&page_maint);
    prefs.present(Some(window));
}

pub fn present_updater_dialog(
    window: &adw::ApplicationWindow,
    cached_info: Option<LauncherReleaseInfo>,
    toast_overlay: &adw::ToastOverlay,
) {
    let dialog = adw::Dialog::builder()
        .title("Başlatıcı Güncelleme (GitHub)")
        .content_width(520)
        .content_height(430)
        .build();

    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    toolbar.add_top_bar(&header);

    let outer = gtk::Box::new(gtk::Orientation::Vertical, 14);
    outer.set_margin_top(14);
    outer.set_margin_bottom(20);
    outer.set_margin_start(22);
    outer.set_margin_end(22);

    let status_group = adw::PreferencesGroup::new();
    let cur_row = adw::ActionRow::builder()
        .title("Yüklü Başlatıcı Sürümü")
        .subtitle(&format!("v{}", env!("CARGO_PKG_VERSION")))
        .build();
    let remote_row = adw::ActionRow::builder()
        .title("GitHub Son Sürüm")
        .subtitle("Denetleniyor...")
        .build();

    let spinner = gtk::Spinner::new();
    spinner.set_valign(gtk::Align::Center);
    spinner.start();
    remote_row.add_suffix(&spinner);

    status_group.add(&cur_row);
    status_group.add(&remote_row);
    outer.append(&status_group);

    let notes_lbl_hdr = gtk::Label::new(Some("Sürüm Notları"));
    notes_lbl_hdr.add_css_class("heading");
    notes_lbl_hdr.set_xalign(0.0);
    outer.append(&notes_lbl_hdr);

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .build();
    let notes_view = gtk::TextView::builder()
        .editable(false)
        .wrap_mode(gtk::WrapMode::WordChar)
        .top_margin(10)
        .bottom_margin(10)
        .left_margin(12)
        .right_margin(12)
        .build();
    notes_view.add_css_class("card");
    notes_view
        .buffer()
        .set_text("GitHub Releases üzerinden en son sürüm bilgileri alınıyor...");
    scroller.set_child(Some(&notes_view));
    outer.append(&scroller);

    let btn_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let btn_github = gtk::Button::with_label("GitHub Sayfasını Aç");
    btn_github.add_css_class("flat");

    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);

    let btn_apply = gtk::Button::with_label("Denetleniyor...");
    btn_apply.add_css_class("suggested-action");
    btn_apply.add_css_class("pill");
    btn_apply.set_sensitive(false);

    btn_row.append(&btn_github);
    btn_row.append(&spacer);
    btn_row.append(&btn_apply);
    outer.append(&btn_row);

    toolbar.set_content(Some(&outer));
    dialog.set_child(Some(&toolbar));
    dialog.present(Some(window));

    let info_rc: Rc<RefCell<Option<LauncherReleaseInfo>>> = Rc::new(RefCell::new(None));

    {
        let win_weak = window.downgrade();
        let info_c = Rc::clone(&info_rc);
        btn_github.connect_clicked(move |_| {
            if let Some(w) = win_weak.upgrade() {
                let url = info_c
                    .borrow()
                    .as_ref()
                    .map(|i| i.html_url.clone())
                    .unwrap_or_else(|| {
                        "https://github.com/Fuwia/goley-revival-linux/releases".to_string()
                    });
                open_uri_or_path(&w, &url);
            }
        });
    }

    let populate_ui = {
        let rem_r = remote_row.clone();
        let spin = spinner.clone();
        let nv = notes_view.clone();
        let btn = btn_apply.clone();
        let info_store = Rc::clone(&info_rc);
        move |res: Result<LauncherReleaseInfo, String>| {
            spin.stop();
            spin.set_visible(false);
            match res {
                Ok(info) => {
                    if info.update_available {
                        rem_r.set_subtitle(&format!(
                            "{} — Yeni Güncelleme Mevcut!",
                            info.tag_name
                        ));
                        btn.set_label(&format!("Şimdi Güncelle ({})", info.tag_name));
                    } else {
                        rem_r.set_subtitle(&format!("{} — Güncel", info.tag_name));
                        btn.set_label(&format!("Yeniden İndir / Onar ({})", info.tag_name));
                    }
                    nv.buffer().set_text(&info.body);
                    btn.set_sensitive(true);
                    *info_store.borrow_mut() = Some(info);
                }
                Err(err) => {
                    rem_r.set_subtitle("Bağlantı hatası");
                    nv.buffer().set_text(&err);
                    btn.set_label("Tekrar Dene");
                    btn.set_sensitive(false);
                }
            }
        }
    };

    if let Some(info) = cached_info {
        populate_ui(Ok(info));
    } else {
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send_blocking(check_github_launcher_update());
        });
        glib::spawn_future_local(async move {
            if let Ok(res) = rx.recv().await {
                populate_ui(res);
            }
        });
    }

    {
        let info_c = Rc::clone(&info_rc);
        let spin = spinner.clone();
        let rem_r = remote_row.clone();
        let toast_ov = toast_overlay.clone();
        let dlg_weak = dialog.downgrade();
        let win_weak = window.downgrade();

        btn_apply.connect_clicked(move |btn| {
            let Some(info) = info_c.borrow().clone() else {
                return;
            };
            btn.set_sensitive(false);
            btn.set_label("İndiriliyor ve Kuruluyor...");
            spin.set_visible(true);
            spin.start();
            rem_r.set_subtitle("GitHub Releases üzerinden yeni sürüm indiriliyor...");

            let (tx, rx) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let _ = tx.send_blocking(perform_launcher_self_update(&info));
            });

            let btn_c = btn.clone();
            let spin_c = spin.clone();
            let rem_c = rem_r.clone();
            let toast_c = toast_ov.clone();
            let dlg_w = dlg_weak.clone();
            let win_w = win_weak.clone();

            glib::spawn_future_local(async move {
                if let Ok(res) = rx.recv().await {
                    spin_c.stop();
                    spin_c.set_visible(false);
                    match res {
                        Ok(new_exe_path) => {
                            toast_c.add_toast(adw::Toast::new(
                                "Güncelleme tamamlandı! Başlatıcı yeniden başlatılıyor...",
                            ));
                            if let Some(d) = dlg_w.upgrade() {
                                d.close();
                            }
                            let _ = Command::new(&new_exe_path).spawn();
                            if let Some(w) = win_w.upgrade() {
                                if let Some(app) = w.application() {
                                    app.quit();
                                }
                            }
                            std::process::exit(0);
                        }
                        Err(err) => {
                            rem_c.set_subtitle("Güncelleme başarısız oldu");
                            btn_c.set_sensitive(true);
                            btn_c.set_label("Tekrar Dene");
                            toast_c.add_toast(adw::Toast::new(&err));
                        }
                    }
                }
            });
        });
    }
}
