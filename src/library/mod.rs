use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::Result;
use lofty::{prelude::*, read_from_path};

use crate::{
    config::{cache_dir, ensure_parent},
    model::{Track, TrackSource},
};

const EXTENSIONS: &[&str] = &[
    "mp3", "flac", "ogg", "oga", "opus", "wav", "m4a", "mp4", "aac", "wma", "aiff",
];

pub fn scan_dirs(paths: &[PathBuf]) -> Result<Vec<Track>> {
    crate::cassette_log!("INFO", "local scan start paths={}", paths.len());
    let cache = cache_dir();
    ensure_parent(&cache)?;
    let mut tracks = Vec::new();
    for path in paths {
        if path.is_dir() {
            crate::cassette_log!("INFO", "local scan directory path={}", path.display());
            let _ = scan_dir(path, &cache, &mut tracks);
        } else if path.is_file()
            && let Some(track) = track_from_path(path, &cache)
        {
            crate::cassette_log!("INFO", "local scan file path={}", path.display());
            tracks.push(track);
        } else {
            crate::cassette_log!("WARN", "local scan skipped path={}", path.display());
        }
    }
    tracks.sort_by(|left, right| left.title.cmp(&right.title));
    crate::cassette_log!("INFO", "local scan finished tracks={}", tracks.len());
    Ok(tracks)
}

fn scan_dir(path: &Path, cache: &Path, tracks: &mut Vec<Track>) -> Result<()> {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) => {
            crate::cassette_log!(
                "WARN",
                "local scan read_dir failed path={:?} error={error}",
                path
            );
            return Ok(());
        }
    };
    crate::cassette_log!("DEBUG", "local scan entering path={}", path.display());
    for entry in entries.flatten() {
        let child = entry.path();
        if child.is_dir() {
            let _ = scan_dir(&child, cache, tracks);
        } else if is_audio(&child)
            && let Some(track) = track_from_path(&child, cache)
        {
            tracks.push(track);
        }
    }
    Ok(())
}

fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str()))
}

fn track_from_path(path: &Path, cache: &Path) -> Option<Track> {
    let tagged = match read_from_path(path) {
        Ok(tagged) => tagged,
        Err(error) => {
            crate::cassette_log!("WARN", "metadata read failed path={:?} error={error}", path);
            return None;
        }
    };
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
    let fallback = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let duration = tagged.properties().duration();
    let thumbnail_path = tag
        .and_then(|tag| tag.pictures().first())
        .and_then(|picture| cache_picture(picture.data(), path, cache).ok());

    let track = Track {
        id: hash(path.to_string_lossy().as_ref()),
        title: tag
            .and_then(|tag| tag.title())
            .map(|value| value.to_string())
            .unwrap_or(fallback),
        artist: tag
            .and_then(|tag| tag.artist())
            .map(|value| value.to_string())
            .unwrap_or_default(),
        album: tag
            .and_then(|tag| tag.album())
            .map(|value| value.to_string())
            .unwrap_or_default(),
        duration: Duration::from_millis(duration.as_millis() as u64),
        thumbnail_url: None,
        thumbnail_path,
        stream_url: None,
        path: Some(path.to_path_buf()),
        source: TrackSource::Local,
        audio_tracks: Vec::new(),
    };
    crate::cassette_log!(
        "DEBUG",
        "local track loaded id={} title={:?} artist={:?} duration_ms={}",
        track.id,
        track.title,
        track.artist,
        duration.as_millis()
    );
    Some(track)
}

fn cache_picture(bytes: &[u8], source: &Path, cache: &Path) -> Result<PathBuf> {
    let key = format!("local-{}", hash(source.to_string_lossy().as_ref()));
    let destination = cache.join(format!("{key}.png"));
    if !destination.exists() {
        let image = image::load_from_memory(bytes)?.to_rgba8();
        image.save(&destination)?;
    }
    Ok(destination)
}

fn hash(value: &str) -> String {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}
