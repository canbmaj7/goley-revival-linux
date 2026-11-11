use std::fs;
use std::path::{Path, PathBuf};

const GAME_WM_CLASS: &str = "binarytru.exe";

fn entry_contents() -> String {
    format!(
        "[Desktop Entry]\n\
Type=Application\n\
Name=Goley Revival\n\
Comment=Goley Revival oyun penceresi\n\
Exec=goley-revival-linux\n\
Icon=io.github.fuwia.GoleyRevivalLinux\n\
StartupWMClass={GAME_WM_CLASS}\n\
NoDisplay=true\n\
Categories=Game;\n"
    )
}

fn desktop_entry_path() -> Option<PathBuf> {
    dirs::data_local_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/share")))
        .map(|d| d.join("applications/goley-revival-game.desktop"))
}

pub fn ensure_game_window_entry() {
    let Some(path) = desktop_entry_path() else {
        return;
    };
    ensure_game_window_entry_at(&path);
}

pub(crate) fn ensure_game_window_entry_at(path: &Path) {
    let content = entry_contents();

    if let Ok(existing) = fs::read_to_string(path) {
        if existing == content {
            return;
        }
    }

    if let Some(parent) = path.parent() {
        if fs::create_dir_all(parent).is_err() {
            return;
        }
    }

    let _ = fs::write(path, content.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TempDirGuard {
        path: PathBuf,
    }

    impl TempDirGuard {
        fn new(name: &str) -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let id = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "goley_desktop_test_{}_{}_{}_{}",
                std::process::id(),
                name,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0),
                id
            ));
            let _ = fs::remove_dir_all(&path);
            let _ = fs::create_dir_all(&path);
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn test_entry_contents() {
        let content = entry_contents();
        let expected = format!(
            "[Desktop Entry]\n\
Type=Application\n\
Name=Goley Revival\n\
Comment=Goley Revival oyun penceresi\n\
Exec=goley-revival-linux\n\
Icon=io.github.fuwia.GoleyRevivalLinux\n\
StartupWMClass={GAME_WM_CLASS}\n\
NoDisplay=true\n\
Categories=Game;\n"
        );
        assert_eq!(content, expected);
        assert!(content.contains("StartupWMClass=binarytru.exe"));
        assert!(content.contains("NoDisplay=true"));
        assert!(content.contains("Categories=Game;\n"));
    }

    #[test]
    fn test_ensure_game_window_entry_at_creates_and_updates() {
        let t = TempDirGuard::new("desktop_entry");
        let target = t.path().join("applications/goley-revival-game.desktop");

        // 1. Initial write
        ensure_game_window_entry_at(&target);
        let read1 = fs::read_to_string(&target).unwrap_or_default();
        assert_eq!(read1, entry_contents());

        // 2. Overwrite when content is different
        let _ = fs::write(&target, "Different content");
        ensure_game_window_entry_at(&target);
        let read2 = fs::read_to_string(&target).unwrap_or_default();
        assert_eq!(read2, entry_contents());
    }
}
