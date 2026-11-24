use std::{fmt, path::PathBuf, time::Duration};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackSource {
    Invidious,
    Piped,
    Local,
}

impl fmt::Display for TrackSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invidious => write!(formatter, "Invidious"),
            Self::Piped => write!(formatter, "Piped"),
            Self::Local => write!(formatter, "Local"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AudioTrack {
    pub id: String,
    pub label: String,
    pub language: String,
    pub kind: String,
    pub url: String,
    pub codec: String,
    pub bitrate: u64,
    pub sample_rate: u64,
    pub channels: u64,
}

#[derive(Clone, Debug)]
pub struct VideoSource {
    pub video_url: String,
    pub hls_url: Option<String>,
    pub quality: Option<String>,
    pub audio_tracks: Vec<AudioTrack>,
}

#[derive(Clone, Debug)]
pub struct Playlist {
    pub id: String,
    pub title: String,
    pub description: String,
    pub folders: Vec<String>,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: Duration,
    pub thumbnail_url: Option<String>,
    pub thumbnail_path: Option<PathBuf>,
    pub stream_url: Option<String>,
    pub path: Option<PathBuf>,
    pub source: TrackSource,
    pub audio_tracks: Vec<AudioTrack>,
}

impl Track {
    pub fn duration_label(&self) -> String {
        let seconds = self.duration.as_secs();
        format!("{:02}:{:02}", seconds / 60, seconds % 60)
    }
}
