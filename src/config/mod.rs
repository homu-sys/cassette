use std::{
    collections::HashMap,
    fmt, fs,
    io::ErrorKind,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

mod theme;

pub use theme::ThemeKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendKind {
    Invidious,
    Piped,
}

impl fmt::Display for BackendKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invidious => write!(formatter, "Invidious"),
            Self::Piped => write!(formatter, "Piped"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    pub backend: BackendKind,
    pub invidious_instances: Vec<String>,
    pub piped_instances: Vec<String>,
    pub local_dirs: Vec<PathBuf>,
    pub volume: f32,
    #[serde(default)]
    pub theme: ThemeKind,
    #[serde(default = "default_playlist_header_url")]
    pub playlist_header_url: String,
}

fn default_playlist_header_url() -> String {
    "https://raw.githubusercontent.com/homu-sys/cassette-playlists-public/main/header.json".into()
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct VideoProgressEntry {
    #[serde(default)]
    pub position: f64,
    #[serde(default)]
    pub duration: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct VideoProgressFile {
    #[serde(default)]
    pub videos: HashMap<String, VideoProgressEntry>,
}

impl Default for Config {
    fn default() -> Self {
        let local_dirs = dirs::audio_dir()
            .filter(|path| path.exists())
            .into_iter()
            .collect();
        Self {
            backend: BackendKind::Invidious,
            invidious_instances: vec!["https://yt.chocolatemoo53.com".into()],
            piped_instances: vec![
                "https://pipedapi.ducks.party".into(),
                "https://api.piped.private.coffee".into(),
            ],
            local_dirs,
            volume: 0.7,
            theme: ThemeKind::default(),
            playlist_header_url: default_playlist_header_url(),
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("cassette")
            .join("config.toml")
    }

    pub fn load() -> Result<Self> {
        let path = Self::path();
        match fs::read_to_string(&path) {
            Ok(contents) => toml::from_str(&contents).context("could not parse config"),
            Err(error) if error.kind() == ErrorKind::NotFound => {
                let config = Self::default();
                config.save()?;
                Ok(config)
            }
            Err(error) => Err(error).context("could not read config"),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).context("could not create config directory")?;
        }
        let contents = toml::to_string_pretty(self).context("could not serialize config")?;
        fs::write(&path, contents).context("could not write config")?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .context("could not secure config")?;
        Ok(())
    }
}

pub fn expand_tilde(value: &str) -> PathBuf {
    if value == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from(value));
    }
    if let Some(rest) = value.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(value)
}

pub fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("cassette")
        .join("art")
}

pub fn audio_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("cassette")
        .join("music")
}

pub fn runtime_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("cassette")
        .join("runtime")
}

pub fn mpv_log_path() -> PathBuf {
    runtime_dir().join("mpv.log")
}

pub fn audio_cache_path(id: &str) -> PathBuf {
    audio_cache_dir().join(format!("{}.m4a", sanitize_cache_key(id)))
}

pub fn video_audio_cache_path(id: &str) -> PathBuf {
    audio_cache_dir().join(format!("{}-video.ogg", sanitize_cache_key(id)))
}

fn sanitize_cache_key(id: &str) -> String {
    id.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect()
}

pub fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("could not create cache directory")?;
    }
    Ok(())
}

pub fn video_progress_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("cassette")
        .join("video-progress.json")
}

pub fn load_video_progress() -> VideoProgressFile {
    let path = video_progress_path();
    match fs::read_to_string(&path) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_else(|error| {
            crate::cassette_log!(
                "WARN",
                "video progress parse failed path={} error={error}",
                path.display()
            );
            VideoProgressFile::default()
        }),
        Err(error) if error.kind() == ErrorKind::NotFound => VideoProgressFile::default(),
        Err(error) => {
            crate::cassette_log!(
                "WARN",
                "video progress read failed path={} error={error}",
                path.display()
            );
            VideoProgressFile::default()
        }
    }
}

pub fn save_video_progress(progress: &VideoProgressFile) -> Result<()> {
    let path = video_progress_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("could not create video progress directory")?;
    }
    let temp = path.with_extension("json.tmp");
    let contents =
        serde_json::to_string_pretty(progress).context("could not serialize video progress")?;
    fs::write(&temp, contents).context("could not write video progress")?;
    fs::rename(&temp, &path).context("could not replace video progress")?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
        .context("could not secure video progress")?;
    Ok(())
}
