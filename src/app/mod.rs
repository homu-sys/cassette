use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use image::DynamicImage;
use ratatui::layout::Size;
use ratatui_image::protocol::Protocol;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::{
    api::MusicClient,
    art::ArtLoader,
    config::{
        BackendKind, Config, VideoProgressEntry, VideoProgressFile, ensure_parent, expand_tilde,
        load_video_progress, save_video_progress, video_audio_cache_path,
    },
    library,
    model::{Playlist, Track, VideoSource},
    player::{PlaySource, PlayerEvent, PlayerHandle},
    video::{VideoEvent, VideoHandle},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Search,
    Local,
    Playlists,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputKind {
    Search,
    AddDir,
}

pub enum CatalogEntry<'a> {
    Folder(&'a str),
    Playlist(&'a Playlist),
}

pub enum AppEvent {
    Art {
        id: u64,
        image: Option<DynamicImage>,
    },
    Search {
        generation: u64,
        result: Result<Vec<Track>>,
    },
    Scan {
        result: Result<Vec<Track>>,
    },
}

pub struct App {
    pub config: Config,
    pub client: MusicClient,
    pub player: PlayerHandle,
    pub video: VideoHandle,
    pub art: ArtLoader,
    pub tab: Tab,
    pub search_results: Vec<Track>,
    pub local_tracks: Vec<Track>,
    pub playlists: Vec<Playlist>,
    pub catalog_path: Vec<String>,
    pub active_playlist: Option<String>,
    pub selected: usize,
    pub selected_audio_track: usize,
    pub input: String,
    pub input_kind: Option<InputKind>,
    pub status: String,
    pub playing: Option<Track>,
    pub progress: (f32, f32),
    pub paused: bool,
    pub art_protocol: Option<Protocol>,
    pub video_source: Option<VideoSource>,
    pub video_audio_label: Option<String>,
    video_progress_state: VideoProgressFile,
    video_progress_saved_at: Option<Instant>,
    video_progress_saved_position: f64,
    video_auto_next: Option<u64>,
    pub protocol_label: String,
    pub should_quit: bool,
    events_tx: UnboundedSender<AppEvent>,
    events_rx: UnboundedReceiver<AppEvent>,
    search_generation: u64,
    art_id: u64,
    art_key: Option<String>,
}

impl App {
    pub fn new(
        config: Config,
        client: MusicClient,
        player: PlayerHandle,
        video: VideoHandle,
        art: ArtLoader,
        protocol_label: String,
        local_tracks: Vec<Track>,
        playlists: Vec<Playlist>,
    ) -> Self {
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        Self {
            config,
            client,
            player,
            video,
            art,
            tab: Tab::Search,
            search_results: Vec::new(),
            local_tracks,
            playlists,
            catalog_path: Vec::new(),
            active_playlist: None,
            selected: 0,
            selected_audio_track: 0,
            input: String::new(),
            input_kind: None,
            status: "Ready. Type / to search YouTube Music.".into(),
            playing: None,
            progress: (0.0, 0.0),
            paused: false,
            art_protocol: None,
            video_source: None,
            video_audio_label: None,
            video_progress_state: load_video_progress(),
            video_progress_saved_at: None,
            video_progress_saved_position: 0.0,
            video_auto_next: None,
            protocol_label,
            should_quit: false,
            events_tx,
            events_rx,
            search_generation: 0,
            art_id: 0,
            art_key: None,
        }
    }

    pub fn visible_tracks(&self) -> &[Track] {
        match self.tab {
            Tab::Search => &self.search_results,
            Tab::Local => &self.local_tracks,
            Tab::Playlists => self
                .active_playlist()
                .map(|playlist| playlist.tracks.as_slice())
                .unwrap_or(&[]),
        }
    }

    pub fn selected_track(&self) -> Option<&Track> {
        self.visible_tracks().get(self.selected)
    }

    pub fn active_audio_track_label(&self) -> Option<&str> {
        if self.video_source.is_some() {
            return self.video_audio_label.as_deref();
        }
        self.playing
            .as_ref()?
            .audio_tracks
            .get(self.selected_audio_track)
            .map(|track| track.label.as_str())
    }

    pub fn active_playlist(&self) -> Option<&Playlist> {
        self.active_playlist
            .as_ref()
            .and_then(|id| self.playlists.iter().find(|playlist| playlist.id == *id))
    }

    pub fn catalog_breadcrumb(&self) -> String {
        if self.catalog_path.is_empty() {
            "Premade playlists".into()
        } else {
            format!("Premade playlists / {}", self.catalog_path.join(" / "))
        }
    }

    pub fn visible_catalog_entries(&self) -> Vec<CatalogEntry<'_>> {
        let mut folder_names: Vec<&str> = Vec::new();
        let mut playlist_entries: Vec<&Playlist> = Vec::new();
        let path = self.catalog_path.as_slice();
        for playlist in &self.playlists {
            if playlist.folders.len() < path.len() || &playlist.folders[..path.len()] != path {
                continue;
            }
            if playlist.folders.len() == path.len() {
                playlist_entries.push(playlist);
            } else {
                let name = playlist.folders[path.len()].as_str();
                if !folder_names.contains(&name) {
                    folder_names.push(name);
                }
            }
        }
        let mut entries = Vec::with_capacity(folder_names.len() + playlist_entries.len());
        for name in folder_names {
            entries.push(CatalogEntry::Folder(name));
        }
        for playlist in playlist_entries {
            entries.push(CatalogEntry::Playlist(playlist));
        }
        entries
    }

    pub fn visible_catalog_count(&self) -> usize {
        self.visible_catalog_entries().len()
    }

    pub fn selected_catalog_playlist(&self) -> Option<&Playlist> {
        match self.visible_catalog_entries().get(self.selected) {
            Some(CatalogEntry::Playlist(playlist)) => Some(*playlist),
            _ => None,
        }
    }

    pub fn selected_catalog_folder(&self) -> Option<&str> {
        match self.visible_catalog_entries().get(self.selected) {
            Some(CatalogEntry::Folder(name)) => Some(*name),
            _ => None,
        }
    }

    pub fn selected_playlist(&self) -> Option<&Playlist> {
        if self.tab != Tab::Playlists || self.active_playlist.is_some() {
            return None;
        }
        self.selected_catalog_playlist()
    }

    pub fn open_catalog_item(&mut self) {
        if let Some(folder) = self.selected_catalog_folder().map(str::to_string) {
            self.catalog_path.push(folder.clone());
            self.selected = 0;
            self.art_key = None;
            self.art_protocol = None;
            self.status = format!("Folder: {folder}");
            crate::cassette_log!("INFO", "catalog folder opened path={:?}", self.catalog_path);
            self.refresh_art();
            return;
        }
        let Some((id, title, count)) = self.selected_catalog_playlist().map(|playlist| {
            (
                playlist.id.clone(),
                playlist.title.clone(),
                playlist.tracks.len(),
            )
        }) else {
            self.status = "No playlist selected".into();
            return;
        };
        self.active_playlist = Some(id);
        self.selected = 0;
        self.art_key = None;
        self.art_protocol = None;
        self.status = format!("{title} · {count} episodes");
        crate::cassette_log!("INFO", "playlist opened title={:?} tracks={}", title, count);
        self.refresh_art();
    }

    pub fn close_catalog_folder(&mut self) {
        if self.active_playlist.is_some() {
            self.close_playlist();
            return;
        }
        if self.catalog_path.pop().is_none() {
            return;
        }
        self.selected = 0;
        self.art_key = None;
        self.art_protocol = None;
        self.status = self.catalog_breadcrumb();
        crate::cassette_log!("INFO", "catalog folder closed path={:?}", self.catalog_path);
        self.refresh_art();
    }

    pub fn reset_catalog(&mut self) {
        self.close_playlist();
        self.catalog_path.clear();
        self.selected = 0;
        self.art_key = None;
        self.art_protocol = None;
    }

    pub fn close_playlist(&mut self) {
        if self.active_playlist.is_none() {
            return;
        }
        self.active_playlist = None;
        self.selected = 0;
        self.art_key = None;
        self.art_protocol = None;
        self.status = self.catalog_breadcrumb();
        crate::cassette_log!("INFO", "playlist closed");
        self.refresh_art();
    }

    pub fn move_selection(&mut self, delta: isize) {
        let count = if self.tab == Tab::Playlists && self.active_playlist.is_none() {
            self.visible_catalog_count()
        } else {
            self.visible_tracks().len()
        };
        if count == 0 {
            self.selected = 0;
            crate::cassette_log!("DEBUG", "selection moved to empty list delta={}", delta);
            return;
        }
        self.selected = if delta >= 0 {
            (self.selected + delta as usize).min(count - 1)
        } else {
            self.selected.saturating_sub(delta.unsigned_abs())
        };
        crate::cassette_log!(
            "DEBUG",
            "selection moved delta={} index={} count={}",
            delta,
            self.selected,
            count
        );
        self.refresh_art();
    }

    pub fn set_tab(&mut self, tab: Tab) {
        if self.tab != tab {
            self.tab = tab;
            self.selected = 0;
            if tab != Tab::Playlists {
                self.active_playlist = None;
            }
            self.refresh_art();
        }
    }

    pub fn begin_input(&mut self, kind: InputKind) {
        crate::cassette_log!("INFO", "input started kind={:?}", kind);
        self.input_kind = Some(kind);
        self.input.clear();
    }

    pub fn cancel_input(&mut self) {
        crate::cassette_log!("INFO", "input cancelled kind={:?}", self.input_kind);
        self.input_kind = None;
        self.input.clear();
    }

    pub fn push_input(&mut self, character: char) {
        self.input.push(character);
        crate::cassette_log!("TRACE", "input buffer={:?}", self.input);
    }

    pub fn pop_input(&mut self) {
        crate::cassette_log!("TRACE", "input backspace before={:?}", self.input);
        self.input.pop();
    }

    pub fn submit_input(&mut self) {
        let Some(kind) = self.input_kind else {
            return;
        };
        let value = self.input.trim().to_string();
        crate::cassette_log!("INFO", "input submitted kind={:?} value={:?}", kind, value);
        self.input_kind = None;
        self.input.clear();
        if value.is_empty() {
            crate::cassette_log!("INFO", "input ignored because it was empty");
            return;
        }

        match kind {
            InputKind::Search => {
                self.search_generation += 1;
                let generation = self.search_generation;
                let client = self.client.clone();
                let tx = self.events_tx.clone();
                self.status = format!("Searching for {value}...");
                crate::cassette_log!(
                    "INFO",
                    "search requested generation={} query={:?} active={}",
                    generation,
                    value,
                    self.client.instance_label()
                );
                tokio::spawn(async move {
                    let result = client.search(&value).await;
                    let _ = tx.send(AppEvent::Search { generation, result });
                });
            }
            InputKind::AddDir => {
                let path = expand_tilde(&value);
                crate::cassette_log!("INFO", "local dir requested path={}", path.display());
                if !self.config.local_dirs.contains(&path) {
                    self.config.local_dirs.push(path.clone());
                    crate::cassette_log!("INFO", "local dir added path={}", path.display());
                } else {
                    crate::cassette_log!(
                        "INFO",
                        "local dir already configured path={}",
                        path.display()
                    );
                }
                let dirs = self.config.local_dirs.clone();
                if let Err(error) = self.config.save() {
                    crate::cassette_log!("ERROR", "config save failed error={error}");
                    self.status = format!("Could not save config: {error}");
                }
                let tx = self.events_tx.clone();
                self.status = format!("Scanning {}...", path.display());
                tokio::spawn(async move {
                    let result = tokio::task::spawn_blocking(move || library::scan_dirs(&dirs))
                        .await
                        .map_err(|error| anyhow!(error))
                        .and_then(|result| result);
                    let _ = tx.send(AppEvent::Scan { result });
                });
            }
        }
    }

    pub fn switch_backend(&mut self) -> Result<()> {
        crate::cassette_log!(
            "INFO",
            "backend switch start from={:?}",
            self.config.backend
        );
        self.config.backend = match self.config.backend {
            BackendKind::Invidious => BackendKind::Piped,
            BackendKind::Piped => BackendKind::Invidious,
        };
        self.config.save()?;
        self.client = MusicClient::new(self.config.clone())?;
        self.search_results.clear();
        self.selected = 0;
        self.art_key = None;
        self.art_protocol = None;
        self.status = format!("Backend: {}", self.config.backend);
        crate::cassette_log!("INFO", "backend switch ok to={:?}", self.config.backend);
        Ok(())
    }

    pub fn cycle_theme(&mut self) {
        self.config.theme = self.config.theme.next();
        if let Err(error) = self.config.save() {
            self.status = format!("Theme save failed: {error}");
            crate::cassette_log!("ERROR", "theme save failed error={error}");
            return;
        }
        self.status = format!("Theme: {}", self.config.theme);
        crate::cassette_log!("INFO", "theme changed to={}", self.config.theme);
    }

    fn current_video_id(&self) -> Option<String> {
        if self.video_source.is_none() {
            return None;
        }
        self.playing
            .as_ref()
            .filter(|track| !track.id.is_empty())
            .map(|track| track.id.clone())
    }

    fn video_duration(&self, id: &str) -> f64 {
        if self.progress.1 > 0.0 {
            return f64::from(self.progress.1);
        }
        let track_duration = self
            .playing
            .as_ref()
            .filter(|track| track.id == id)
            .map(|track| track.duration.as_secs_f64())
            .unwrap_or(0.0);
        if track_duration > 0.0 {
            return track_duration;
        }
        self.video_progress_state
            .videos
            .get(id)
            .map(|entry| entry.duration)
            .unwrap_or(0.0)
    }

    fn resume_position(&self, track: &Track) -> f64 {
        let Some(entry) = self.video_progress_state.videos.get(&track.id) else {
            return 0.0;
        };
        let metadata_duration = track.duration.as_secs_f64();
        let duration = if metadata_duration > 0.0 {
            metadata_duration
        } else {
            entry.duration
        };
        if entry.position < 5.0 || (duration > 0.0 && entry.position >= duration - 10.0) {
            return 0.0;
        }
        entry.position
    }

    fn flush_video_progress(&mut self) {
        match save_video_progress(&self.video_progress_state) {
            Ok(()) => {
                self.video_progress_saved_at = Some(Instant::now());
                self.video_progress_saved_position = f64::from(self.progress.0);
            }
            Err(error) => crate::cassette_log!("ERROR", "video progress save failed error={error}"),
        }
    }

    fn remember_video_position(&mut self, id: &str, position: f64, duration: f64) {
        if !position.is_finite() || position <= 0.0 {
            return;
        }
        let should_store = position >= 5.0 && !(duration > 0.0 && position >= duration - 10.0);
        if should_store {
            self.video_progress_state
                .videos
                .insert(id.to_string(), VideoProgressEntry { position, duration });
        } else if self.video_progress_state.videos.remove(id).is_none() {
            return;
        }

        let due = self
            .video_progress_saved_at
            .is_none_or(|saved_at| saved_at.elapsed() >= Duration::from_secs(5))
            || (position - self.video_progress_saved_position).abs() >= 10.0;
        if due {
            self.flush_video_progress();
        }
    }

    fn save_current_video_position(&mut self) {
        let Some(id) = self.current_video_id() else {
            return;
        };
        let position = f64::from(self.progress.0);
        let duration = self.video_duration(&id);
        self.remember_video_position(&id, position, duration);
        self.flush_video_progress();
        crate::cassette_log!(
            "INFO",
            "video progress saved id={id} position={:.1} duration={:.1}",
            position,
            duration
        );
    }

    fn clear_video_position(&mut self, id: &str) {
        if self.video_progress_state.videos.remove(id).is_some() {
            self.flush_video_progress();
            crate::cassette_log!("INFO", "video progress cleared id={id}");
        }
    }

    pub async fn play_selected(&mut self) {
        let Some(track) = self.selected_track().cloned() else {
            crate::cassette_log!("WARN", "play requested with no selected track");
            return;
        };
        self.save_current_video_position();
        if self.video_source.is_some() {
            self.video.stop();
        }
        self.video_source = None;
        self.video_audio_label = None;
        crate::cassette_log!(
            "INFO",
            "play start id={} title={:?} artist={:?} source={:?} local={}",
            track.id,
            track.title,
            track.artist,
            track.source,
            track.path.is_some()
        );
        self.status = format!("Loading {}", track.title);

        let playable = if track.path.is_some() {
            crate::cassette_log!("INFO", "play source is local path={:?}", track.path);
            track
        } else {
            crate::cassette_log!(
                "INFO",
                "stream resolve start id={} active={}",
                track.id,
                self.client.instance_label()
            );
            match self.client.resolve(&track).await {
                Ok(playable) => {
                    crate::cassette_log!(
                        "INFO",
                        "stream resolve ok id={} has_url={}",
                        playable.id,
                        playable.stream_url.is_some()
                    );
                    playable
                }
                Err(error) => {
                    crate::cassette_log!(
                        "ERROR",
                        "stream resolve failed id={} error={error}",
                        track.id
                    );
                    self.status = format!("Could not resolve stream: {error}");
                    return;
                }
            }
        };

        self.selected_audio_track = 0;
        let source = if let Some(path) = playable.path.clone() {
            let source = local_play_source(&playable, &path);
            crate::cassette_log!("INFO", "play source selected path={:?}", source);
            source
        } else if let Some(url) = playable.stream_url.clone() {
            crate::cassette_log!(
                "INFO",
                "play source selected url_host={:?} url_len={}",
                crate::api::url_host(&url),
                url.len()
            );
            PlaySource::Url {
                url,
                cache_key: audio_cache_key(&playable, self.selected_audio_track),
            }
        } else {
            crate::cassette_log!("ERROR", "play failed because there was no playable source");
            self.status = "No playable source for this track".into();
            return;
        };

        let duration = playable.duration.as_secs_f32();
        crate::cassette_log!(
            "INFO",
            "player command play sent id={} duration={}",
            playable.id,
            duration
        );
        self.player.play(playable.clone(), source);
        self.playing = Some(playable);
        self.progress = (0.0, duration);
        self.paused = false;
    }

    pub async fn play_video_selected(&mut self) {
        let Some(track) = self.selected_track().cloned() else {
            crate::cassette_log!("WARN", "video play requested with no selected track");
            return;
        };
        self.save_current_video_position();
        let resume = self.resume_position(&track);
        if self.video_source.is_some() {
            self.video.stop();
        }
        self.video_source = None;
        self.video_audio_label = Some("Portuguese (dub)".into());
        self.status = if resume > 0.0 {
            format!("Resuming {} at {:.0}s", track.title, resume)
        } else {
            format!("Loading video {}", track.title)
        };
        crate::cassette_log!(
            "INFO",
            "video play start id={} title={:?} source={:?}",
            track.id,
            track.title,
            track.source
        );

        let video = match self.client.resolve_video(&track).await {
            Ok(video) => video,
            Err(error) => {
                crate::cassette_log!(
                    "ERROR",
                    "video resolve failed id={} error={error}",
                    track.id
                );
                self.status = format!("Could not resolve video: {error}");
                return;
            }
        };

        let url = if video.video_url.is_empty() {
            video.hls_url.clone().unwrap_or_default()
        } else {
            video.video_url.clone()
        };
        if url.is_empty() {
            self.status = "Video has no playable URL".into();
            return;
        }
        let mut playable = track.clone();
        playable.audio_tracks = video.audio_tracks.clone();
        self.selected_audio_track = 0;
        self.player.stop();
        let audio_tracks = video
            .audio_tracks
            .iter()
            .map(|audio| (audio.label.clone(), audio.url.clone()))
            .collect();
        self.video.play(
            url,
            audio_tracks,
            track.title.clone(),
            self.config.volume,
            resume as f32,
            "pt",
        );
        crate::cassette_log!(
            "INFO",
            "mpv video requested id={} direct={} quality={:?} audio_tracks={} resume={:.1}",
            track.id,
            !video.video_url.is_empty(),
            video.quality,
            video.audio_tracks.len(),
            resume
        );
        self.video_source = Some(video);
        self.playing = Some(playable);
        self.progress = (resume as f32, track.duration.as_secs_f32());
        self.paused = false;
        self.status = format!("Opening mpv for {}", track.title);
    }

    pub async fn play_next(&mut self) {
        self.move_selection(1);
        self.play_selected().await;
    }

    pub async fn play_next_video(&mut self) {
        self.move_selection(1);
        self.play_video_selected().await;
    }

    pub async fn play_previous(&mut self) {
        self.move_selection(-1);
        self.play_selected().await;
    }

    pub async fn play_previous_video(&mut self) {
        self.move_selection(-1);
        self.play_video_selected().await;
    }

    pub fn stop_video(&mut self) {
        self.save_current_video_position();
        if self.video_source.is_some() {
            self.video.stop();
        }
        self.video_source = None;
        self.video_audio_label = None;
    }

    pub fn toggle_pause(&mut self) {
        if self.playing.is_none() {
            return;
        }
        if self.paused {
            if self.video_source.is_some() {
                self.video.set_paused(self.video.generation(), false);
            } else {
                self.player.resume();
            }
            self.paused = false;
            self.status = "Resumed".into();
        } else {
            if self.video_source.is_some() {
                self.video.set_paused(self.video.generation(), true);
            } else {
                self.player.pause();
            }
            self.paused = true;
            self.status = "Paused".into();
        }
    }

    pub fn adjust_volume(&mut self, delta: f32) {
        self.config.volume = (self.config.volume + delta).clamp(0.0, 1.0);
        if self.video_source.is_some() {
            self.video
                .set_volume(self.video.generation(), self.config.volume);
        } else {
            self.player.set_volume(self.config.volume);
        }
        let _ = self.config.save();
        self.status = format!("Volume: {:.0}%", self.config.volume * 100.0);
    }

    pub fn cycle_audio_track(&mut self, delta: isize) {
        if self.video_source.is_some() {
            self.video
                .cycle_audio(self.video.generation(), delta as i32);
            self.status = "Audio: switching in mpv".into();
            return;
        }
        let Some(playable) = self.playing.clone() else {
            self.status = "Nothing playing".into();
            return;
        };
        if playable.audio_tracks.is_empty() {
            self.status = "No alternate audio tracks".into();
            return;
        }
        let count = playable.audio_tracks.len();
        let index = advance_index(self.selected_audio_track, count, delta);
        self.select_audio_track(index);
    }

    pub fn cycle_dub(&mut self, delta: isize) {
        if self.video_source.is_some() {
            let Some(source) = self.video_source.clone() else {
                self.status = "No video playing".into();
                return;
            };
            let dubs: Vec<usize> = source
                .audio_tracks
                .iter()
                .enumerate()
                .filter(|(_, audio)| audio.kind != "original")
                .map(|(index, _)| index)
                .collect();
            if dubs.is_empty() {
                self.status = "No dubs available".into();
                return;
            }
            let current_label = self.active_audio_track_label().map(str::to_string);
            let current = current_label
                .as_deref()
                .and_then(|label| {
                    dubs.iter()
                        .position(|&index| source.audio_tracks[index].label == label)
                })
                .unwrap_or(0);
            let target = dubs[advance_index(current, dubs.len(), delta)];
            let aid = i64::try_from(target + 1).unwrap_or(1);
            let label = source.audio_tracks[target].label.clone();
            self.video.select_audio(self.video.generation(), aid);
            self.video_audio_label = Some(label.clone());
            self.status = format!("Audio: {label}");
            crate::cassette_log!(
                "INFO",
                "dub switch index={} count={} label={:?} aid={}",
                target,
                source.audio_tracks.len(),
                label,
                aid
            );
            return;
        }
        let Some(playable) = self.playing.clone() else {
            self.status = "Nothing playing".into();
            return;
        };
        let dubs: Vec<usize> = playable
            .audio_tracks
            .iter()
            .enumerate()
            .filter(|(_, audio)| audio.kind != "original")
            .map(|(index, _)| index)
            .collect();
        if dubs.is_empty() {
            self.status = "No dubs available".into();
            return;
        }
        let current = dubs
            .iter()
            .position(|&index| index == self.selected_audio_track)
            .unwrap_or(0);
        let target = dubs[advance_index(current, dubs.len(), delta)];
        crate::cassette_log!(
            "INFO",
            "dub switch index={} count={} label={:?}",
            target,
            dubs.len(),
            playable.audio_tracks[target].label
        );
        self.select_audio_track(target);
    }

    fn select_audio_track(&mut self, index: usize) {
        let Some(mut playable) = self.playing.clone() else {
            self.status = "Nothing playing".into();
            return;
        };
        let Some(audio) = playable.audio_tracks.get(index).cloned() else {
            self.status = "No alternate audio tracks".into();
            return;
        };
        self.selected_audio_track = index;
        playable.stream_url = Some(audio.url.clone());
        let position = self.progress.0;
        let duration = self.progress.1;
        let fraction = if duration > 0.0 {
            (position / duration).clamp(0.0, 1.0)
        } else {
            0.0
        };
        crate::cassette_log!(
            "INFO",
            "audio track switch index={} count={} label={:?} language={} kind={} resume={}",
            index,
            playable.audio_tracks.len(),
            audio.label,
            audio.language,
            audio.kind,
            position
        );
        self.player.play(
            playable.clone(),
            PlaySource::Url {
                url: audio.url.clone(),
                cache_key: audio_cache_key(&playable, index),
            },
        );
        if fraction > 0.0 {
            self.player.seek(fraction as f64);
        }
        self.playing = Some(playable);
        self.paused = false;
        self.status = format!("Audio: {}", audio.label);
    }

    pub fn refresh_art(&mut self) {
        let Some(track) = self.selected_track().cloned() else {
            crate::cassette_log!("DEBUG", "art refresh skipped because no track is selected");
            self.art_protocol = None;
            self.art_key = None;
            return;
        };
        let key = track.thumbnail_url.clone().or_else(|| {
            track
                .thumbnail_path
                .as_ref()
                .map(|path| path.to_string_lossy().to_string())
        });
        if key == self.art_key {
            return;
        }
        crate::cassette_log!(
            "DEBUG",
            "art refresh requested id={} title={:?} key={:?}",
            track.id,
            track.title,
            key
        );
        self.art_key = key.clone();
        self.art_protocol = None;
        let Some(_) = key else {
            return;
        };

        self.art_id += 1;
        let id = self.art_id;
        let loader = self.art.clone();
        let tx = self.events_tx.clone();
        tokio::spawn(async move {
            let image = loader.load(&track).await;
            let _ = tx.send(AppEvent::Art { id, image });
        });
    }

    pub fn try_recv_event(&mut self) -> Option<AppEvent> {
        self.events_rx.try_recv().ok()
    }

    pub fn handle_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Art { id, image } => {
                if id != self.art_id {
                    crate::cassette_log!(
                        "DEBUG",
                        "art result ignored stale id={} current={}",
                        id,
                        self.art_id
                    );
                    return;
                }
                crate::cassette_log!("DEBUG", "art result id={} loaded={}", id, image.is_some());
                self.art_protocol = None;
                if let Some(image) = image {
                    self.art_protocol = self.art.protocol(image, Size::new(48, 18));
                    crate::cassette_log!(
                        "DEBUG",
                        "art protocol created id={} protocol={}",
                        id,
                        self.protocol_label
                    );
                }
            }
            AppEvent::Search { generation, result } => {
                if generation != self.search_generation {
                    crate::cassette_log!(
                        "DEBUG",
                        "search result ignored stale generation={} current={}",
                        generation,
                        self.search_generation
                    );
                    return;
                }
                match result {
                    Ok(tracks) => {
                        crate::cassette_log!(
                            "INFO",
                            "search completed generation={} results={}",
                            generation,
                            tracks.len()
                        );
                        self.status = format!("{} results", tracks.len());
                        self.search_results = tracks;
                        self.selected = 0;
                        self.art_key = None;
                        self.refresh_art();
                    }
                    Err(error) => {
                        crate::cassette_log!(
                            "ERROR",
                            "search failed generation={} error={error}",
                            generation
                        );
                        self.status = format!("Search failed: {error}");
                    }
                }
            }
            AppEvent::Scan { result } => match result {
                Ok(tracks) => {
                    crate::cassette_log!("INFO", "local scan completed tracks={}", tracks.len());
                    self.status = format!("{} local tracks", tracks.len());
                    self.local_tracks = tracks;
                    self.selected = 0;
                    self.art_key = None;
                    self.refresh_art();
                }
                Err(error) => {
                    crate::cassette_log!("ERROR", "local scan failed error={error}");
                    self.status = format!("Scan failed: {error}");
                }
            },
        }
    }

    pub fn take_video_auto_next(&mut self) -> Option<u64> {
        self.video_auto_next.take()
    }

    pub fn handle_video_event(&mut self, event: VideoEvent) {
        match event {
            VideoEvent::Started { generation } => {
                crate::cassette_log!("INFO", "mpv started generation={}", generation);
            }
            VideoEvent::Progress {
                generation,
                position,
                duration,
            } => {
                if generation != self.video.generation() {
                    return;
                }
                crate::cassette_log!(
                    "DEBUG",
                    "mpv progress generation={} position={:.1} duration={:.1}",
                    generation,
                    position,
                    duration
                );
                self.progress = (position as f32, duration as f32);
                if let Some(id) = self.current_video_id() {
                    self.remember_video_position(&id, position, duration);
                }
            }
            VideoEvent::Paused { generation, paused } => {
                if generation == self.video.generation() {
                    self.paused = paused;
                    self.status = if paused {
                        "Paused".into()
                    } else {
                        "Playing".into()
                    };
                }
            }
            VideoEvent::AudioChanged { generation, label } => {
                if generation == self.video.generation() {
                    crate::cassette_log!("INFO", "mpv audio track label={:?}", label);
                    self.video_audio_label = Some(label.clone());
                    self.status = format!("Audio: {label}");
                }
            }
            VideoEvent::Stopped => {
                crate::cassette_log!("INFO", "mpv stopped");
            }
            VideoEvent::Finished { generation } => {
                if generation == self.video.generation() {
                    let finished_id = self.current_video_id();
                    let in_playlist = self.tab == Tab::Playlists && self.active_playlist.is_some();
                    let has_next = in_playlist && self.selected + 1 < self.visible_tracks().len();
                    crate::cassette_log!(
                        "INFO",
                        "mpv finished generation={} in_playlist={} has_next={}",
                        generation,
                        in_playlist,
                        has_next
                    );
                    if let Some(id) = finished_id {
                        self.clear_video_position(&id);
                    }
                    self.video.stop();
                    self.video_source = None;
                    self.video_audio_label = None;
                    self.video_auto_next = has_next.then_some(generation);
                    self.progress = (0.0, 0.0);
                    self.paused = false;
                    self.status = if has_next {
                        "Loading next video...".into()
                    } else if in_playlist {
                        "Playlist finished".into()
                    } else {
                        "Video finished".into()
                    };
                }
            }
            VideoEvent::Error(message) => {
                crate::cassette_log!("ERROR", "mpv error message={message}");
                self.status = format!("Video: {message}");
                self.video_source = None;
                self.video_audio_label = None;
            }
        }
    }

    pub fn handle_player_event(&mut self, event: PlayerEvent) {
        match event {
            PlayerEvent::Loading(track) => {
                crate::cassette_log!(
                    "INFO",
                    "player event loading id={} title={:?} source={:?}",
                    track.id,
                    track.title,
                    track.source
                );
                self.status = format!("Loading {}", track.title);
            }
            PlayerEvent::Playing {
                track,
                duration,
                cached,
            } => {
                crate::cassette_log!(
                    "INFO",
                    "player event playing id={} title={:?} duration={} cached={}",
                    track.id,
                    track.title,
                    duration,
                    cached
                );
                self.status = format!(
                    "Playing {}{}",
                    track.title,
                    if cached { " (cached)" } else { "" }
                );
                self.progress = (0.0, duration);
            }
            PlayerEvent::Paused => {
                crate::cassette_log!("INFO", "player event paused");
                self.paused = true;
            }
            PlayerEvent::Resumed => {
                crate::cassette_log!("INFO", "player event resumed");
                self.paused = false;
            }
            PlayerEvent::Progress { position, duration } => {
                crate::cassette_log!(
                    "TRACE",
                    "player event progress position={} duration={}",
                    position,
                    duration
                );
                self.progress = (position, duration);
            }
            PlayerEvent::Stopped => {
                crate::cassette_log!("INFO", "player event stopped");
                if self.video_source.is_none() {
                    self.progress = (0.0, 0.0);
                }
            }
            PlayerEvent::Finished => {
                crate::cassette_log!("INFO", "player event finished");
                if self.video_source.is_none() {
                    self.status = "Finished".into();
                }
            }
            PlayerEvent::Error(message) => {
                crate::cassette_log!("ERROR", "player event error message={message}");
                self.status = format!("Audio: {message}");
            }
        }
    }

    pub fn quit(&mut self) {
        crate::cassette_log!("INFO", "quit requested");
        self.save_current_video_position();
        self.should_quit = true;
        self.video.shutdown();
        self.player.shutdown();
        let _ = self.config.save();
    }
}

