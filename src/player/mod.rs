use std::{
    fs,
    io::{self, Cursor, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::Duration,
};

use cpal::traits::{DeviceTrait, HostTrait};
use reqwest::header::{CONTENT_RANGE, RANGE};
use rodio::Source;

use crate::{api::url_host, config, model::Track};

#[derive(Clone, Debug)]
pub enum PlaySource {
    Url { url: String, cache_key: String },
    Path(PathBuf),
}

enum Command {
    Play { track: Track, source: PlaySource },
    Pause,
    Resume,
    Stop,
    Seek(f64),
    SetVolume(f32),
    Shutdown,
}

#[derive(Clone, Debug)]
pub enum PlayerEvent {
    Loading(Track),
    Playing {
        track: Track,
        duration: f32,
        cached: bool,
    },
    Paused,
    Resumed,
    Progress {
        position: f32,
        duration: f32,
    },
    Stopped,
    Finished,
    Error(String),
}

pub struct PlayerHandle {
    commands: Sender<Command>,
    events: Receiver<PlayerEvent>,
    thread: Option<thread::JoinHandle<()>>,
}

impl PlayerHandle {
    pub fn spawn(initial_volume: f32) -> Self {
        crate::cassette_log!("INFO", "player spawn start volume={}", initial_volume);
        let (commands, command_rx) = mpsc::channel();
        let (event_tx, events) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("cassette-audio".into())
            .spawn(move || run(command_rx, event_tx, initial_volume))
            .expect("starting audio thread");
        Self {
            commands,
            events,
            thread: Some(thread),
        }
    }

    pub fn play(&self, track: Track, source: PlaySource) {
        let _ = self.commands.send(Command::Play { track, source });
    }

    pub fn pause(&self) {
        let _ = self.commands.send(Command::Pause);
    }

    pub fn resume(&self) {
        let _ = self.commands.send(Command::Resume);
    }

    pub fn stop(&self) {
        let _ = self.commands.send(Command::Stop);
    }

    pub fn seek(&self, fraction: f64) {
        let _ = self.commands.send(Command::Seek(fraction.clamp(0.0, 1.0)));
    }

    pub fn set_volume(&self, volume: f32) {
        let _ = self
            .commands
            .send(Command::SetVolume(volume.clamp(0.0, 1.0)));
    }

    pub fn shutdown(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
            crate::cassette_log!("INFO", "player thread joined");
        }
    }

    pub fn try_recv_event(&self) -> Option<PlayerEvent> {
        self.events.try_recv().ok()
    }
}

