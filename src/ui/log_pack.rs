use crate::config::AppConfig;
use crate::prefix::{revival_launcher_log_path, select_runner};
use adw::prelude::*;
use gtk::gdk;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct LogPackageData<'a> {
    pub version: &'a str,
    pub arch: &'a str,
    pub session_type: &'a str,
    pub desktop: &'a str,
    pub runner_label: &'a str,
    pub launcher_log: Option<&'a str>,
    pub runner_log: Option<&'a str>,
    pub home_dir: Option<&'a str>,
}

pub fn sanitize_log_line(line: &str) -> String {
    let lower = line.to_lowercase();
    if lower.contains("password")
        || lower.contains("passwd")
        || lower.contains("token")
        || lower.contains("ticket issued")
        || lower.contains("session")
    {
        "[gizlendi]".to_string()
    } else {
        line.to_string()
    }
}

pub fn extract_log_tail(content: Option<&str>, max_lines: usize, home_dir: Option<&str>) -> String {
    let Some(raw) = content else {
        return "(yok)".to_string();
    };
    if raw.trim().is_empty() {
        return "(yok)".to_string();
    }

    let all_lines: Vec<&str> = raw.lines().collect();
    let start = all_lines.len().saturating_sub(max_lines);
    let mut out = Vec::with_capacity(max_lines);

    for line in &all_lines[start..] {
        let masked = sanitize_log_line(line);
        let masked = if let Some(home) = home_dir {
            if !home.is_empty() {
                masked.replace(home, "~")
            } else {
                masked
            }
        } else {
            masked
        };
        out.push(masked);
    }

    if out.is_empty() {
        "(yok)".to_string()
    } else {
        out.join("\n")
    }
}

pub fn build_log_package(data: &LogPackageData) -> String {
    let launcher_tail = extract_log_tail(data.launcher_log, 80, data.home_dir);
    let runner_tail = extract_log_tail(data.runner_log, 60, data.home_dir);

    format!(
        "=== Goley Revival Linux - Log Paketi ===\n\
         Sürüm: {}\n\
         Mimari: {}\n\
         XDG_SESSION_TYPE: {}\n\
         XDG_CURRENT_DESKTOP: {}\n\
         Çalıştırıcı: {}\n\n\
         === launcher.log (Son 80 Satır) ===\n\
         {}\n\n\
         === runner.log (Son 60 Satır) ===\n\
         {}\n",
        data.version,
        data.arch,
        data.session_type,
        data.desktop,
        data.runner_label,
        launcher_tail,
        runner_tail
    )
}

pub fn copy_log_package_to_clipboard(prefix: &Path, runner_id: &str) -> String {
    let version = env!("CARGO_PKG_VERSION");
    let arch = std::env::consts::ARCH;
    let session_type = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "bilinmiyor".into());
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| "bilinmiyor".into());

    let runner_label = select_runner(runner_id)
        .map(|r| r.label)
        .unwrap_or_else(|| "Otomatik".into());

    let launcher_log_path = revival_launcher_log_path(prefix);
    let runner_log_path = AppConfig::cache_dir().join("runner.log");

    let launcher_raw = fs::read_to_string(&launcher_log_path).ok();
    let runner_raw = fs::read_to_string(&runner_log_path).ok();

    let home = dirs::home_dir().map(|h| h.display().to_string());

    let package = build_log_package(&LogPackageData {
        version,
        arch,
        session_type: &session_type,
        desktop: &desktop,
        runner_label: &runner_label,
        launcher_log: launcher_raw.as_deref(),
        runner_log: runner_raw.as_deref(),
        home_dir: home.as_deref(),
    });

    if let Some(display) = gdk::Display::default() {
        display.clipboard().set_text(&package);
    }

    package
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sensitive_lines_masked() {
        assert_eq!(sanitize_log_line("normal status line"), "normal status line");
        assert_eq!(sanitize_log_line("login password=secret123"), "[gizlendi]");
        assert_eq!(sanitize_log_line("saved PASSWD hash"), "[gizlendi]");
        assert_eq!(sanitize_log_line("bearer Token: abcdef"), "[gizlendi]");
        assert_eq!(
            sanitize_log_line("AuthClient: ticket issued successfully"),
            "[gizlendi]"
        );
        assert_eq!(sanitize_log_line("AuthClient: session active"), "[gizlendi]");
    }

    #[test]
    fn test_extract_log_tail_home_replacement_and_missing() {
        assert_eq!(extract_log_tail(None, 80, Some("/home/player")), "(yok)");
        assert_eq!(extract_log_tail(Some("   "), 80, Some("/home/player")), "(yok)");

        let log = "/home/player/.wine/drive_c\nnormal line\npassWord hidden here";
        let tail = extract_log_tail(Some(log), 10, Some("/home/player"));
        let lines: Vec<&str> = tail.lines().collect();
        assert_eq!(lines[0], "~/.wine/drive_c");
        assert_eq!(lines[1], "normal line");
        assert_eq!(lines[2], "[gizlendi]");
    }

    #[test]
    fn test_build_log_package_structure() {
        let data = LogPackageData {
            version: "0.1.8",
            arch: "x86_64",
            session_type: "wayland",
            desktop: "GNOME",
            runner_label: "Sistem Wine",
            launcher_log: Some("Launcher started"),
            runner_log: None,
            home_dir: Some("/home/user"),
        };
        let pkg = build_log_package(&data);
        assert!(pkg.contains("Sürüm: 0.1.8"));
        assert!(pkg.contains("Mimari: x86_64"));
        assert!(pkg.contains("XDG_SESSION_TYPE: wayland"));
        assert!(pkg.contains("XDG_CURRENT_DESKTOP: GNOME"));
        assert!(pkg.contains("Çalıştırıcı: Sistem Wine"));
        assert!(pkg.contains("Launcher started"));
        assert!(pkg.contains("(yok)"));
    }
}
