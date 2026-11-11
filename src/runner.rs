use crate::config::{which_binary, AppConfig};
use crate::prefix::{
    atomic_write_file, find_running_pids, install_ncrypt_shims, resolve_wine_user_dir,
    revival_launcher_log_path, revival_local_appdata_dir, revival_root_dir, select_runner,
    write_login_prefs_to_user_reg, RunnerKind,
};
use crate::userdata;
use async_channel::Sender;
use std::fs;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct LoginCredentials {
    pub username: String,
    pub password: String,
    pub remember: bool,
}

#[derive(Debug, Clone)]
pub enum RunnerEvent {
    LogLine(String),
    Status {
        headline: String,
        detail: String,
        pulsing: bool,
    },
    NeedLogin {
        reason: Option<String>,
    },
    LoginSucceeded {
        username: String,
    },
    LaunchError(String),
    GameRunning {
        pid: u32,
    },
    GameExited,
}

// Allow short process handovers without mistaking them for a launcher crash.
struct LaunchWatch {
    seen_launcher: bool,
    missing_since: Option<Instant>,
    phase: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeStateSnapshot {
    pub log_len: u64,
    pub status_content: Option<String>,
}

pub fn has_bridge_progress(last: &BridgeStateSnapshot, current: &BridgeStateSnapshot) -> bool {
    current.log_len > last.log_len || current.status_content != last.status_content
}

impl LaunchWatch {
    fn new(existing_launcher: bool, phase: &'static str) -> Self {
        Self {
            seen_launcher: existing_launcher,
            missing_since: None,
            phase,
        }
    }

    fn launcher_exited(&mut self, running: bool, now: Instant) -> bool {
        if running {
            self.seen_launcher = true;
            self.missing_since = None;
            return false;
        }
        if !self.seen_launcher {
            return false;
        }
        let since = self.missing_since.get_or_insert(now);
        now.duration_since(*since) >= Duration::from_secs(3)
    }
}

pub fn send_bridge_command(prefix: &Path, cmd: &str) -> Result<(), String> {
    let dir = revival_local_appdata_dir(prefix);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let cmd_file = dir.join("goley_bridge_cmd.txt");
    atomic_write_file(&cmd_file, cmd.as_bytes())
}

pub fn stop_running_game(prefix: &Path) {
    let _ = send_bridge_command(prefix, "QUIT\n");
    let (game_pid, launcher_pid) = find_running_pids();
    if let Some(pid) = game_pid {
        let _ = Command::new("kill")
            .args(["-15", &pid.to_string()])
            .status();
    }
    if let Some(pid) = launcher_pid {
        let _ = Command::new("kill")
            .args(["-15", &pid.to_string()])
            .status();
    }
}

fn stop_launcher_only(prefix: &Path) {
    let _ = send_bridge_command(prefix, "QUIT\n");
    for _ in 0..5 {
        if find_running_pids().1.is_none() {
            return;
        }
        std::thread::sleep(Duration::from_millis(80));
    }
    if let Some(pid) = find_running_pids().1 {
        let _ = Command::new("kill")
            .args(["-15", &pid.to_string()])
            .status();
        for _ in 0..6 {
            if find_running_pids().1.is_none() {
                return;
            }
            std::thread::sleep(Duration::from_millis(80));
        }
    }
}

fn detect_runner_version(kind: &RunnerKind) -> String {
    match kind {
        RunnerKind::Wine { wine_bin } => Command::new(wine_bin)
            .arg("--version")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "bilinmiyor".to_string()),
        RunnerKind::Umu {
            umu_bin,
            proton_dir,
        } => {
            let umu_ver = Command::new(umu_bin)
                .arg("--version")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.lines().next().unwrap_or("").trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "umu-run".to_string());
            let p_name = proton_dir
                .as_ref()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str())
                .unwrap_or("varsayılan");
            format!("{} / {}", umu_ver, p_name)
        }
        RunnerKind::ProtonDirect { proton_script } => proton_script
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or("Proton")
            .to_string(),
    }
}

pub fn launch_headless_session(
    cfg: AppConfig,
    creds: Option<LoginCredentials>,
    tx: Sender<RunnerEvent>,
) {
    std::thread::spawn(move || {
        if let Err(err) = run_session_inner(&cfg, creds, &tx) {
            let _ = tx.send_blocking(RunnerEvent::LogLine(format!(
                "[goley-revival-linux] Başlatma hatası: {}",
                err
            )));
            let _ = tx.send_blocking(RunnerEvent::LaunchError(err));
        }
    });
}