fn run(command_rx: Receiver<Command>, event_tx: Sender<PlayerEvent>, initial_volume: f32) {
    let host = cpal::default_host();
    let host_id = format!("{:?}", host.id());
    let default_device = host
        .default_output_device()
        .and_then(|device| device.description().ok())
        .map(|description| description.name().to_string())
        .unwrap_or_else(|| "<none>".into());
    crate::cassette_log!(
        "INFO",
        "audio backend selected host={} default_device={:?}",
        host_id,
        default_device
    );

    let downloader = match reqwest::blocking::Client::builder()
        .user_agent(concat!("cassette/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(120))
        .build()
    {
        Ok(client) => {
            crate::cassette_log!("INFO", "player http client ready timeout=120s");
            client
        }
        Err(error) => {
            crate::cassette_log!("ERROR", "player http client failed error={error}");
            let _ = event_tx.send(PlayerEvent::Error(format!("HTTP client: {error}")));
            return;
        }
    };

    let mut audio: Option<(rodio::MixerDeviceSink, rodio::Player)> = None;
    let mut volume = initial_volume.clamp(0.0, 1.0);
    crate::cassette_log!(
        "INFO",
        "player run start initial_volume={} clamped_volume={}",
        initial_volume,
        volume
    );
    let mut current: Option<(Track, f32)> = None;
    let mut paused = false;

    loop {
        while let Ok(command) = command_rx.try_recv() {
            match command {
                Command::Play { track, source } => {
                    crate::cassette_log!(
                        "INFO",
                        "player command play received id={} title={:?} source={:?}",
                        track.id,
                        track.title,
                        source_kind(&source)
                    );
                    if audio.is_none() {
                        crate::cassette_log!("INFO", "audio sink open start");
                        let sink = match rodio::DeviceSinkBuilder::open_default_sink() {
                            Ok(sink) => {
                                crate::cassette_log!("INFO", "audio sink open ok");
                                sink
                            }
                            Err(error) => {
                                crate::cassette_log!(
                                    "ERROR",
                                    "audio sink open failed error={error}"
                                );
                                let _ = event_tx
                                    .send(PlayerEvent::Error(format!("audio output: {error}")));
                                continue;
                            }
                        };
                        let player = rodio::Player::connect_new(sink.mixer());
                        player.set_volume(volume);
                        audio = Some((sink, player));
                        crate::cassette_log!("INFO", "audio player connected volume={}", volume);
                    }
                    let Some((_, player)) = audio.as_mut() else {
                        crate::cassette_log!("ERROR", "audio player unavailable after init");
                        continue;
                    };

                    let _ = event_tx.send(PlayerEvent::Loading(track.clone()));
                    player.stop();
                    paused = false;

                    let (decoded, cached) = match open_source(&downloader, source, &track.id) {
                        Ok(result) => result,
                        Err(error) => {
                            crate::cassette_log!(
                                "ERROR",
                                "track load failed id={} error={error}",
                                track.id
                            );
                            current = None;
                            let _ = event_tx
                                .send(PlayerEvent::Error(format!("could not load track: {error}")));
                            continue;
                        }
                    };
                    crate::cassette_log!(
                        "INFO",
                        "track decode ok id={} cached={}",
                        track.id,
                        cached
                    );

                    let duration = decoded
                        .total_duration()
                        .map(|duration| duration.as_secs_f32())
                        .unwrap_or(track.duration.as_secs_f32());
                    player.append(decoded);
                    player.set_volume(volume);
                    player.play();
                    current = Some((track.clone(), duration));
                    crate::cassette_log!(
                        "INFO",
                        "playback started id={} duration={} volume={} cached={}",
                        track.id,
                        duration,
                        volume,
                        cached
                    );
                    let _ = event_tx.send(PlayerEvent::Playing {
                        track,
                        duration,
                        cached,
                    });
                }
                Command::Pause => {
                    crate::cassette_log!("INFO", "player command pause received");
                    if let Some((_, player)) = &mut audio
                        && current.is_some()
                        && !paused
                    {
                        player.pause();
                        paused = true;
                        let _ = event_tx.send(PlayerEvent::Paused);
                    }
                }
                Command::Resume => {
                    crate::cassette_log!("INFO", "player command resume received");
                    if let Some((_, player)) = &mut audio
                        && current.is_some()
                        && paused
                    {
                        player.play();
                        paused = false;
                        let _ = event_tx.send(PlayerEvent::Resumed);
                    }
                }
                Command::Stop => {
                    crate::cassette_log!("INFO", "player command stop received");
                    if let Some((_, player)) = &mut audio {
                        player.stop();
                    }
                    current = None;
                    paused = false;
                    let _ = event_tx.send(PlayerEvent::Stopped);
                }
                Command::Seek(fraction) => {
                    crate::cassette_log!(
                        "INFO",
                        "player command seek received fraction={}",
                        fraction
                    );
                    if let (Some((_, player)), Some((_, duration))) = (audio.as_mut(), &current) {
                        let target = Duration::from_secs_f64(*duration as f64 * fraction);
                        if let Err(error) = player.try_seek(target) {
                            crate::cassette_log!(
                                "ERROR",
                                "seek failed target={:?} error={error}",
                                target
                            );
                            let _ = event_tx
                                .send(PlayerEvent::Error(format!("could not seek: {error}")));
                        } else {
                            crate::cassette_log!("INFO", "seek ok target={:?}", target);
                        }
                    }
                }
                Command::SetVolume(value) => {
                    crate::cassette_log!(
                        "INFO",
                        "player command set_volume received value={}",
                        value
                    );
                    volume = value.clamp(0.0, 1.0);
                    if let Some((_, player)) = &mut audio {
                        player.set_volume(volume);
                    }
                }
                Command::Shutdown => {
                    crate::cassette_log!("INFO", "player command shutdown received");
                    if let Some((_, player)) = &mut audio {
                        player.stop();
                    }
                    return;
                }
            }
        }

        if let Some((_, player)) = &mut audio
            && let Some((_, duration)) = &current
            && !paused
        {
            if player.empty() {
                crate::cassette_log!("INFO", "playback finished");
                current = None;
                paused = false;
                let _ = event_tx.send(PlayerEvent::Finished);
            } else {
                let _ = event_tx.send(PlayerEvent::Progress {
                    position: player.get_pos().as_secs_f32(),
                    duration: *duration,
                });
            }
        }

        thread::sleep(Duration::from_millis(100));
    }
}

trait AudioRead: Read + Seek + Send + Sync {}
impl<T: Read + Seek + Send + Sync> AudioRead for T {}

type AudioDecoder = rodio::Decoder<Box<dyn AudioRead>>;

const RANGE_CHUNK: u64 = 1024 * 1024;

fn cache_ready(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.len() > 0)
}

fn open_source(
    _client: &reqwest::blocking::Client,
    source: PlaySource,
    id: &str,
) -> Result<(AudioDecoder, bool), String> {
    match source {
        PlaySource::Url { url, cache_key } => {
            let cache_path = config::audio_cache_path(&cache_key);
            let mut cached = cache_ready(&cache_path);
            let mut retried = false;
            loop {
                if cached {
                    crate::cassette_log!(
                        "INFO",
                        "audio cache hit id={} cache_key={} path={:?}",
                        id,
                        cache_key,
                        cache_path
                    );
                } else {
                    let temp_path = partial_cache_path(&cache_path);
                    crate::cassette_log!(
                        "INFO",
                        "audio download start id={} cache_key={} path={:?} url_host={:?}",
                        id,
                        cache_key,
                        cache_path,
                        url_host(&url)
                    );
                    if let Err(error) = download_to_cache(&url, &temp_path, &cache_path) {
                        crate::cassette_log!(
                            "ERROR",
                            "audio download failed id={} error={error}",
                            id
                        );
                        return Err(error);
                    }
                }

                let file = match fs::File::open(&cache_path) {
                    Ok(file) => file,
                    Err(error) => {
                        crate::cassette_log!(
                            "ERROR",
                            "audio cache open failed id={} path={:?} error={error}",
                            id,
                            cache_path
                        );
                        return Err(error.to_string());
                    }
                };
                match rodio::Decoder::new(Box::new(file) as Box<dyn AudioRead>) {
                    Ok(decoder) => return Ok((decoder, cached)),
                    Err(error) => {
                        crate::cassette_log!(
                            "ERROR",
                            "audio cache decode failed id={} error={error}",
                            id
                        );
                        if cached && !retried {
                            crate::cassette_log!(
                                "WARN",
                                "audio cache invalid id={} path={:?} removing and retrying",
                                id,
                                cache_path
                            );
                            let _ = fs::remove_file(&cache_path);
                            let _ = fs::remove_file(partial_cache_path(&cache_path));
                            cached = false;
                            retried = true;
                            continue;
                        }
                        return Err(error.to_string());
                    }
                }
            }
        }
        PlaySource::Path(path) => {
            crate::cassette_log!("INFO", "local read start id={} path={:?}", id, path);
            let bytes = fs::read(&path).map_err(|error| {
                crate::cassette_log!("ERROR", "local read failed path={:?} error={error}", path);
                error.to_string()
            })?;
            crate::cassette_log!(
                "INFO",
                "local read complete bytes={} path={:?}",
                bytes.len(),
                path
            );
            let decoder = rodio::Decoder::new(Box::new(Cursor::new(bytes)) as Box<dyn AudioRead>)
                .map_err(|error| {
                crate::cassette_log!("ERROR", "local decode failed id={} error={error}", id);
                error.to_string()
            })?;
            Ok((decoder, false))
        }
    }
}

fn download_to_cache(url: &str, temp_path: &Path, final_path: &Path) -> Result<u64, String> {
    crate::cassette_log!(
        "INFO",
        "audio cache download start path={:?} url_host={:?}",
        final_path,
        url_host(url)
    );
    let result = (|| -> io::Result<u64> {
        if let Some(parent) = final_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let client = reqwest::blocking::Client::builder()
            .user_agent(concat!("cassette-cache/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(io::Error::other)?;
        let probe = client
            .get(url)
            .header(RANGE, "bytes=0-0")
            .send()
            .map_err(io::Error::other)?
            .error_for_status()
            .map_err(io::Error::other)?;
        let content_range = probe
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
            .ok_or_else(|| io::Error::other("server did not report content length"))?;
        let total = content_range_total(&content_range)
            .ok_or_else(|| io::Error::other("invalid content range"))?;
        let mut file = fs::File::create(temp_path)?;
        file.set_len(total)?;
        let mut downloaded = 0u64;
        let mut start = 0u64;
        while start < total {
            let end = (start + RANGE_CHUNK - 1).min(total - 1);
            let bytes = fetch_range(&client, url, start, end)?;
            file.seek(SeekFrom::Start(start))?;
            file.write_all(&bytes)?;
            downloaded += bytes.len() as u64;
            crate::cassette_log!(
                "DEBUG",
                "audio cache download progress bytes={} total={} path={:?}",
                downloaded,
                total,
                final_path
            );
            start = end + 1;
        }
        file.flush()?;
        Ok(downloaded)
    })();
    match result {
        Ok(bytes) => {
            if let Err(error) = fs::rename(temp_path, final_path) {
                let message = format!("audio cache rename: {error}");
                crate::cassette_log!(
                    "WARN",
                    "audio cache rename failed path={:?} error={error}",
                    final_path
                );
                let _ = fs::remove_file(temp_path);
                return Err(message);
            }
            crate::cassette_log!(
                "INFO",
                "audio cache saved bytes={} path={:?}",
                bytes,
                final_path
            );
            Ok(bytes)
        }
        Err(error) => {
            crate::cassette_log!(
                "WARN",
                "audio cache download failed path={:?} error={error}",
                final_path
            );
            let _ = fs::remove_file(temp_path);
            Err(error.to_string())
        }
    }
}

fn fetch_range(
    client: &reqwest::blocking::Client,
    url: &str,
    start: u64,
    end: u64,
) -> io::Result<Vec<u8>> {
    let expected = (end - start + 1) as usize;
    let mut last_error = "range download failed".to_string();
    for attempt in 1..=3 {
        let range = format!("bytes={start}-{end}");
        let result = (|| -> io::Result<Vec<u8>> {
            let response = client
                .get(url)
                .header(RANGE, &range)
                .send()
                .map_err(io::Error::other)?
                .error_for_status()
                .map_err(io::Error::other)?;
            let status = response.status();
            let content_range = response
                .headers()
                .get(CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            let mut bytes = response.bytes().map_err(io::Error::other)?.to_vec();
            if status.as_u16() == 200 && start > 0 {
                let skip = start as usize;
                if bytes.len() < skip {
                    return Err(io::Error::other("server ignored range request"));
                }
                bytes.drain(..skip);
            } else if let Some(header) = content_range.as_deref()
                && let Some(actual) = content_range_start(header)
                && actual != start
            {
                return Err(io::Error::other("range start mismatch"));
            }
            if bytes.len() != expected {
                return Err(io::Error::other(format!(
                    "short range: got {} expected {}",
                    bytes.len(),
                    expected
                )));
            }
            Ok(bytes)
        })();
        match result {
            Ok(bytes) => return Ok(bytes),
            Err(error) => {
                last_error = error.to_string();
                crate::cassette_log!(
                    "WARN",
                    "audio range retry attempt={} start={} end={} error={error}",
                    attempt,
                    start,
                    end
                );
            }
        }
    }
    Err(io::Error::other(last_error))
}

fn content_range_total(header: &str) -> Option<u64> {
    header.rsplit('/').next()?.parse().ok()
}

fn content_range_start(header: &str) -> Option<u64> {
    header
        .split_whitespace()
        .nth(1)?
        .split('-')
        .next()?
        .parse()
        .ok()
}

fn partial_cache_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    path.with_file_name(name)
}

fn source_kind(source: &PlaySource) -> &'static str {
    match source {
        PlaySource::Url { .. } => "url",
        PlaySource::Path(_) => "path",
    }
}
