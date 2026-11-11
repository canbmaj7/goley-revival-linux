use crate::api::get_available_space;
use crate::config::{which_binary, AppConfig};
use crate::prefix::{find_proton_dirs, find_umu_bin, resolve_wine_user_dir};
use adw::prelude::*;
use gtk::glib;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub name: String,
    pub ok: bool,
    pub details: String,
    pub hint: Option<String>,
}

impl CheckResult {
    pub fn ok(name: &str, details: impl Into<String>) -> Self {
        Self {
            name: name.to_string(),
            ok: true,
            details: details.into(),
            hint: None,
        }
    }

    pub fn err(name: &str, details: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            name: name.to_string(),
            ok: false,
            details: details.into(),
            hint: Some(hint.into()),
        }
    }
}

pub fn check_wine_bin(bin: Option<PathBuf>) -> CheckResult {
    match bin {
        Some(p) => CheckResult::ok("Wine", p.display().to_string()),
        None => CheckResult::err("Wine", "wine ikili dosyası bulunamadı", "wine veya wine-staging paketi kurun"),
    }
}

pub fn check_umu_bin(bin: Option<PathBuf>) -> CheckResult {
    match bin {
        Some(p) => CheckResult::ok("umu-run", p.display().to_string()),
        None => CheckResult::err("umu-run", "umu-run bulunamadı", "umu-run veya Faugus Launcher kurun"),
    }
}

pub fn check_proton_dirs_result(dirs: &[PathBuf]) -> CheckResult {
    if !dirs.is_empty() {
        let name = dirs[0].file_name().and_then(|s| s.to_str()).unwrap_or("Proton");
        CheckResult::ok("Proton-GE", format!("{} sürüm bulundu ({})", dirs.len(), name))
    } else {
        CheckResult::err("Proton-GE", "Uyumluluk klasörlerinde Proton bulunamadı", "ProtonUp-Qt ile GE-Proton yükleyin")
    }
}

pub fn check_vulkaninfo_success(success: bool) -> CheckResult {
    if success {
        CheckResult::ok("Vulkan Desteği", "vulkaninfo başarıyla doğrulandı")
    } else {
        CheckResult::err("Vulkan Desteği", "vulkaninfo komutu başarısız veya bulunamadı", "vulkan-tools / mesa vulkan sürücüsü kur")
    }
}

pub fn check_gnutls_32_present(found: bool) -> CheckResult {
    if found {
        CheckResult::ok("32-bit GnuTLS", "32-bit libgnutls kütüphanesi mevcut")
    } else {
        CheckResult::err("32-bit GnuTLS", "32-bit libgnutls.so bulunamadı", "32-bit gnutls kütüphanesi kurun (lib32-gnutls veya libgnutls30:i386)")
    }
}

pub fn check_available_space_threshold(bytes: Option<u64>, min_required: u64) -> CheckResult {
    match bytes {
        Some(b) if b >= min_required => {
            let gb = b as f64 / (1024.0 * 1024.0 * 1024.0);
            CheckResult::ok("Boş Disk Alanı", format!("{:.1} GB boş alan mevcut", gb))
        }
        Some(b) => {
            let gb = b as f64 / (1024.0 * 1024.0 * 1024.0);
            CheckResult::err("Boş Disk Alanı", format!("{:.1} GB boş alan (2 GB altı)", gb), "En az 2 GB boş disk alanı açın")
        }
        None => CheckResult::err("Boş Disk Alanı", "Disk alanı sorgulanamadı", "Disk alanınızı kontrol edin (en az 2 GB gerekli)"),
    }
}

pub fn check_wine_user_dir_exists(dir: &Path) -> CheckResult {
    if dir.is_dir() {
        CheckResult::ok("Wine Prefix Kullanıcı Klasörü", dir.display().to_string())
    } else {
        CheckResult::err("Wine Prefix Kullanıcı Klasörü", format!("Dizin bulunamadı: {}", dir.display()), "Wine prefix kullanıcı dizini bulunamadı; prefix'i başlatın")
    }
}

pub fn detect_32bit_gnutls() -> bool {
    let candidates = ["/usr/lib32", "/usr/lib/i386-linux-gnu"];
    for dir in candidates {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    if name.starts_with("libgnutls.so") {
                        return true;
                    }
                }
            }
        }
    }
    false
}