fn run_session_inner(
    cfg: &AppConfig,
    creds: Option<LoginCredentials>,
    tx: &Sender<RunnerEvent>,
) -> Result<(), String> {
    let prefix = &cfg.prefix_path;
    let rev_root = revival_root_dir(prefix);
    let launcher_exe = rev_root.join("revival_launcher.exe");

    let _ = tx.send_blocking(RunnerEvent::LogLine(format!(
        "[goley-revival-linux] Sürüm: {} | Mimari: {} | Oturum: {} | Masaüstü: {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::ARCH,
        std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "bilinmiyor".into()),
        std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| "bilinmiyor".into()),
    )));

    let pristine: std::collections::HashMap<String, String> = match crate::api::fetch_remote_manifests() {
        Ok(manifests) => {
            let mut map = std::collections::HashMap::new();
            for f in &manifests.game.files {
                let normalized = f.path.replace('\\', "/");
                if userdata::PROTECTED_FILES.contains(&normalized.as_str()) {
                    map.insert(normalized, f.sha256.to_lowercase());
                }
            }
            map
        }
        Err(err) => {
            let _ = tx.send_blocking(RunnerEvent::LogLine(format!(
                "[goley-revival-linux] Resmî sunucu manifestosu alınamadı ({}); oyun ayarları yakalaması bu oturumda atlanıyor.",
                err
            )));
            std::collections::HashMap::new()
        }
    };

    for msg in userdata::capture(prefix, &pristine) {
        let _ = tx.send_blocking(RunnerEvent::LogLine(msg));
    }

    if !launcher_exe.is_file() {
        return Err(
            "revival_launcher.exe bulunamadı. Lütfen önce 'Oyunu İndir / Güncelle' butonuna tıklayın."
                .to_string(),
        );
    }

    // 1. Atomically install x64 + x86 ncrypt.dll shims
    install_ncrypt_shims(prefix)?;

    let (existing_game_pid, mut existing_launcher_pid) = find_running_pids();
    if let Some(gpid) = existing_game_pid {
        let _ = tx.send_blocking(RunnerEvent::GameRunning { pid: gpid });
        monitor_until_exit(prefix, &pristine, tx);
        return Ok(());
    }

    // If the user clicked "OYNA" freshly (creds.is_none()) while a stale background
    // revival_launcher.exe is still lingering (e.g. after a previous launch error or
    // runner setting change), restart it cleanly so current runner & shims take effect.
    if existing_launcher_pid.is_some() && creds.is_none() {
        let _ = tx.send_blocking(RunnerEvent::LogLine(
            "[goley-revival-linux] Önceki arka plan başlatıcısı kapatılıp güncel ayarlarla yeniden başlatılıyor..."
                .to_string(),
        ));
        stop_launcher_only(prefix);
        existing_launcher_pid = find_running_pids().1;
    }

    let log_path = revival_launcher_log_path(prefix);
    let mut log_offset: u64 = 0;

    // 2. If revival_launcher.exe is waiting invisibly in background for login credentials
    if existing_launcher_pid.is_some() {
        let _ = tx.send_blocking(RunnerEvent::LogLine(format!(
            "[goley-revival-linux] Mevcut arka plan başlatıcısı kullanılıyor (Prefix: {}).",
            prefix.display()
        )));
        if let Ok(meta) = fs::metadata(&log_path) {
            log_offset = meta.len();
        }
        for msg in userdata::restore(prefix) {
            let _ = tx.send_blocking(RunnerEvent::LogLine(msg));
        }
        if let Some(ref c) = creds {
            let _ = tx.send_blocking(RunnerEvent::Status {
                headline: "Hesaba giriş yapılıyor...".to_string(),
                detail: format!("{} hesabı doğrulanıyor", c.username),
                pulsing: true,
            });
            send_bridge_command(prefix, &format!("LOGIN\t{}\t{}\n", c.username, c.password))?;
        } else {
            let _ = tx.send_blocking(RunnerEvent::Status {
                headline: "Oyun başlatılıyor...".to_string(),
                detail: "Arka plan oturumundan bilet isteniyor".to_string(),
                pulsing: true,
            });
            send_bridge_command(prefix, "PLAY\n")?;
        }
    } else {
        // Fresh headless spawn
        let Some(runner) = select_runner(&cfg.runner_id) else {
            return Err(
                "Uyumluluk katmanı (UMU-Run, Proton veya Wine) bulunamadı! Ayarlar'dan kontrol edin."
                    .to_string(),
            );
        };

        // Pre-populate username in user.reg if credentials provided
        if let Some(ref c) = creds {
            let _ = write_login_prefs_to_user_reg(prefix, &c.username, c.remember);
        }

        // Clear stale IPC files & rotate previous launcher.log so we only parse fresh lines
        let bridge_dir = revival_local_appdata_dir(prefix);
        let _ = fs::remove_file(bridge_dir.join("goley_bridge_cmd.txt"));
        let _ = fs::remove_file(bridge_dir.join("goley_bridge_status.txt"));
        if log_path.is_file() {
            let prev = log_path.with_extension("prev.log");
            let _ = fs::rename(&log_path, prev);
        }

        let _ = tx.send_blocking(RunnerEvent::Status {
            headline: "Goley Revival Başlatılıyor...".to_string(),
            detail: format!("{} çalıştırılıyor", runner.label),
            pulsing: true,
        });

        let runner_ver = detect_runner_version(&runner.kind);
        let mut cmd = build_runner_command(cfg, &runner.kind, &launcher_exe)?;
        let _ = tx.send_blocking(RunnerEvent::LogLine(format!(
            "[goley-revival-linux] Başlatılıyor: {} [{}] (Prefix: {})",
            runner.label,
            runner_ver,
            prefix.display()
        )));

        let runner_log = crate::config::AppConfig::cache_dir().join("runner.log");
        let _ = fs::create_dir_all(crate::config::AppConfig::cache_dir());
        let (out, err) = match fs::File::create(&runner_log) {
            Ok(f) => match f.try_clone() {
                Ok(f2) => (Stdio::from(f), Stdio::from(f2)),
                Err(_) => (Stdio::null(), Stdio::null()),
            },
            Err(_) => (Stdio::null(), Stdio::null()),
        };
        cmd.current_dir(&rev_root)
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err);

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Çalıştırıcı başlatılamadı: {}", e))?;
        // Reap the wrapper even if this session returns early to request a login.
        let exit_tx = tx.clone();
        std::thread::spawn(move || {
            if let Ok(status) = child.wait() {
                let _ = exit_tx.send_blocking(RunnerEvent::LogLine(format!(
                    "[goley-revival-linux] Çalıştırıcı işlemi sonlandı: {}",
                    status
                )));
            }
        });
    }

    // 3. Tail launcher.log and drive the headless bridge state machine
    let start_wait = Instant::now();
    let mut phase_ready = existing_launcher_pid.is_some();
    let mut restored_settings = existing_launcher_pid.is_some();
    let mut ready_since: Option<Instant> = if existing_launcher_pid.is_some() {
        Some(Instant::now())
    } else {
        None
    };
    let initial_phase = if existing_launcher_pid.is_some() {
        if creds.is_some() {
            "hesaba giriş"
        } else {
            "OYNA komutuna yanıt"
        }
    } else {
        "resmi başlatıcının açılması"
    };
    let mut watch = LaunchWatch::new(existing_launcher_pid.is_some(), initial_phase);
    let mut no_stored_session = false;
    let mut login_sent = existing_launcher_pid.is_some() && creds.is_some();
    let mut play_sent_at: Option<Instant> = if existing_launcher_pid.is_some() && creds.is_none() {
        Some(Instant::now())
    } else {
        None
    };
    let mut ticket_requested = false;
    let status_path = revival_local_appdata_dir(prefix).join("goley_bridge_status.txt");
    let mut bridge_cmd_sent = existing_launcher_pid.is_some();
    let mut last_bridge_progress_at = Instant::now();
    let mut last_bridge_snapshot = BridgeStateSnapshot {
        log_len: fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0),
        status_content: fs::read_to_string(&status_path).ok(),
    };

    loop {
        let cur_snapshot = BridgeStateSnapshot {
            log_len: fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0),
            status_content: fs::read_to_string(&status_path).ok(),
        };
        if has_bridge_progress(&last_bridge_snapshot, &cur_snapshot) {
            last_bridge_snapshot = cur_snapshot;
            last_bridge_progress_at = Instant::now();
        } else if bridge_cmd_sent && last_bridge_progress_at.elapsed() >= Duration::from_secs(20) {
            let err_msg = format!(
                "Gizli başlatıcı komuta yanıt vermiyor. Wine kullanıcı klasörü: {}. Log: {}",
                revival_local_appdata_dir(prefix).display(),
                log_path.display()
            );
            let _ = tx.send_blocking(RunnerEvent::LogLine(err_msg.clone()));
            stop_launcher_only(prefix);
            return Err(err_msg);
        }
        // Check if BinaryTrU.exe is already running!
        let (game_pid, launcher_pid) = find_running_pids();
        if let Some(gpid) = game_pid {
            let _ = send_bridge_command(prefix, "SLEEP_UI\n");
            let _ = tx.send_blocking(RunnerEvent::LogLine(format!(
                "[goley-revival-linux] BinaryTrU.exe çalışıyor (PID {}).",
                gpid
            )));
            let _ = tx.send_blocking(RunnerEvent::GameRunning { pid: gpid });
            monitor_until_exit(prefix, &pristine, tx);
            return Ok(());
        }

        if watch.launcher_exited(launcher_pid.is_some(), Instant::now()) {
            return Err(format!(
                "Resmi başlatıcı oyun açılmadan kapandı. Son aşama: {}. Ayrıntılar için >_ Günlükler penceresini açın.",
                watch.phase
            ));
        }

        // No launcher was observed; the wrapper may have failed or startup may be slow.
        if !watch.seen_launcher && start_wait.elapsed() > Duration::from_secs(20) {
            return Err(
                "Resmi başlatıcı 20 sn içinde algılanamadı. Ayarlar'daki Wine/Proton seçimini kontrol edin; ayrıntılar >_ Günlükler penceresinde."
                    .to_string()
            );
        }

        // Timeout check (allow 150s in case launcher downloads small patches)
        if start_wait.elapsed() > Duration::from_secs(150) {
            return Err(format!(
                "Başlatma zaman aşımına uğradı (150 sn). Son aşama: {}. Ayrıntılar için >_ Günlükler penceresini açın.",
                watch.phase
            ));
        }

        // Read newly appended lines from launcher.log
        if let Ok(mut file) = fs::File::open(&log_path) {
            if let Ok(meta) = file.metadata() {
                if meta.len() < log_offset {
                    log_offset = 0;
                }
                if meta.len() > log_offset && file.seek(SeekFrom::Start(log_offset)).is_ok() {
                    let mut reader = BufReader::new(file);
                    let mut line = String::new();
                    while let Ok(bytes_read) = reader.read_line(&mut line) {
                        if bytes_read == 0 {
                            break;
                        }
                        log_offset += bytes_read as u64;
                        let trimmed = line.trim().to_string();
                        line.clear();
                        if trimmed.is_empty() {
                            continue;
                        }

                        let _ = tx.send_blocking(RunnerEvent::LogLine(trimmed.clone()));

                        if trimmed.contains("PatchUpdater: checking") {
                            watch.phase = "oyun dosyalarının denetlenmesi";
                            let _ = tx.send_blocking(RunnerEvent::Status {
                                headline: "Oyun dosyaları denetleniyor...".to_string(),
                                detail: "Resmi sunucu manifestosu doğrulanıyor".to_string(),
                                pulsing: true,
                            });
                        } else if trimmed.contains("phase Verifying -> Downloading")
                            || trimmed.contains("game files to download")
                        {
                            watch.phase = "ek yama dosyalarının indirilmesi";
                            let _ = tx.send_blocking(RunnerEvent::Status {
                                headline: "Ek yama dosyaları indiriliyor...".to_string(),
                                detail: trimmed.clone(),
                                pulsing: true,
                            });
                        } else if trimmed.contains("AuthClient: no stored session")
                            || trimmed.contains("AuthClient: dropping the stored session")
                            || trimmed.contains("AuthClient: restore request failed")
                            || trimmed.contains("AuthClient: restore failed")
                        {
                            no_stored_session = true;
                        } else if trimmed.contains("-> Ready") {
                            watch.phase = "başlatıcının hazır olması";
                            phase_ready = true;
                            ready_since = Some(Instant::now());
                            if !restored_settings {
                                restored_settings = true;
                                for msg in userdata::restore(prefix) {
                                    let _ = tx.send_blocking(RunnerEvent::LogLine(msg));
                                }
                            }
                        } else if let Some(idx) = trimmed.find("AuthClient: login failed ->") {
                            let reason = trimmed[idx + "AuthClient: login failed ->".len()..]
                                .trim()
                                .to_string();
                            let _ = tx.send_blocking(RunnerEvent::NeedLogin {
                                reason: Some(reason),
                            });
                            return Ok(());
                        } else if trimmed.contains("AuthClient: login succeeded") {
                            no_stored_session = false;
                            let uname = creds
                                .as_ref()
                                .map(|c| c.username.clone())
                                .unwrap_or_else(|| "Oyuncu".to_string());
                            let _ =
                                tx.send_blocking(RunnerEvent::LoginSucceeded { username: uname });
                            let _ = tx.send_blocking(RunnerEvent::Status {
                                headline: "Giriş başarılı!".to_string(),
                                detail: "Oyun oturumu başlatılıyor...".to_string(),
                                pulsing: true,
                            });
                            std::thread::sleep(Duration::from_millis(350));
                            if !restored_settings {
                                restored_settings = true;
                                for msg in userdata::restore(prefix) {
                                    let _ = tx.send_blocking(RunnerEvent::LogLine(msg));
                                }
                            }
                            send_bridge_command(prefix, "PLAY\n")?;
                            watch.phase = "OYNA komutuna yanıt";
                            play_sent_at = Some(Instant::now());
                            bridge_cmd_sent = true;
                            last_bridge_progress_at = Instant::now();
                        } else if trimmed.contains("AuthClient: ticket requested") {
                            watch.phase = "sunucudan giriş bileti yanıtı";
                            ticket_requested = true;
                            let _ = tx.send_blocking(RunnerEvent::Status {
                                headline: "Sunucudan giriş bileti alınıyor...".to_string(),
                                detail: "Bilet isteği gönderildi, sunucunun yanıtı bekleniyor"
                                    .to_string(),
                                pulsing: true,
                            });
                        } else if trimmed.contains("AuthClient: ticket issued") {
                            watch.phase = "oyun başlangıç verisinin hazırlanması (GameLauncher)";
                            ticket_requested = true;
                            let _ = tx.send_blocking(RunnerEvent::Status {
                                headline: "Giriş bileti alındı, oyun hazırlanıyor...".to_string(),
                                detail:
                                    "Oyun başlangıç verisi hazırlanıyor ve BinaryTrU.exe başlatılıyor"
                                        .to_string(),
                                pulsing: true,
                            });
                        } else if let Some(idx) = trimmed.find("ticket failed") {
                            let reason = trimmed[idx..].to_string();
                            stop_launcher_only(prefix);
                            return Err(format!("Oyun bileti alınamadı: {}", reason));
                        } else if trimmed.contains("GameLauncher: cannot start the game")
                            || trimmed.contains("RevivalLauncher: game launch failed")
                            || trimmed.contains("GameLauncher: the game failed to start")
                            || trimmed.contains("GameLauncher: game executable not found")
                        {
                            stop_launcher_only(prefix);
                            return Err(format!(
                                "Resmi başlatıcı oyunu başlatamadı ({}). Ayrıntılar için >_ Günlükler penceresini açın.",
                                trimmed
                            ));
                        } else if trimmed.contains("GameLauncher: starting the game")
                            || trimmed.contains("GameLauncher: game started")
                        {
                            watch.phase = "oyun işleminin (BinaryTrU.exe) açılması";
                            ticket_requested = true;
                            let _ = tx.send_blocking(RunnerEvent::Status {
                                headline: "Goley Revival açılıyor...".to_string(),
                                detail: "İyi oyunlar!".to_string(),
                                pulsing: true,
                            });
                        }
                    }
                }
            }
        }

        // Act on Ready phase
        if phase_ready {
            let elapsed_ready = ready_since.map(|t| t.elapsed()).unwrap_or_default();

            if no_stored_session {
                if let Some(ref c) = creds {
                    if !login_sent && elapsed_ready >= Duration::from_millis(400) {
                        login_sent = true;
                        watch.phase = "hesaba giriş";
                        let _ = tx.send_blocking(RunnerEvent::Status {
                            headline: "Hesaba giriş yapılıyor...".to_string(),
                            detail: format!("{} doğrulanıyor", c.username),
                            pulsing: true,
                        });
                        if !restored_settings {
                            restored_settings = true;
                            for msg in userdata::restore(prefix) {
                                let _ = tx.send_blocking(RunnerEvent::LogLine(msg));
                            }
                        }
                        send_bridge_command(
                            prefix,
                            &format!("LOGIN\t{}\t{}\n", c.username, c.password),
                        )?;
                        bridge_cmd_sent = true;
                        last_bridge_progress_at = Instant::now();
                    }
                } else if elapsed_ready >= Duration::from_millis(300) {
                    let _ = tx.send_blocking(RunnerEvent::NeedLogin { reason: None });
                    return Ok(());
                }
            } else if play_sent_at.is_none() && elapsed_ready >= Duration::from_millis(650) {
                let _ = tx.send_blocking(RunnerEvent::Status {
                    headline: "Oyun oturumu başlatılıyor...".to_string(),
                    detail: "Gizli köprü üzerinden OYNA komutu gönderildi".to_string(),
                    pulsing: true,
                });
                if !restored_settings {
                    restored_settings = true;
                    for msg in userdata::restore(prefix) {
                        let _ = tx.send_blocking(RunnerEvent::LogLine(msg));
                    }
                }
                send_bridge_command(prefix, "PLAY\n")?;
                watch.phase = "OYNA komutuna yanıt";
                play_sent_at = Some(Instant::now());
                bridge_cmd_sent = true;
                last_bridge_progress_at = Instant::now();
            } else if let Some(sent_time) = play_sent_at {
                // Retry PLAY if ticket wasn't requested after 2.4s (e.g., session restore just finished)
                if !ticket_requested && sent_time.elapsed() >= Duration::from_millis(2400) {
                    send_bridge_command(prefix, "PLAY\n")?;
                    play_sent_at = Some(Instant::now());
                }
            }
        }

        std::thread::sleep(Duration::from_millis(180));
    }
}