pub fn local_dir_label(dirs: &[PathBuf]) -> String {
    dirs.first()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|| "none".into())
}

fn audio_cache_key(track: &Track, index: usize) -> String {
    track
        .audio_tracks
        .get(index)
        .map(|audio| format!("{}:{}", track.id, audio.id))
        .unwrap_or_else(|| track.id.clone())
}

fn advance_index(current: usize, count: usize, delta: isize) -> usize {
    if count == 0 {
        return 0;
    }
    if delta >= 0 {
        (current + delta as usize) % count
    } else {
        (current + count - delta.unsigned_abs() % count) % count
    }
}

fn local_play_source(track: &crate::model::Track, path: &Path) -> PlaySource {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(
        extension.as_str(),
        "mp4" | "m4a" | "mov" | "webm" | "mkv" | "avi"
    ) {
        match extract_local_audio(track, path) {
            Ok(source) => source,
            Err(error) => {
                crate::cassette_log!(
                    "WARN",
                    "local audio extraction failed id={} error={error} path={:?}",
                    track.id,
                    path
                );
                PlaySource::Path(path.to_path_buf())
            }
        }
    } else {
        PlaySource::Path(path.to_path_buf())
    }
}

fn extract_local_audio(track: &crate::model::Track, path: &Path) -> Result<PlaySource> {
    let destination = video_audio_cache_path(&track.id);
    if fs::metadata(&destination).is_ok_and(|metadata| metadata.len() > 0) {
        crate::cassette_log!(
            "INFO",
            "video audio cache hit id={} path={:?}",
            track.id,
            destination
        );
        return Ok(PlaySource::Path(destination));
    }
    ensure_parent(&destination).context("could not create video audio cache directory")?;
    crate::cassette_log!(
        "INFO",
        "video audio extraction start id={} path={:?} destination={:?}",
        track.id,
        path,
        destination
    );
    let output = ProcessCommand::new("ffmpeg")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-nostdin")
        .arg("-y")
        .arg("-i")
        .arg(path)
        .arg("-map")
        .arg("0:a:0")
        .arg("-vn")
        .arg("-c:a")
        .arg("libvorbis")
        .arg("-q:a")
        .arg("4")
        .arg("-f")
        .arg("ogg")
        .arg(&destination)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .context("could not run ffmpeg audio extraction")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("ffmpeg audio extraction failed: {}", stderr.trim()));
    }
    let metadata = fs::metadata(&destination).context("audio extraction produced no file")?;
    crate::cassette_log!(
        "INFO",
        "video audio extraction complete id={} bytes={} path={:?}",
        track.id,
        metadata.len(),
        destination
    );
    Ok(PlaySource::Path(destination))
}