pub fn run_all_system_checks(prefix: &Path) -> Vec<CheckResult> {
    let mut results = Vec::new();
    results.push(check_wine_bin(which_binary("wine")));
    results.push(check_umu_bin(find_umu_bin()));
    results.push(check_proton_dirs_result(&find_proton_dirs()));

    let vulkan_ok = Command::new("vulkaninfo")
        .arg("--summary")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    results.push(check_vulkaninfo_success(vulkan_ok));
    results.push(check_gnutls_32_present(detect_32bit_gnutls()));

    let space = get_available_space(prefix);
    results.push(check_available_space_threshold(space, 2 * 1024 * 1024 * 1024));

    let user_dir = resolve_wine_user_dir(prefix);
    results.push(check_wine_user_dir_exists(&user_dir));

    results
}

pub fn show_health_dialog(parent: &impl IsA<gtk::Window>) {
    let dialog = adw::Dialog::builder()
        .title("Sistem Kontrolü")
        .content_width(520)
        .content_height(480)
        .build();

    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    toolbar.add_top_bar(&header);

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .build();

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 12);
    vbox.set_margin_top(16);
    vbox.set_margin_bottom(20);
    vbox.set_margin_start(20);
    vbox.set_margin_end(20);

    let pref_group = adw::PreferencesGroup::builder()
        .title("Sistem Bileşenleri")
        .description("Goley Revival Linux için gerekli bileşenlerin sağlık kontrolü.")
        .build();

    let spinner = gtk::Spinner::new();
    spinner.start();
    spinner.set_halign(gtk::Align::Center);
    vbox.append(&spinner);
    vbox.append(&pref_group);

    scroller.set_child(Some(&vbox));
    toolbar.set_content(Some(&scroller));
    dialog.set_child(Some(&toolbar));
    let win: &gtk::Window = parent.as_ref();
    dialog.present(Some(win));

    let cfg = AppConfig::load();
    let prefix = cfg.prefix_path.clone();

    let (tx, rx) = async_channel::bounded::<Vec<CheckResult>>(1);
    std::thread::spawn(move || {
        let results = run_all_system_checks(&prefix);
        let _ = tx.send_blocking(results);
    });

    let group_clone = pref_group.clone();
    let spinner_clone = spinner.clone();
    glib::spawn_future_local(async move {
        if let Ok(results) = rx.recv().await {
            spinner_clone.stop();
            spinner_clone.set_visible(false);

            for res in results {
                let subtitle_text = if res.ok {
                    res.details
                } else if let Some(ref h) = res.hint {
                    format!("{} — Öneri: {}", res.details, h)
                } else {
                    res.details
                };

                let row = adw::ActionRow::builder()
                    .title(&res.name)
                    .subtitle(&subtitle_text)
                    .build();

                let icon = gtk::Image::from_icon_name(if res.ok {
                    "emblem-ok-symbolic"
                } else {
                    "dialog-error-symbolic"
                });
                icon.add_css_class(if res.ok { "success" } else { "error" });
                row.add_prefix(&icon);

                group_clone.add(&row);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_space_threshold_behavior() {
        let two_gb = 2 * 1024 * 1024 * 1024;
        let ok_res = check_available_space_threshold(Some(3 * 1024 * 1024 * 1024), two_gb);
        assert!(ok_res.ok);
        assert!(ok_res.hint.is_none());

        let low_res = check_available_space_threshold(Some(1024 * 1024 * 1024), two_gb);
        assert!(!low_res.ok);
        assert!(low_res.hint.is_some());

        let none_res = check_available_space_threshold(None, two_gb);
        assert!(!none_res.ok);
        assert!(none_res.hint.is_some());
    }

    #[test]
    fn proton_dirs_empty_versus_found() {
        let empty_res = check_proton_dirs_result(&[]);
        assert!(!empty_res.ok);
        assert!(empty_res.hint.as_deref().unwrap_or("").contains("ProtonUp-Qt"));

        let found = vec![PathBuf::from("/test/GE-Proton8-25")];
        let found_res = check_proton_dirs_result(&found);
        assert!(found_res.ok);
        assert!(found_res.details.contains("GE-Proton8-25"));
    }

    #[test]
    fn vulkan_and_gnutls_check_logic() {
        let vulkan_fail = check_vulkaninfo_success(false);
        assert!(!vulkan_fail.ok);
        assert!(vulkan_fail.hint.as_deref().unwrap_or("").contains("vulkan-tools"));

        let vulkan_pass = check_vulkaninfo_success(true);
        assert!(vulkan_pass.ok);

        let gnutls_fail = check_gnutls_32_present(false);
        assert!(!gnutls_fail.ok);
        assert!(gnutls_fail.hint.as_deref().unwrap_or("").contains("32-bit gnutls"));

        let gnutls_pass = check_gnutls_32_present(true);
        assert!(gnutls_pass.ok);
    }
}