fn monitor_until_exit(
    prefix: &Path,
    pristine: &std::collections::HashMap<String, String>,
    tx: &Sender<RunnerEvent>,
) {
    loop {
        std::thread::sleep(Duration::from_millis(1500));
        let (game_pid, _) = find_running_pids();
        if game_pid.is_none() {
            // Game closed -> cleanly close hidden revival_launcher.exe too
            let _ = send_bridge_command(prefix, "QUIT\n");
            for msg in userdata::capture(prefix, pristine) {
                let _ = tx.send_blocking(RunnerEvent::LogLine(msg));
            }
            let _ = tx.send_blocking(RunnerEvent::LogLine(
                "[goley-revival-linux] Oyun kapandı, arka plan başlatıcısı sonlandırıldı."
                    .to_string(),
            ));
            let _ = tx.send_blocking(RunnerEvent::GameExited);
            break;
        } else {
            for msg in userdata::capture(prefix, pristine) {
                let _ = tx.send_blocking(RunnerEvent::LogLine(msg));
            }
        }
    }
}

fn build_runner_command(
    cfg: &AppConfig,
    kind: &RunnerKind,
    launcher_exe: &Path,
) -> Result<Command, String> {
    let mut wrappers: Vec<String> = Vec::new();

    if cfg.use_gamemode && which_binary("gamemoderun").is_some() {
        wrappers.push("gamemoderun".to_string());
    }
    if cfg.use_mangohud && which_binary("mangohud").is_some() {
        wrappers.push("mangohud".to_string());
    }
    if cfg.use_gamescope && which_binary("gamescope").is_some() {
        wrappers.push("gamescope".to_string());
        for arg in cfg.gamescope_args.split_whitespace() {
            wrappers.push(arg.to_string());
        }
        wrappers.push("--".to_string());
    }

    let mut base_args: Vec<String> = Vec::new();
    match kind {
        RunnerKind::Umu {
            umu_bin,
            proton_dir,
        } => {
            base_args.push(umu_bin.to_string_lossy().to_string());
            base_args.push(launcher_exe.to_string_lossy().to_string());
            let mut cmd = assemble_wrapped_command(&wrappers, &base_args);
            cmd.env("WINEPREFIX", &cfg.prefix_path);
            cmd.env("GAMEID", "revival");
            cmd.env("FAUGUSID", "revival");
            cmd.env("LOG_DIR", "revival");
            if let Some(pdir) = proton_dir {
                // umu 1.4.x, yalnızca klasör adını (ör. "proton-ge-custom") çözemiyor;
                // her zaman tam yol ver.
                cmd.env("PROTONPATH", pdir);
            }
            apply_common_envs(&mut cmd, cfg);
            Ok(cmd)
        }
        RunnerKind::ProtonDirect { proton_script } => {
            base_args.push(proton_script.to_string_lossy().to_string());
            base_args.push("waitforexitandrun".to_string());
            base_args.push(launcher_exe.to_string_lossy().to_string());
            let mut cmd = assemble_wrapped_command(&wrappers, &base_args);
            cmd.env("STEAM_COMPAT_DATA_PATH", &cfg.prefix_path);
            let steam_client = dirs::home_dir()
                .map(|h| h.join(".local/share/Steam"))
                .unwrap_or_else(|| cfg.prefix_path.clone());
            cmd.env("STEAM_COMPAT_CLIENT_INSTALL_PATH", steam_client);
            apply_common_envs(&mut cmd, cfg);
            Ok(cmd)
        }
        RunnerKind::Wine { wine_bin } => {
            base_args.push(wine_bin.to_string_lossy().to_string());
            base_args.push(launcher_exe.to_string_lossy().to_string());
            let mut cmd = assemble_wrapped_command(&wrappers, &base_args);
            cmd.env("WINEPREFIX", &cfg.prefix_path);
            let wine_user = resolve_wine_user_dir(&cfg.prefix_path);
            let user_name = wine_user
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("steamuser");
            cmd.env("USER", user_name);
            cmd.env("LOGNAME", user_name);
            apply_common_envs(&mut cmd, cfg);
            Ok(cmd)
        }
    }
}

