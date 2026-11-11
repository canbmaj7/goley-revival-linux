use crate::config::AppConfig;
use crate::prefix::{install_ncrypt_shims, revival_game_dir, revival_root_dir};
use reqwest::blocking::Client;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const API_BASE: &str = "https://api.playrevival.co";
const DEFAULT_PATCH_BASE: &str = "https://patch.playrevival.co/";

#[derive(Debug, Clone, Deserialize)]
pub struct SlidesResponse {
    pub slides: Vec<SlideItem>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct SlideItem {
    pub id: String,
    pub title: String,
    pub description: String,
    pub image_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DecodedImage {
    pub width: i32,
    pub height: i32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct PreparedSlide {
    pub item: SlideItem,
    pub image: Option<DecodedImage>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewsListResponse {
    pub news: Vec<NewsItem>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewsDetailResponse {
    pub news: NewsDetail,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct NewsItem {
    pub id: String,
    pub title: String,
    pub slug: Option<String>,
    pub summary: Option<String>,
    pub published_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct NewsDetail {
    pub id: String,
    pub title: String,
    pub summary: Option<String>,
    pub body: Option<String>,
    pub published_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct PatchManifest {
    pub schema_version: u32,
    pub version: String,
    pub files: Vec<ManifestFile>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ManifestFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone)]
pub struct RemoteManifests {
    pub patch_base_url: String,
    pub launcher: PatchManifest,
    pub game: PatchManifest,
}

#[derive(Debug, Clone)]
pub enum LocalInstallState {
    NotInstalled {
        remote_version: String,
        total_bytes: u64,
        total_files: usize,
    },
    UpdateRequired {
        remote_version: String,
        missing_files: usize,
        download_bytes: u64,
    },
    UpToDate {
        version: String,
    },
}

#[derive(Debug, Clone)]
pub enum PatchProgress {
    Verifying {
        checked: usize,
        total: usize,
        current_file: String,
    },
    Downloading {
        downloaded_bytes: u64,
        total_bytes: u64,
        speed_mbps: f64,
        eta_secs: u64,
        files_done: usize,
        files_total: usize,
    },
}

pub fn build_http_client() -> Result<Client, String> {
    Client::builder()
        .user_agent(concat!("goley-revival-linux/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(12))
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|e| format!("HTTP istemcisi oluşturulamadı: {}", e))
}

pub fn fetch_portal_content() -> Result<(Vec<PreparedSlide>, Vec<NewsItem>), String> {
    let client = build_http_client()?;

    let slides_resp: SlidesResponse = client
        .get(format!("{}/api/v1/content/slides", API_BASE))
        .send()
        .map_err(|e| format!("Sunucuya bağlanılamadı: {}", e))?
        .error_for_status()
        .map_err(|e| format!("Slayt yanıtı hatalı: {}", e))?
        .json()
        .map_err(|e| format!("Slayt JSON ayrıştırılamadı: {}", e))?;

    let news_resp: NewsListResponse = client
        .get(format!("{}/api/v1/content/news?limit=12&offset=0", API_BASE))
        .send()
        .map_err(|e| format!("Duyurular alınamadı: {}", e))?
        .error_for_status()
        .map_err(|e| format!("Duyuru yanıtı hatalı: {}", e))?
        .json()
        .map_err(|e| format!("Duyuru JSON ayrıştırılamadı: {}", e))?;

    let cache_slides_dir = AppConfig::cache_dir().join("slides");
    let _ = fs::create_dir_all(&cache_slides_dir);

    let mut prepared_slides = Vec::new();
    for slide in slides_resp.slides {
        let mut decoded = None;
        if let Some(ref rel_path) = slide.image_path {
            let url = if rel_path.starts_with("http") {
                rel_path.clone()
            } else {
                format!("{}{}", API_BASE, rel_path)
            };
            let cache_name = sanitize_cache_filename(rel_path);
            let cache_file = cache_slides_dir.join(cache_name);

            let img_bytes = if let Ok(bytes) = fs::read(&cache_file) {
                Some(bytes)
            } else if let Ok(resp) = client.get(&url).send() {
                if let Ok(bytes) = resp.bytes() {
                    let vec = bytes.to_vec();
                    let _ = fs::write(&cache_file, &vec);
                    Some(vec)
                } else {
                    None
                }
            } else {
                None
            };

            if let Some(bytes) = img_bytes {
                if let Ok(dyn_img) = image::load_from_memory(&bytes) {
                    let rgba_img = dyn_img.to_rgba8();
                    decoded = Some(DecodedImage {
                        width: rgba_img.width() as i32,
                        height: rgba_img.height() as i32,
                        rgba: rgba_img.into_raw(),
                    });
                }
            }
        }
        prepared_slides.push(PreparedSlide {
            item: slide,
            image: decoded,
        });
    }

    Ok((prepared_slides, news_resp.news))
}

pub fn fetch_news_detail(id: &str) -> Result<NewsDetail, String> {
    let client = build_http_client()?;
    let resp: NewsDetailResponse = client
        .get(format!("{}/api/v1/content/news/{}", API_BASE, id))
        .send()
        .map_err(|e| format!("Duyuru detayı alınamadı: {}", e))?
        .error_for_status()
        .map_err(|e| format!("Duyuru sunucu hatası: {}", e))?
        .json()
        .map_err(|e| format!("Duyuru içeriği okunamadı: {}", e))?;
    Ok(resp.news)
}

pub fn fetch_remote_manifests() -> Result<RemoteManifests, String> {
    let client = build_http_client()?;

    let launcher_resp = client
        .get(format!("{}/api/v1/patch/launcher", API_BASE))
        .send()
        .map_err(|e| format!("Başlatıcı manifestosu alınamadı: {}", e))?
        .error_for_status()
        .map_err(|e| format!("Başlatıcı manifest hatası: {}", e))?;

    let mut patch_base = launcher_resp
        .headers()
        .get("x-patch-base-url")
        .and_then(|h| h.to_str().ok())
        .unwrap_or(DEFAULT_PATCH_BASE)
        .to_string();
    if !patch_base.ends_with('/') {
        patch_base.push('/');
    }

    let launcher: PatchManifest = launcher_resp
        .json()
        .map_err(|e| format!("Başlatıcı manifest JSON hatası: {}", e))?;

    let game: PatchManifest = client
        .get(format!("{}/api/v1/patch/live", API_BASE))
        .send()
        .map_err(|e| format!("Oyun manifestosu alınamadı: {}", e))?
        .error_for_status()
        .map_err(|e| format!("Oyun manifest hatası: {}", e))?
        .json()
        .map_err(|e| format!("Oyun manifest JSON hatası: {}", e))?;

    Ok(RemoteManifests {
        patch_base_url: patch_base,
        launcher,
        game,
    })
}

/// Oyunun kendi yazdığı/yeniden yazdığı dosyalar (Option.ini, shader'lar vb.) manifestteki varsayılandan
/// farklı boyutta olur; var olduktan sonra "eksik/güncel değil" sayılmamalı.
fn is_user_settings_file(rel: &str) -> bool {
    let norm = rel.replace('\\', "/");
    // Oyun, shader dosyalarının ilk satırındaki `#line` yolunu kendi kurulum yoluyla
    // yeniden yazar; boyut manifestten farklı çıkar ama içerik aynıdır.
    let is_game_rewritten_shader = norm.starts_with("Data/DefaultShader/") && norm.ends_with(".hlsl");
    is_game_rewritten_shader
        || crate::userdata::PROTECTED_FILES.iter().any(|p| p.eq_ignore_ascii_case(&norm))
}

pub fn check_install_status_fast(prefix: &Path, manifests: &RemoteManifests) -> LocalInstallState {
    let rev_root = revival_root_dir(prefix);
    let game_dir = revival_game_dir(prefix);

    let launcher_exe = rev_root.join("revival_launcher.exe");
    let game_exe = game_dir.join("BinaryTr/BinaryTrU.exe");

    let total_all_bytes: u64 = manifests.launcher.files.iter().map(|f| f.size).sum::<u64>()
        + manifests.game.files.iter().map(|f| f.size).sum::<u64>();
    let total_all_files = manifests.launcher.files.len() + manifests.game.files.len();

    if !launcher_exe.is_file() && !game_exe.is_file() {
        return LocalInstallState::NotInstalled {
            remote_version: manifests.game.version.clone(),
            total_bytes: total_all_bytes,
            total_files: total_all_files,
        };
    }

    let mut missing_files = 0usize;
    let mut download_bytes = 0u64;

    for f in &manifests.launcher.files {
        let local_path = rev_root.join(&f.path);
        match fs::metadata(&local_path) {
            Ok(meta) if meta.is_file() && meta.len() == f.size => {}
            _ => {
                missing_files += 1;
                download_bytes += f.size;
            }
        }
    }

    for f in &manifests.game.files {
        let local_path = game_dir.join(&f.path);
        if is_user_settings_file(&f.path) && local_path.is_file() {
            continue;
        }
        match fs::metadata(&local_path) {
            Ok(meta) if meta.is_file() && meta.len() == f.size => {}
            _ => {
                missing_files += 1;
                download_bytes += f.size;
            }
        }
    }

    if missing_files > 0 {
        LocalInstallState::UpdateRequired {
            remote_version: manifests.game.version.clone(),
            missing_files,
            download_bytes,
        }
    } else {
        LocalInstallState::UpToDate {
            version: manifests.game.version.clone(),
        }
    }
}

#[derive(Clone)]
struct DownloadTask {
    dest_path: PathBuf,
    rel_name: String,
    size: u64,
    sha256: String,
}

pub fn verify_and_patch<F>(
    prefix: &Path,
    manifests: &RemoteManifests,
    full_sha256_check: bool,
    mut on_progress: F,
) -> Result<usize, String>
where
    F: FnMut(PatchProgress) + Send,
{
    let rev_root = revival_root_dir(prefix);
    let game_dir = revival_game_dir(prefix);

    fs::create_dir_all(&rev_root).map_err(|e| format!("Kurulum dizini açılamadı: {}", e))?;
    fs::create_dir_all(&game_dir).map_err(|e| format!("Oyun dizini açılamadı: {}", e))?;

    let mut all_targets = Vec::with_capacity(manifests.launcher.files.len() + manifests.game.files.len());
    for f in &manifests.launcher.files {
        all_targets.push(DownloadTask {
            dest_path: rev_root.join(&f.path),
            rel_name: f.path.clone(),
            size: f.size,
            sha256: f.sha256.to_lowercase(),
        });
    }
    for f in &manifests.game.files {
        all_targets.push(DownloadTask {
            dest_path: game_dir.join(&f.path),
            rel_name: f.path.clone(),
            size: f.size,
            sha256: f.sha256.to_lowercase(),
        });
    }

    let total_count = all_targets.len();
    let mut to_download = Vec::new();

    for (idx, task) in all_targets.into_iter().enumerate() {
        on_progress(PatchProgress::Verifying {
            checked: idx + 1,
            total: total_count,
            current_file: task.rel_name.clone(),
        });

        let needs_dl = match fs::metadata(&task.dest_path) {
            Ok(meta) if meta.is_file() && is_user_settings_file(&task.rel_name) => false,
            Ok(meta) if meta.is_file() && meta.len() == task.size => {
                if full_sha256_check {
                    match compute_file_sha256(&task.dest_path) {
                        Ok(hash) => hash != task.sha256,
                        Err(_) => true,
                    }
                } else {
                    false
                }
            }
            _ => true,
        };

        if needs_dl {
            to_download.push(task);
        }
    }

    let files_total = to_download.len();
    if files_total == 0 {
        install_ncrypt_shims(prefix)?;
        return Ok(0);
    }

    let total_bytes: u64 = to_download.iter().map(|t| t.size).sum();

    if let Some(available) = get_available_space(&game_dir) {
        let required = total_bytes as f64 * 1.10;
        if (available as f64) < required {
            const GB: f64 = 1024.0 * 1024.0 * 1024.0;
            return Err(format!(
                "Yetersiz disk alanı: {:.1} GB gerekli, {:.1} GB boş",
                required / GB,
                available as f64 / GB
            ));
        }
    }

    let downloaded_bytes = Arc::new(AtomicU64::new(0));
    let files_done = Arc::new(AtomicU64::new(0));
    let cancelled = Arc::new(AtomicBool::new(false));
    let first_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let queue = Arc::new(Mutex::new(to_download));

    let client = build_http_client()?;
    let worker_count = files_total.min(6).max(1);
    let patch_base = manifests.patch_base_url.clone();

    let mut handles = Vec::new();
    for _ in 0..worker_count {
        let q = Arc::clone(&queue);
        let dl_bytes = Arc::clone(&downloaded_bytes);
        let f_done = Arc::clone(&files_done);
        let cancel = Arc::clone(&cancelled);
        let err_slot = Arc::clone(&first_error);
        let http = client.clone();
        let base_url = patch_base.clone();

        handles.push(std::thread::spawn(move || {
            loop {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let next_task = {
                    let mut guard = q.lock().unwrap();
                    guard.pop()
                };
                let Some(task) = next_task else { break };

                if let Err(e) = download_single_object_with_retry(&http, &base_url, &task, &dl_bytes, &cancel) {
                    cancel.store(true, Ordering::Relaxed);
                    let mut g = err_slot.lock().unwrap();
                    if g.is_none() {
                        *g = Some(e);
                    }
                    break;
                }
                f_done.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    let start_instant = Instant::now();
    loop {
        let done = files_done.load(Ordering::Relaxed) as usize;
        let bytes = downloaded_bytes.load(Ordering::Relaxed);
        let elapsed = start_instant.elapsed().as_secs_f64().max(0.1);
        let speed_bps = bytes as f64 / elapsed;
        let speed_mbps = speed_bps / (1024.0 * 1024.0);
        let rem_bytes = total_bytes.saturating_sub(bytes);
        let eta_secs = if speed_bps > 1024.0 {
            (rem_bytes as f64 / speed_bps) as u64
        } else {
            0
        };

        on_progress(PatchProgress::Downloading {
            downloaded_bytes: bytes,
            total_bytes,
            speed_mbps,
            eta_secs,
            files_done: done,
            files_total,
        });

        if done >= files_total || cancelled.load(Ordering::Relaxed) {
            break;
        }
        std::thread::sleep(Duration::from_millis(120));
    }

    for h in handles {
        let _ = h.join();
    }

    if let Some(err) = first_error.lock().unwrap().take() {
        return Err(err);
    }

    // Ensure our x64 + x86 ncrypt.dll shims are freshly deployed after patching!
    install_ncrypt_shims(prefix)?;

    Ok(files_total)
}

fn parse_df_available(output: &str) -> Option<u64> {
    let mut lines = output.lines().filter(|l| !l.trim().is_empty());
    let _header = lines.next()?;
    for line in lines {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 4 {
            if let Ok(available) = parts[3].parse::<u64>() {
                return Some(available);
            }
        }
    }
    None
}

pub(crate) fn get_available_space(path: &Path) -> Option<u64> {
    use std::process::Command;
    let target = if path.exists() {
        path
    } else if let Some(parent) = path.parent() {
        if parent.exists() {
            parent
        } else {
            path
        }
    } else {
        path
    };
    let output = Command::new("df")
        .arg("-P")
        .arg("-B1")
        .arg(target)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_df_available(&stdout)
}

fn part_path_for(dest: &Path) -> PathBuf {
    let mut tmp_name = dest.as_os_str().to_owned();
    tmp_name.push(".goley_part");
    PathBuf::from(tmp_name)
}

fn is_http_client_error(err: &str) -> bool {
    if err.contains("client error") {
        return true;
    }
    for code in 400..=499 {
        let pattern1 = format!("({})", code);
        let pattern2 = format!("({} ", code);
        let pattern3 = format!("status: {}", code);
        let pattern4 = format!("status {}", code);
        if err.contains(&pattern1)
            || err.contains(&pattern2)
            || err.contains(&pattern3)
            || err.contains(&pattern4)
        {
            return true;
        }
    }
    false
}

fn is_http_server_error(err: &str) -> bool {
    if err.contains("server error") {
        return true;
    }
    for code in 500..=599 {
        let pattern1 = format!("({})", code);
        let pattern2 = format!("({} ", code);
        let pattern3 = format!("status: {}", code);
        let pattern4 = format!("status {}", code);
        if err.contains(&pattern1)
            || err.contains(&pattern2)
            || err.contains(&pattern3)
            || err.contains(&pattern4)
        {
            return true;
        }
    }
    false
}

fn is_retryable_error(err: &str) -> bool {
    if err == "İndirme iptal edildi" {
        return false;
    }
    if err.starts_with("İndirme hatası (") {
        return true;
    }
    if err.starts_with("Ağ okuma hatası (") {
        return true;
    }
    if err.starts_with("SHA-256 doğrulama hatası (") {
        return true;
    }
    if err.starts_with("CDN HTTP hatası (") {
        if is_http_client_error(err) {
            return false;
        }
        if is_http_server_error(err) {
            return true;
        }
        return false;
    }
    false
}

fn cancellable_sleep(duration: Duration, cancel: &Arc<AtomicBool>) -> bool {
    let step = Duration::from_millis(50);
    let mut elapsed = Duration::ZERO;
    while elapsed < duration {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        let this_step = step.min(duration - elapsed);
        std::thread::sleep(this_step);
        elapsed += this_step;
    }
    !cancel.load(Ordering::Relaxed)
}

fn run_with_retry<F>(
    mut attempt_fn: F,
    cancel: &Arc<AtomicBool>,
    delays: &[Duration],
) -> Result<(), String>
where
    F: FnMut() -> Result<(), String>,
{
    const MAX_ATTEMPTS: usize = 3;
    for attempt in 1..=MAX_ATTEMPTS {
        if cancel.load(Ordering::Relaxed) {
            return Err("İndirme iptal edildi".to_string());
        }

        match attempt_fn() {
            Ok(()) => return Ok(()),
            Err(err) => {
                if cancel.load(Ordering::Relaxed) || err == "İndirme iptal edildi" {
                    return Err("İndirme iptal edildi".to_string());
                }

                if !is_retryable_error(&err) {
                    return Err(err);
                }

                if attempt >= MAX_ATTEMPTS {
                    return Err(format!("{} (3 deneme sonrası)", err));
                }

                let delay = delays
                    .get(attempt - 1)
                    .copied()
                    .unwrap_or(Duration::from_secs(4));
                if !cancellable_sleep(delay, cancel) {
                    return Err("İndirme iptal edildi".to_string());
                }
            }
        }
    }
    Ok(())
}

fn download_single_object_with_retry(
    client: &Client,
    patch_base: &str,
    task: &DownloadTask,
    dl_bytes: &Arc<AtomicU64>,
    cancel: &Arc<AtomicBool>,
) -> Result<(), String> {
    const RETRY_DELAYS: [Duration; 3] = [
        Duration::from_secs(1),
        Duration::from_secs(2),
        Duration::from_secs(4),
    ];
    run_with_retry(
        || download_single_object(client, patch_base, task, dl_bytes, cancel),
        cancel,
        &RETRY_DELAYS,
    )
}

fn download_single_object(
    client: &Client,
    patch_base: &str,
    task: &DownloadTask,
    dl_bytes: &Arc<AtomicU64>,
    cancel: &Arc<AtomicBool>,
) -> Result<(), String> {
    if let Some(parent) = task.dest_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Klasör hatası ({}): {}", task.rel_name, e))?;
    }

    let url = format!("{}objects/{}", patch_base, task.sha256);
    let mut resp = client
        .get(&url)
        .send()
        .map_err(|e| format!("İndirme hatası ({}): {}", task.rel_name, e))?
        .error_for_status()
        .map_err(|e| format!("CDN HTTP hatası ({}): {}", task.rel_name, e))?;

    let tmp_path = part_path_for(&task.dest_path);
    let mut out_file = fs::File::create(&tmp_path)
        .map_err(|e| format!("Dosya oluşturulamadı ({}): {}", task.rel_name, e))?;

    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    let mut written_this_file = 0u64;

    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = fs::remove_file(&tmp_path);
            dl_bytes.fetch_sub(written_this_file, Ordering::Relaxed);
            return Err("İndirme iptal edildi".to_string());
        }
        let n = match resp.read(&mut buf) {
            Ok(n) => n,
            Err(e) => {
                let _ = fs::remove_file(&tmp_path);
                dl_bytes.fetch_sub(written_this_file, Ordering::Relaxed);
                return Err(format!("Ağ okuma hatası ({}): {}", task.rel_name, e));
            }
        };
        if n == 0 {
            break;
        }
        if let Err(e) = out_file.write_all(&buf[..n]) {
            let _ = fs::remove_file(&tmp_path);
            dl_bytes.fetch_sub(written_this_file, Ordering::Relaxed);
            return Err(format!("Disk yazma hatası ({}): {}", task.rel_name, e));
        }
        hasher.update(&buf[..n]);
        written_this_file += n as u64;
        dl_bytes.fetch_add(n as u64, Ordering::Relaxed);
    }

    out_file.sync_all().ok();
    drop(out_file);

    let got_hash = hex::encode(hasher.finalize());
    if got_hash != task.sha256 {
        let _ = fs::remove_file(&tmp_path);
        dl_bytes.fetch_sub(written_this_file, Ordering::Relaxed);
        return Err(format!(
            "SHA-256 doğrulama hatası ({}): beklenen {}, alınan {}",
            task.rel_name, task.sha256, got_hash
        ));
    }

    fs::rename(&tmp_path, &task.dest_path).map_err(|e| {
        let _ = fs::remove_file(&tmp_path);
        dl_bytes.fetch_sub(written_this_file, Ordering::Relaxed);
        format!("Dosya yerleştirilemedi ({}): {}", task.rel_name, e)
    })
}

pub fn compute_file_sha256(path: &Path) -> Result<String, String> {
    let mut f = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn sanitize_cache_filename(rel: &str) -> String {
    rel.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' { c } else { '_' })
        .collect()
}

pub fn format_turkish_date(iso: Option<&str>) -> String {
    let Some(raw) = iso else {
        return "Duyuru".to_string();
    };
    let date_part = raw.split('T').next().unwrap_or(raw);
    let parts: Vec<&str> = date_part.split('-').collect();
    if parts.len() == 3 {
        let year = parts[0];
        let month = match parts[1] {
            "01" => "Oca",
            "02" => "Şub",
            "03" => "Mar",
            "04" => "Nis",
            "05" => "May",
            "06" => "Haz",
            "07" => "Tem",
            "08" => "Ağu",
            "09" => "Eyl",
            "10" => "Eki",
            "11" => "Kas",
            "12" => "Ara",
            m => m,
        };
        let day = parts[2].trim_start_matches('0');
        return format!("{} {} {}", if day.is_empty() { "1" } else { day }, month, year);
    }
    date_part.to_string()
}

pub fn format_bytes_human(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    if bytes as f64 >= GB {
        format!("{:.2} GB", bytes as f64 / GB)
    } else {
        format!("{:.1} MB", (bytes as f64 / MB).max(0.1))
    }
}

const GITHUB_LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/Fuwia/goley-revival-linux/releases/latest";
const DEFAULT_TARBALL_URL: &str =
    "https://github.com/Fuwia/goley-revival-linux/releases/latest/download/goley-revival-linux-x86_64.tar.gz";
const DEFAULT_APPIMAGE_URL: &str =
    "https://github.com/Fuwia/goley-revival-linux/releases/latest/download/goley-revival-linux-x86_64.AppImage";

#[derive(Debug, Clone, Deserialize)]
struct GithubReleaseDto {
    tag_name: String,
    html_url: Option<String>,
    body: Option<String>,
    #[serde(default)]
    assets: Vec<GithubAssetDto>,
}

#[derive(Debug, Clone, Deserialize)]
struct GithubAssetDto {
    name: String,
    browser_download_url: String,
}

#[derive(Debug, Clone)]
pub struct LauncherReleaseInfo {
    pub current_version: String,
    pub latest_version: String,
    pub tag_name: String,
    pub html_url: String,
    pub body: String,
    pub update_available: bool,
    pub tarball_url: String,
    pub appimage_url: String,
}

fn parse_semver_tuple(ver: &str) -> (u32, u32, u32) {
    let cleaned = ver.trim().trim_start_matches('v').trim_start_matches('V');
    let mut parts = cleaned
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty());
    let major = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let patch = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    (major, minor, patch)
}

pub fn check_github_launcher_update() -> Result<LauncherReleaseInfo, String> {
    let client = build_http_client()?;
    let dto: GithubReleaseDto = client
        .get(GITHUB_LATEST_RELEASE_API)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| format!("GitHub sunucusuna bağlanılamadı: {}", e))?
        .error_for_status()
        .map_err(|e| format!("GitHub sürüm bilgisi alınamadı: {}", e))?
        .json()
        .map_err(|e| format!("GitHub JSON yanıtı ayrıştırılamadı: {}", e))?;

    let current_version = env!("CARGO_PKG_VERSION").to_string();
    let latest_version = dto
        .tag_name
        .trim()
        .trim_start_matches('v')
        .trim_start_matches('V')
        .to_string();

    let cur_tuple = parse_semver_tuple(&current_version);
    let lat_tuple = parse_semver_tuple(&latest_version);
    let update_available = lat_tuple > cur_tuple;

    let mut tarball_url = DEFAULT_TARBALL_URL.to_string();
    let mut appimage_url = DEFAULT_APPIMAGE_URL.to_string();

    for asset in &dto.assets {
        if asset.name.ends_with(".tar.gz") {
            tarball_url = asset.browser_download_url.clone();
        } else if asset.name.ends_with(".AppImage") {
            appimage_url = asset.browser_download_url.clone();
        }
    }

    Ok(LauncherReleaseInfo {
        current_version,
        latest_version,
        tag_name: dto.tag_name,
        html_url: dto.html_url.unwrap_or_else(|| {
            "https://github.com/Fuwia/goley-revival-linux/releases/latest".to_string()
        }),
        body: dto
            .body
            .filter(|b| !b.trim().is_empty())
            .unwrap_or_else(|| "Yeni sürüm iyileştirmeleri ve hata düzeltmeleri.".to_string()),
        update_available,
        tarball_url,
        appimage_url,
    })
}

pub fn perform_launcher_self_update(info: &LauncherReleaseInfo) -> Result<PathBuf, String> {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    let client = build_http_client()?;

    // Case 1: Running inside an AppImage
    if let Ok(appimage_var) = std::env::var("APPIMAGE") {
        let appimage_path = PathBuf::from(appimage_var.trim());
        if !appimage_var.trim().is_empty() && appimage_path.is_file() {
            let bytes = client
                .get(&info.appimage_url)
                .send()
                .map_err(|e| format!("AppImage indirilemedi: {}", e))?
                .error_for_status()
                .map_err(|e| format!("AppImage indirme hatası: {}", e))?
                .bytes()
                .map_err(|e| format!("AppImage verisi okunamadı: {}", e))?;

            if bytes.len() < 100_000 || &bytes[..4] != b"\x7fELF" {
                return Err("İndirilen AppImage dosyası geçersiz görünüyor.".to_string());
            }

            let tmp_path = appimage_path.with_extension(format!("update_tmp_{}", std::process::id()));
            fs::write(&tmp_path, &bytes)
                .map_err(|e| format!("Geçici AppImage yazılamadı: {}", e))?;
            let _ = fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o755));
            fs::rename(&tmp_path, &appimage_path).map_err(|e| {
                let _ = fs::remove_file(&tmp_path);
                format!("AppImage dosyası güncellenemedi: {}", e)
            })?;
            return Ok(appimage_path);
        }
    }

    // Case 2: Standard ELF binary installation (~/.local/bin, /usr/bin, or portable dir)
    let tmp_dir = std::env::temp_dir().join(format!("goley-self-update-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp_dir);
    fs::create_dir_all(&tmp_dir)
        .map_err(|e| format!("Geçici güncelleme klasörü oluşturulamadı: {}", e))?;

    let tarball_path = tmp_dir.join("release.tar.gz");
    let bytes = client
        .get(&info.tarball_url)
        .send()
        .map_err(|e| {
            let _ = fs::remove_dir_all(&tmp_dir);
            format!("Güncelleme paketi indirilemedi: {}", e)
        })?
        .error_for_status()
        .map_err(|e| {
            let _ = fs::remove_dir_all(&tmp_dir);
            format!("Güncelleme paketi HTTP hatası: {}", e)
        })?
        .bytes()
        .map_err(|e| {
            let _ = fs::remove_dir_all(&tmp_dir);
            format!("Güncelleme paketi okunamadı: {}", e)
        })?;

    fs::write(&tarball_path, &bytes).map_err(|e| {
        let _ = fs::remove_dir_all(&tmp_dir);
        format!("Arşiv diske yazılamadı: {}", e)
    })?;

    let status = Command::new("tar")
        .arg("-xzf")
        .arg(&tarball_path)
        .arg("-C")
        .arg(&tmp_dir)
        .status()
        .map_err(|e| {
            let _ = fs::remove_dir_all(&tmp_dir);
            format!("'tar' komutu çalıştırılamadı: {}", e)
        })?;

    if !status.success() {
        let _ = fs::remove_dir_all(&tmp_dir);
        return Err("İndirilen güncelleme arşivi çıkartılamadı.".to_string());
    }

    let extracted_bin = tmp_dir.join("goley-revival-linux");
    let bin_bytes = fs::read(&extracted_bin).map_err(|e| {
        let _ = fs::remove_dir_all(&tmp_dir);
        format!("Çıkartılan ikili dosya okunamadı: {}", e)
    })?;

    if bin_bytes.len() < 100_000 || &bin_bytes[..4] != b"\x7fELF" {
        let _ = fs::remove_dir_all(&tmp_dir);
        return Err("Çıkartılan başlatıcı dosyası geçerli bir ELF ikilisi değil.".to_string());
    }
    let _ = fs::set_permissions(&extracted_bin, fs::Permissions::from_mode(0o755));

    let cur_exe = std::env::current_exe()
        .unwrap_or_else(|_| PathBuf::from("/usr/bin/goley-revival-linux"));

    // Try atomic in-place replacement in current_exe's directory first
    let staged_near_exe = cur_exe.with_extension(format!("new_{}", std::process::id()));
    if fs::write(&staged_near_exe, &bin_bytes).is_ok() {
        let _ = fs::set_permissions(&staged_near_exe, fs::Permissions::from_mode(0o755));
        if fs::rename(&staged_near_exe, &cur_exe).is_ok() {
            let _ = fs::remove_dir_all(&tmp_dir);
            return Ok(cur_exe);
        }
        let _ = fs::remove_file(&staged_near_exe);
    }

    // If current_exe is in a root-owned path (e.g. /usr/bin), try pkexec first
    if crate::config::which_binary("pkexec").is_some() {
        if let Ok(st) = Command::new("pkexec")
            .args([
                "install",
                "-Dm755",
                &extracted_bin.to_string_lossy(),
                &cur_exe.to_string_lossy(),
            ])
            .status()
        {
            if st.success() {
                let _ = fs::remove_dir_all(&tmp_dir);
                return Ok(cur_exe);
            }
        }
    }

    // Fallback: install cleanly into ~/.local/bin/goley-revival-linux
    if let Some(home) = dirs::home_dir() {
        let local_bin_dir = home.join(".local/bin");
        let _ = fs::create_dir_all(&local_bin_dir);
        let target_bin = local_bin_dir.join("goley-revival-linux");
        let staged_local = target_bin.with_extension(format!("new_{}", std::process::id()));
        fs::write(&staged_local, &bin_bytes).map_err(|e| {
            let _ = fs::remove_dir_all(&tmp_dir);
            format!("~/.local/bin içine yazılamadı: {}", e)
        })?;
        let _ = fs::set_permissions(&staged_local, fs::Permissions::from_mode(0o755));
        fs::rename(&staged_local, &target_bin).map_err(|e| {
            let _ = fs::remove_file(&staged_local);
            let _ = fs::remove_dir_all(&tmp_dir);
            format!("~/.local/bin/goley-revival-linux güncellenemedi: {}", e)
        })?;

        // Also update desktop entry if extracted
        let extracted_desktop = tmp_dir.join("io.github.fuwia.GoleyRevivalLinux.desktop");
        if let Ok(desktop_raw) = fs::read_to_string(&extracted_desktop) {
            let app_dir = home.join(".local/share/applications");
            let _ = fs::create_dir_all(&app_dir);
            let patched = desktop_raw
                .lines()
                .map(|l| {
                    if l.starts_with("Exec=") {
                        format!("Exec={}", target_bin.display())
                    } else {
                        l.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
                + "\n";
            let _ = fs::write(
                app_dir.join("io.github.fuwia.GoleyRevivalLinux.desktop"),
                patched,
            );
        }

        let _ = fs::remove_dir_all(&tmp_dir);
        return Ok(target_bin);
    }

    let _ = fs::remove_dir_all(&tmp_dir);
    Err("Güncelleme dosyası hedef konuma yerleştirilemedi.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn test_part_path_for_different_extensions() {
        let vlh = Path::new("/opt/goley/Stadium.VLH");
        let vld = Path::new("/opt/goley/Stadium.VLD");
        let part_vlh = part_path_for(vlh);
        let part_vld = part_path_for(vld);

        assert_ne!(part_vlh, part_vld);
        assert_eq!(part_vlh, PathBuf::from("/opt/goley/Stadium.VLH.goley_part"));
        assert_eq!(part_vld, PathBuf::from("/opt/goley/Stadium.VLD.goley_part"));
        assert!(part_vlh.to_string_lossy().ends_with(".goley_part"));
        assert!(part_vld.to_string_lossy().ends_with(".goley_part"));
    }

    #[test]
    fn test_parse_df_available_valid() {
        let sample = "Filesystem         1-blocks         Used    Available Capacity Mounted on\n\
                      /dev/nvme0n1p2 507813089280 130826940416 375391145984      26% /home\n";
        assert_eq!(parse_df_available(sample), Some(375391145984));
    }

    #[test]
    fn test_parse_df_available_broken() {
        assert_eq!(parse_df_available(""), None);
        assert_eq!(
            parse_df_available("Filesystem 1-blocks Used Available Capacity Mounted on\n"),
            None
        );
        assert_eq!(
            parse_df_available("df: /nonexistent: No such file or directory\n"),
            None
        );
        assert_eq!(
            parse_df_available(
                "Filesystem 1-blocks Used Available Capacity Mounted on\n/dev/sda1 100 50 not_a_number 10% /\n"
            ),
            None
        );
        assert_eq!(parse_df_available("single_token\n"), None);
    }

    #[test]
    fn test_is_retryable_error_classification() {
        // Retryable errors
        assert!(is_retryable_error("İndirme hatası (data.pak): connection timeout"));
        assert!(is_retryable_error("Ağ okuma hatası (data.pak): unexpected EOF"));
        assert!(is_retryable_error(
            "SHA-256 doğrulama hatası (data.pak): beklenen abc, alınan def"
        ));
        assert!(is_retryable_error(
            "CDN HTTP hatası (data.pak): HTTP status server error (500 Internal Server Error)"
        ));
        assert!(is_retryable_error(
            "CDN HTTP hatası (data.pak): HTTP status server error (502 Bad Gateway)"
        ));
        assert!(is_retryable_error(
            "CDN HTTP hatası (data.pak): HTTP status server error (503 Service Unavailable)"
        ));

        // Non-retryable errors
        assert!(!is_retryable_error(
            "CDN HTTP hatası (data.pak): HTTP status client error (404 Not Found)"
        ));
        assert!(!is_retryable_error(
            "CDN HTTP hatası (data.pak): HTTP status client error (403 Forbidden)"
        ));
        assert!(!is_retryable_error(
            "CDN HTTP hatası (data.pak): HTTP status client error (400 Bad Request)"
        ));
        assert!(!is_retryable_error("İndirme iptal edildi"));
        assert!(!is_retryable_error(
            "Disk yazma hatası (data.pak): No space left on device"
        ));
        assert!(!is_retryable_error(
            "Dosya oluşturulamadı (data.pak): Permission denied"
        ));
    }

    #[test]
    fn test_retry_success_on_retry() {
        let cancel = Arc::new(AtomicBool::new(false));
        let mut count = 0;
        let res = run_with_retry(
            || {
                count += 1;
                if count < 2 {
                    Err("Ağ okuma hatası (test): connection reset".to_string())
                } else {
                    Ok(())
                }
            },
            &cancel,
            &[Duration::ZERO, Duration::ZERO],
        );
        assert!(res.is_ok());
        assert_eq!(count, 2);
    }

    #[test]
    fn test_retry_exhausted_appends_suffix() {
        let cancel = Arc::new(AtomicBool::new(false));
        let mut count = 0;
        let res = run_with_retry(
            || {
                count += 1;
                Err("SHA-256 doğrulama hatası (test): beklenen a, alınan b".to_string())
            },
            &cancel,
            &[Duration::ZERO, Duration::ZERO],
        );
        assert_eq!(count, 3);
        match res {
            Err(e) => {
                assert!(e.ends_with("(3 deneme sonrası)"));
                assert!(e.starts_with("SHA-256 doğrulama hatası (test):"));
            }
            Ok(_) => panic!("Expected error after 3 attempts"),
        }
    }

    #[test]
    fn test_retry_4xx_does_not_retry() {
        let cancel = Arc::new(AtomicBool::new(false));
        let mut count = 0;
        let res = run_with_retry(
            || {
                count += 1;
                Err("CDN HTTP hatası (test): HTTP status client error (404 Not Found)".to_string())
            },
            &cancel,
            &[Duration::ZERO, Duration::ZERO],
        );
        assert_eq!(count, 1);
        match res {
            Err(e) => {
                assert!(!e.contains("(3 deneme sonrası)"));
                assert_eq!(
                    e,
                    "CDN HTTP hatası (test): HTTP status client error (404 Not Found)"
                );
            }
            Ok(_) => panic!("Expected error on 404"),
        }
    }

    #[test]
    fn test_retry_cancel_aborts_immediately() {
        let cancel = Arc::new(AtomicBool::new(true));
        let mut count = 0;
        let res = run_with_retry(
            || {
                count += 1;
                Ok(())
            },
            &cancel,
            &[Duration::ZERO, Duration::ZERO],
        );
        assert_eq!(count, 0);
        assert_eq!(res.unwrap_err(), "İndirme iptal edildi");
    }
}
