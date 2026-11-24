use std::{env, fs, path::PathBuf, time::Duration};

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{
    config,
    model::{Playlist, Track},
};

#[derive(Debug, Deserialize)]
struct Header {
    #[allow(dead_code)]
    version: u32,
    playlists: Vec<PlaylistRef>,
}

#[derive(Debug, Deserialize)]
struct PlaylistRef {
    id: String,
    title: String,
    path: String,
    #[serde(default)]
    folders: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PlaylistFile {
    id: String,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    folders: Vec<String>,
    tracks: Vec<TrackFile>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct TrackFile {
    title: String,
    #[serde(default)]
    artist: String,
    #[serde(default)]
    album: String,
    video_id: String,
    #[serde(default)]
    duration_seconds: u64,
    #[serde(default)]
    thumbnail_url: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Cache {
    playlists: Vec<PlaylistFile>,
}

pub async fn load(header_url: &str) -> Result<Vec<PlaylistFile>> {
    let http = Client::builder()
        .user_agent(concat!("cassette-playlists/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(20))
        .build()
        .context("could not create playlist HTTP client")?;
    let header = fetch_json::<Header>(&http, header_url).await?;
    let base = Url::parse(header_url).context("invalid playlist header URL")?;
    let mut playlists = Vec::new();
    for reference in header.playlists {
        let url = base
            .join(&reference.path)
            .with_context(|| format!("invalid playlist path {}", reference.path))?;
        let mut playlist = fetch_json::<PlaylistFile>(&http, url.as_str()).await?;
        if playlist.id.is_empty() {
            playlist.id = reference.id;
        }
        if playlist.title.is_empty() {
            playlist.title = reference.title;
        }
        if !reference.folders.is_empty() {
            playlist.folders = reference.folders;
        }
        playlists.push(playlist);
    }
    save_cache(&playlists)?;
    Ok(playlists)
}

pub fn load_cache() -> Vec<PlaylistFile> {
    let Ok(bytes) = fs::read(cache_path()) else {
        return Vec::new();
    };
    match serde_json::from_slice::<Cache>(&bytes) {
        Ok(cache) => cache.playlists,
        Err(error) => {
            crate::cassette_log!("WARN", "playlist cache parse failed error={error}");
            Vec::new()
        }
    }
}

pub fn into_playlists(
    files: Vec<PlaylistFile>,
    source: crate::model::TrackSource,
) -> Vec<Playlist> {
    files
        .into_iter()
        .map(|playlist| Playlist {
            id: playlist.id,
            title: playlist.title,
            description: playlist.description,
            folders: playlist.folders,
            tracks: playlist
                .tracks
                .into_iter()
                .map(|track| Track {
                    id: track.video_id,
                    title: track.title,
                    artist: if track.artist.is_empty() {
                        "Vivziepop".into()
                    } else {
                        track.artist
                    },
                    album: track.album,
                    duration: Duration::from_secs(track.duration_seconds),
                    thumbnail_url: track.thumbnail_url,
                    thumbnail_path: None,
                    stream_url: None,
                    path: None,
                    source,
                    audio_tracks: Vec::new(),
                })
                .collect(),
        })
        .collect()
}

async fn fetch_json<T: for<'de> Deserialize<'de>>(http: &Client, url: &str) -> Result<T> {
    crate::cassette_log!("INFO", "playlist fetch start url={}", redact_url(url));
    let mut request = http.get(url);
    if let Ok(token) = env::var("CASSETTE_GITHUB_TOKEN").or_else(|_| env::var("GITHUB_TOKEN")) {
        if !token.is_empty() {
            request = request.bearer_auth(token);
        }
    }
    let response = request
        .send()
        .await
        .with_context(|| format!("could not fetch {}", redact_url(url)))?
        .error_for_status()
        .with_context(|| format!("playlist fetch failed for {}", redact_url(url)))?;
    let bytes = response
        .bytes()
        .await
        .context("could not read playlist response")?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("could not parse playlist JSON from {}", redact_url(url)))
}

fn cache_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("cassette")
        .join("playlists.json")
}

fn save_cache(playlists: &[PlaylistFile]) -> Result<()> {
    let path = cache_path();
    config::ensure_parent(&path)?;
    let bytes = serde_json::to_vec_pretty(&Cache {
        playlists: playlists.to_vec(),
    })
    .context("could not serialize playlist cache")?;
    fs::write(&path, bytes).context("could not save playlist cache")?;
    Ok(())
}

fn redact_url(url: &str) -> String {
    match Url::parse(url) {
        Ok(mut parsed) => {
            if parsed.password().is_some() {
                let _ = parsed.set_password(None);
            }
            let _ = parsed.set_username("");
            parsed.to_string()
        }
        Err(_) => url.to_string(),
    }
}
