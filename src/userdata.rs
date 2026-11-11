use crate::prefix::{atomic_write_file, revival_game_dir};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const PROTECTED_FILES: &[&str] = &["Data/Option.ini", "Data/FavoriteInfo.ini"];

pub fn userdata_dir() -> PathBuf {
    crate::config::AppConfig::data_dir().join("userdata")
}

pub fn capture(prefix: &Path, pristine: &HashMap<String, String>) -> Vec<String> {
    capture_user_modified(&revival_game_dir(prefix), &userdata_dir(), pristine)
}

pub fn capture_user_modified(
    game_dir: &Path,
    storage_dir: &Path,
    pristine: &HashMap<String, String>,
) -> Vec<String> {
    let mut messages = Vec::new();
    if pristine.is_empty() {
        return messages;
    }

    for rel in PROTECTED_FILES {
        let pristine_sha = pristine
            .get(*rel)
            .or_else(|| pristine.get(&rel.replace('/', "\\")));
        let Some(pristine_sha) = pristine_sha else {
            continue;
        };

        let src = game_dir.join(rel);
        if !src.is_file() {
            continue;
        }

        let Ok(src_bytes) = fs::read(&src) else {
            continue;
        };

        let file_sha = hex::encode(Sha256::digest(&src_bytes));
        if file_sha.eq_ignore_ascii_case(pristine_sha) {
            continue;
        }

        let dst = storage_dir.join(rel);
        if dst.is_file() {
            if let Ok(dst_bytes) = fs::read(&dst) {
                if src_bytes == dst_bytes {
                    continue;
                }
            }
        }

        match atomic_write_file(&dst, &src_bytes) {
            Ok(()) => messages.push(format!("[goley-revival-linux] Oyun ayarları saklandı: {}", rel)),
            Err(e) => messages.push(format!("[goley-revival-linux] Oyun ayarları saklanamadı ({}): {}", rel, e)),
        }
    }
    messages
}

pub fn restore(prefix: &Path) -> Vec<String> {
    restore_from(&userdata_dir(), &revival_game_dir(prefix))
}

pub(crate) fn restore_from(storage_dir: &Path, game_dir: &Path) -> Vec<String> {
    let mut messages = Vec::new();
    for rel in PROTECTED_FILES {
        let src = storage_dir.join(rel);
        if !src.is_file() {
            continue;
        }
        let Ok(src_bytes) = fs::read(&src) else {
            continue;
        };
        let dst = game_dir.join(rel);
        if dst.is_file() {
            if let Ok(dst_bytes) = fs::read(&dst) {
                if src_bytes == dst_bytes {
                    continue;
                }
            }
        }
        match atomic_write_file(&dst, &src_bytes) {
            Ok(()) => messages.push(format!("[goley-revival-linux] Oyun ayarları geri yüklendi: {}", rel)),
            Err(e) => messages.push(format!("[goley-revival-linux] Oyun ayarları geri yüklenemedi ({}): {}", rel, e)),
        }
    }
    messages
}

#[cfg(test)]
#[path = "userdata_tests.rs"]
mod tests;
