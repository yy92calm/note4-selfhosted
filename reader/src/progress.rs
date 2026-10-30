//! 进度记忆：books/progress.json

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Progress {
    pub book: String,
    pub chapter: usize,
    pub page: usize,
}

fn path() -> PathBuf {
    // 从项目根运行用 books/，从 reader/ 运行用 ../books/
    if std::path::Path::new("books").is_dir() {
        PathBuf::from("books/progress.json")
    } else {
        PathBuf::from("../books/progress.json")
    }
}

pub fn load() -> Vec<Progress> {
    std::fs::read(path())
        .ok()
        .and_then(|s| serde_json::from_slice(&s).ok())
        .unwrap_or_default()
}

pub fn save(p: &Progress) -> Result<()> {
    let mut all = load();
    if let Some(slot) = all.iter_mut().find(|x| x.book == p.book) {
        *slot = p.clone();
    } else {
        all.push(p.clone());
    }
    if let Some(dir) = path().parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path(), serde_json::to_vec_pretty(&all)?)?;
    Ok(())
}
