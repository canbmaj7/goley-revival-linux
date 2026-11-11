use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static TEST_COUNTER: AtomicU64 = AtomicU64::new(1);

fn setup_test_dirs() -> (PathBuf, PathBuf, PathBuf) {
    let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let base = std::env::temp_dir().join(format!(
        "goley_userdata_test_{}_{}_{}",
        std::process::id(),
        id,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let game = base.join("Game");
    let storage = base.join("storage");
    (base, game, storage)
}

#[test]
fn test_pristine_file_not_captured_and_does_not_overwrite_stored() {
    let (base, game, storage) = setup_test_dirs();
    let option_game = game.join("Data/Option.ini");
    let option_storage = storage.join("Data/Option.ini");

    let default_official_bytes = b"[Display]\nResolution=1024x768\nDefault=true\n";
    let default_sha = hex::encode(Sha256::digest(default_official_bytes));
    let mut pristine = HashMap::new();
    pristine.insert("Data/Option.ini".to_string(), default_sha);

    let user_saved_custom_bytes = b"[Display]\nResolution=1920x1080\nCustom=true\n";
    atomic_write_file(&option_storage, user_saved_custom_bytes).ok();
    atomic_write_file(&option_game, default_official_bytes).ok();

    // 1. Pristine game file should NOT be captured and must NOT overwrite the stored user settings
    let cap_msgs = capture_user_modified(&game, &storage, &pristine);
    assert!(cap_msgs.is_empty());
    assert_eq!(
        fs::read(&option_storage).ok(),
        Some(user_saved_custom_bytes.to_vec()),
        "Saklanan kullanıcı ayarı resmî varsayılan dosya tarafından ezilmemeli!"
    );

    // 2. Restore should properly revert the game file to user custom settings
    let res_msgs = restore_from(&storage, &game);
    assert_eq!(res_msgs.len(), 1);
    assert!(res_msgs[0].contains("Oyun ayarları geri yüklendi"));
    assert_eq!(
        fs::read(&option_game).ok(),
        Some(user_saved_custom_bytes.to_vec())
    );

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn test_user_modified_file_is_captured() {
    let (base, game, storage) = setup_test_dirs();
    let option_game = game.join("Data/Option.ini");
    let option_storage = storage.join("Data/Option.ini");

    let default_official_bytes = b"default official Option.ini";
    let default_sha = hex::encode(Sha256::digest(default_official_bytes));
    let mut pristine = HashMap::new();
    pristine.insert("Data/Option.ini".to_string(), default_sha);

    let user_modified_bytes = b"[Display]\nResolution=2560x1440\n";
    atomic_write_file(&option_game, user_modified_bytes).ok();

    let cap_msgs = capture_user_modified(&game, &storage, &pristine);
    assert_eq!(cap_msgs.len(), 1);
    assert!(cap_msgs[0].contains("Oyun ayarları saklandı"));
    assert_eq!(
        fs::read(&option_storage).ok(),
        Some(user_modified_bytes.to_vec())
    );

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn test_identical_to_stored_is_not_rewritten() {
    let (base, game, storage) = setup_test_dirs();
    let option_game = game.join("Data/Option.ini");
    let option_storage = storage.join("Data/Option.ini");

    let default_sha = hex::encode(Sha256::digest(b"official default"));
    let mut pristine = HashMap::new();
    pristine.insert("Data/Option.ini".to_string(), default_sha);

    let custom_bytes = b"[Display]\nResolution=1920x1080\n";
    atomic_write_file(&option_game, custom_bytes).ok();
    atomic_write_file(&option_storage, custom_bytes).ok();

    let cap_msgs = capture_user_modified(&game, &storage, &pristine);
    assert!(cap_msgs.is_empty());

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn test_empty_pristine_manifest_captures_nothing() {
    let (base, game, storage) = setup_test_dirs();
    let option_game = game.join("Data/Option.ini");
    let option_storage = storage.join("Data/Option.ini");

    let user_modified_bytes = b"[Display]\nResolution=1920x1080\n";
    atomic_write_file(&option_game, user_modified_bytes).ok();

    // Pristine map is empty (e.g. manifest fetch failed or not yet received)
    let empty_pristine = HashMap::new();
    let cap_msgs = capture_user_modified(&game, &storage, &empty_pristine);
    assert!(cap_msgs.is_empty());
    assert!(!option_storage.exists());

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn test_favorite_info_ini_is_also_protected() {
    let (base, game, storage) = setup_test_dirs();
    let fav_game = game.join("Data/FavoriteInfo.ini");
    let fav_storage = storage.join("Data/FavoriteInfo.ini");

    let default_fav = b"fav=default";
    let default_sha = hex::encode(Sha256::digest(default_fav));
    let mut pristine = HashMap::new();
    pristine.insert("Data/FavoriteInfo.ini".to_string(), default_sha);

    let user_fav = b"fav=custom_team_data";
    atomic_write_file(&fav_game, user_fav).ok();

    let cap_msgs = capture_user_modified(&game, &storage, &pristine);
    assert_eq!(cap_msgs.len(), 1);
    assert!(cap_msgs[0].contains("Data/FavoriteInfo.ini"));
    assert_eq!(fs::read(&fav_storage).ok(), Some(user_fav.to_vec()));

    // When game file is overwritten with pristine default
    atomic_write_file(&fav_game, default_fav).ok();
    let cap_msgs2 = capture_user_modified(&game, &storage, &pristine);
    assert!(cap_msgs2.is_empty());
    assert_eq!(fs::read(&fav_storage).ok(), Some(user_fav.to_vec()));

    let res_msgs = restore_from(&storage, &game);
    assert_eq!(res_msgs.len(), 1);
    assert_eq!(fs::read(&fav_game).ok(), Some(user_fav.to_vec()));

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn test_restore_without_stored_copy_does_not_touch_file() {
    let (base, game, storage) = setup_test_dirs();
    let option_ini = game.join("Data/Option.ini");
    let initial_content = b"unchanged";
    atomic_write_file(&option_ini, initial_content).ok();

    let res_msgs = restore_from(&storage, &game);
    assert!(res_msgs.is_empty());
    assert_eq!(fs::read(&option_ini).ok(), Some(initial_content.to_vec()));

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn test_missing_files_do_not_panic() {
    let (base, game, storage) = setup_test_dirs();
    let pristine = HashMap::new();

    let cap_msgs = capture_user_modified(&game, &storage, &pristine);
    assert!(cap_msgs.is_empty());

    let res_msgs = restore_from(&storage, &game);
    assert!(res_msgs.is_empty());

    let _ = fs::remove_dir_all(&base);
}