fn assemble_wrapped_command(wrappers: &[String], base_args: &[String]) -> Command {
    if wrappers.is_empty() {
        let mut c = Command::new(&base_args[0]);
        c.args(&base_args[1..]);
        c
    } else {
        let mut c = Command::new(&wrappers[0]);
        c.args(&wrappers[1..]);
        c.args(base_args);
        c
    }
}

fn apply_common_envs(cmd: &mut Command, cfg: &AppConfig) {
    cmd.env("WINEDEBUG", "-all");
    cmd.env(
        "WINEDLLOVERRIDES",
        "ncrypt=n,b;bcrypt=b;d3d9,d3dx9_42,d3dx9_31,d3dcompiler_42,d3d10core,d3d11,dxgi=n,b;winedbg.exe=d",
    );
    cmd.env("SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS", "0");
    cmd.env("GOLEY_HEADLESS", "1");
    if cfg.show_debug_window {
        cmd.env("GOLEY_ALLOW_WINDOW", "1");
    } else {
        cmd.env_remove("GOLEY_ALLOW_WINDOW");
    }

    for token in cfg.extra_env.split_whitespace() {
        if let Some((k, v)) = token.split_once('=') {
            if !k.is_empty() {
                cmd.env(k, v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_launcher_during_initial_startup_is_not_a_crash() {
        let now = Instant::now();
        let mut watch = LaunchWatch::new(false, "başlatma");
        assert!(!watch.launcher_exited(false, now));
        assert!(!watch.launcher_exited(false, now + Duration::from_secs(30)));
        assert!(!watch.seen_launcher);
    }

    #[test]
    fn launcher_disappearance_is_reported_after_grace_period() {
        let now = Instant::now();
        let mut watch = LaunchWatch::new(false, "bilet yanıtı");
        assert!(!watch.launcher_exited(true, now));
        assert!(!watch.launcher_exited(false, now + Duration::from_secs(1)));
        assert!(!watch.launcher_exited(false, now + Duration::from_secs(3)));
        assert!(watch.launcher_exited(false, now + Duration::from_secs(4)));
    }

    #[test]
    fn brief_process_handover_resets_the_grace_period() {
        let now = Instant::now();
        let mut watch = LaunchWatch::new(true, "giriş");
        assert!(!watch.launcher_exited(false, now));
        assert!(!watch.launcher_exited(true, now + Duration::from_secs(2)));
        assert!(!watch.launcher_exited(false, now + Duration::from_secs(5)));
        assert!(!watch.launcher_exited(false, now + Duration::from_secs(7)));
        assert!(watch.launcher_exited(false, now + Duration::from_secs(8)));
    }

    #[test]
    fn bridge_progress_detected_on_log_growth() {
        let s1 = BridgeStateSnapshot {
            log_len: 100,
            status_content: None,
        };
        let s2 = BridgeStateSnapshot {
            log_len: 120,
            status_content: None,
        };
        assert!(has_bridge_progress(&s1, &s2));
    }

    #[test]
    fn bridge_progress_detected_on_status_change() {
        let s1 = BridgeStateSnapshot {
            log_len: 100,
            status_content: None,
        };
        let s2 = BridgeStateSnapshot {
            log_len: 100,
            status_content: Some("BUSY".into()),
        };
        assert!(has_bridge_progress(&s1, &s2));

        let s3 = BridgeStateSnapshot {
            log_len: 100,
            status_content: Some("READY".into()),
        };
        assert!(has_bridge_progress(&s2, &s3));
    }

    #[test]
    fn bridge_no_progress_when_unchanged_or_shrunk() {
        let s1 = BridgeStateSnapshot {
            log_len: 100,
            status_content: Some("IDLE".into()),
        };
        let s2 = BridgeStateSnapshot {
            log_len: 100,
            status_content: Some("IDLE".into()),
        };
        assert!(!has_bridge_progress(&s1, &s2));

        let shrunk = BridgeStateSnapshot {
            log_len: 50,
            status_content: Some("IDLE".into()),
        };
        assert!(!has_bridge_progress(&s1, &shrunk));
    }

    #[test]
    fn test_wine_runner_command_sets_user_and_logname() {
        let tmp = std::env::temp_dir().join(format!("goley_runner_wine_test_{}", std::process::id()));
        let _ = fs::create_dir_all(&tmp);
        let mut cfg = AppConfig::default();
        cfg.prefix_path = tmp.clone();

        let kind = RunnerKind::Wine {
            wine_bin: std::path::PathBuf::from("/usr/bin/wine"),
        };
        let launcher = tmp.join("revival_launcher.exe");
        let cmd = build_runner_command(&cfg, &kind, &launcher).unwrap();

        let envs: std::collections::HashMap<String, String> = cmd
            .get_envs()
            .filter_map(|(k, v)| {
                v.map(|val| (k.to_string_lossy().to_string(), val.to_string_lossy().to_string()))
            })
            .collect();

        assert_eq!(envs.get("USER").map(|s| s.as_str()), Some("steamuser"));
        assert_eq!(envs.get("LOGNAME").map(|s| s.as_str()), Some("steamuser"));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_wine_runner_command_sets_legacy_user_env() {
        let tmp = std::env::temp_dir().join(format!("goley_runner_legacy_test_{}", std::process::id()));
        let user_launcher_dir = tmp.join("drive_c/users/kullanici/AppData/Local/Programs/Revival");
        let _ = fs::create_dir_all(&user_launcher_dir);
        let _ = fs::write(user_launcher_dir.join("revival_launcher.exe"), b"");

        let mut cfg = AppConfig::default();
        cfg.prefix_path = tmp.clone();

        let kind = RunnerKind::Wine {
            wine_bin: std::path::PathBuf::from("/usr/bin/wine"),
        };
        let launcher = tmp.join("revival_launcher.exe");
        let cmd = build_runner_command(&cfg, &kind, &launcher).unwrap();

        let envs: std::collections::HashMap<String, String> = cmd
            .get_envs()
            .filter_map(|(k, v)| {
                v.map(|val| (k.to_string_lossy().to_string(), val.to_string_lossy().to_string()))
            })
            .collect();

        assert_eq!(envs.get("USER").map(|s| s.as_str()), Some("kullanici"));
        assert_eq!(envs.get("LOGNAME").map(|s| s.as_str()), Some("kullanici"));
        let _ = fs::remove_dir_all(&tmp);
    }
}
