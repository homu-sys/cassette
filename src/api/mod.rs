use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use reqwest::Client;
use serde_json::Value;
use url::Url;

use crate::{
    config::{BackendKind, Config},
    model::{AudioTrack, Track, TrackSource, VideoSource},
};

#[derive(Clone)]
pub struct MusicClient {
    config: Config,
    http: Client,
    current: Arc<Mutex<usize>>,
}

impl MusicClient {
    pub fn new(config: Config) -> Result<Self> {
        crate::cassette_log!(
            "INFO",
            "api client created backend={:?} invidious={:?} piped={:?}",
            config.backend,
            config.invidious_instances,
            config.piped_instances
        );
        let http = Client::builder()
            .user_agent(concat!("cassette/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(20))
            .build()
            .context("could not create HTTP client")?;
        Ok(Self {
            config,
            http,
            current: Arc::new(Mutex::new(0)),
        })
    }

    pub async fn search(&self, query: &str) -> Result<Vec<Track>> {
        let source = self.config.backend;
        let instances = self.instances_for_backend(source);
        crate::cassette_log!(
            "INFO",
            "api search start backend={:?} query={:?} instances={}",
            source,
            query,
            instances.len()
        );
        let mut last_error = None;

        for (index, base) in instances.iter().enumerate() {
            crate::cassette_log!(
                "DEBUG",
                "api search attempt index={} host={:?}",
                index,
                url_host(base)
            );
            let result = match source {
                BackendKind::Invidious => self.search_invidious(base, query).await,
                BackendKind::Piped => self.search_piped(base, query).await,
            };
            match result {
                Ok(tracks) => {
                    crate::cassette_log!(
                        "INFO",
                        "api search ok index={} host={:?} results={}",
                        index,
                        url_host(base),
                        tracks.len()
                    );
                    *self
                        .current
                        .lock()
                        .unwrap_or_else(|error| error.into_inner()) = index;
                    return Ok(tracks);
                }
                Err(error) => {
                    crate::cassette_log!(
                        "WARN",
                        "api search failed index={} host={:?} error={error}",
                        index,
                        url_host(base)
                    );
                    last_error = Some(error);
                }
            }
        }

        let error = last_error.unwrap_or_else(|| anyhow!("no API instances configured"));
        crate::cassette_log!("ERROR", "api search exhausted error={error}");
        Err(error)
    }

    pub async fn resolve(&self, track: &Track) -> Result<Track> {
        if track.source == TrackSource::Local || track.stream_url.is_some() {
            crate::cassette_log!(
                "DEBUG",
                "api resolve skipped id={} source={:?} has_url={}",
                track.id,
                track.source,
                track.stream_url.is_some()
            );
            return Ok(track.clone());
        }

        let instances = self.instances_for(track.source);
        crate::cassette_log!(
            "INFO",
            "api resolve start id={} source={:?} instances={}",
            track.id,
            track.source,
            instances.len()
        );
        let mut last_error = None;

        for (index, base) in instances.iter().enumerate() {
            crate::cassette_log!(
                "DEBUG",
                "api resolve attempt index={} host={:?} id={}",
                index,
                url_host(base),
                track.id
            );
            let result = match track.source {
                TrackSource::Invidious => self.resolve_invidious(base, track).await,
                TrackSource::Piped => self.resolve_piped(base, track).await,
                TrackSource::Local => unreachable!(),
            };
            match result {
                Ok(resolved) => {
                    crate::cassette_log!(
                        "INFO",
                        "api resolve ok index={} host={:?} id={} url_host={:?}",
                        index,
                        url_host(base),
                        track.id,
                        resolved.stream_url.as_deref().and_then(url_host)
                    );
                    *self
                        .current
                        .lock()
                        .unwrap_or_else(|error| error.into_inner()) = index;
                    return Ok(resolved);
                }
                Err(error) => {
                    crate::cassette_log!(
                        "WARN",
                        "api resolve failed index={} host={:?} id={} error={error}",
                        index,
                        url_host(base),
                        track.id
                    );
                    last_error = Some(error);
                }
            }
        }

        let error = last_error.unwrap_or_else(|| anyhow!("no API instances configured"));
        crate::cassette_log!(
            "ERROR",
            "api resolve exhausted id={} error={error}",
            track.id
        );
        Err(error)
    }

    pub async fn resolve_video(&self, track: &Track) -> Result<VideoSource> {
        if let Some(path) = &track.path {
            return Ok(VideoSource {
                video_url: path.to_string_lossy().to_string(),
                hls_url: None,
                quality: None,
                audio_tracks: Vec::new(),
            });
        }

        let instances = self.instances_for(track.source);
        let mut last_error = None;
        for (index, base) in instances.iter().enumerate() {
            let result = match track.source {
                TrackSource::Invidious => self.resolve_video_invidious(base, track).await,
                TrackSource::Piped => self.resolve_video_piped(base, track).await,
                TrackSource::Local => unreachable!(),
            };
            match result {
                Ok(video) => {
                    *self
                        .current
                        .lock()
                        .unwrap_or_else(|error| error.into_inner()) = index;
                    return Ok(video);
                }
                Err(error) => last_error = Some(error),
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow!("no API instances configured")))
    }

    pub fn instance_label(&self) -> String {
        let instances = self.instances_for_backend(self.config.backend);
        let current = *self
            .current
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let host = instances
            .get(current)
            .and_then(|value| Url::parse(value).ok())
            .and_then(|url| url.host_str().map(str::to_string))
            .unwrap_or_else(|| "none".into());
        format!("{} · {}", self.config.backend, host)
    }

    fn instances_for_backend(&self, backend: BackendKind) -> Vec<String> {
        match backend {
            BackendKind::Invidious => self.config.invidious_instances.clone(),
            BackendKind::Piped => self.config.piped_instances.clone(),
        }
    }

    fn instances_for(&self, source: TrackSource) -> Vec<String> {
        match source {
            TrackSource::Invidious | TrackSource::Local => self.config.invidious_instances.clone(),
            TrackSource::Piped => self.config.piped_instances.clone(),
        }
    }

    async fn get_json(&self, url: &str, params: &[(&str, &str)]) -> Result<Value> {
        crate::cassette_log!("DEBUG", "http request GET url={} params={:?}", url, params);
        let response = self
            .http
            .get(url)
            .query(params)
            .send()
            .await
            .with_context(|| format!("request failed: {url}"))?;
        let status = response.status();
        crate::cassette_log!(
            "DEBUG",
            "http response status={} url={} host={:?}",
            status,
            url,
            url_host(url)
        );
        let response = response
            .error_for_status()
            .with_context(|| format!("HTTP error from {url}"))?;
        let value = response
            .json::<Value>()
            .await
            .context("invalid JSON response")?;
        crate::cassette_log!(
            "TRACE",
            "http json parsed bytes_hint={}",
            value.to_string().len()
        );
        Ok(value)
    }

    async fn search_invidious(&self, base: &str, query: &str) -> Result<Vec<Track>> {
        let url = format!("{}/api/v1/search", base.trim_end_matches('/'));
        let params = [("q", query), ("type", "video"), ("page", "1")];
        let value = self.get_json(&url, &params).await?;
        let items = value
            .as_array()
            .ok_or_else(|| anyhow!("Invidious search response was not an array"))?;
        crate::cassette_log!(
            "DEBUG",
            "invidious search parsed items={} host={:?}",
            items.len(),
            url_host(base)
        );

        Ok(items
            .iter()
            .filter_map(|item| {
                let id = field_string(item, &["videoId", "video_id"])?;
                Some(Track {
                    id,
                    title: field_string(item, &["title"]).unwrap_or_else(|| "Untitled".into()),
                    artist: field_string(item, &["author", "channel"]).unwrap_or_default(),
                    album: String::new(),
                    duration: Duration::from_secs(field_u64(item, &["lengthSeconds", "duration"])),
                    thumbnail_url: pick_thumbnail(item, base),
                    thumbnail_path: None,
                    stream_url: None,
                    path: None,
                    source: TrackSource::Invidious,
                    audio_tracks: Vec::new(),
                })
            })
            .collect())
    }

    async fn search_piped(&self, base: &str, query: &str) -> Result<Vec<Track>> {
        let url = format!("{}/search", base.trim_end_matches('/'));
        let params = [("q", query), ("filter", "videos")];
        let value = self.get_json(&url, &params).await?;
        let items = value
            .get("items")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("Piped search response had no items"))?;
        crate::cassette_log!(
            "DEBUG",
            "piped search parsed items={} host={:?}",
            items.len(),
            url_host(base)
        );

        Ok(items
            .iter()
            .filter_map(|item| {
                let raw_id = field_string(item, &["url", "videoId", "video_id"])?;
                Some(Track {
                    id: video_id(&raw_id),
                    title: field_string(item, &["title"]).unwrap_or_else(|| "Untitled".into()),
                    artist: field_string(item, &["uploaderName", "uploader", "channelName"])
                        .unwrap_or_default(),
                    album: String::new(),
                    duration: Duration::from_secs(field_u64(item, &["duration"])),
                    thumbnail_url: pick_thumbnail(item, base),
                    thumbnail_path: None,
                    stream_url: None,
                    path: None,
                    source: TrackSource::Piped,
                    audio_tracks: Vec::new(),
                })
            })
            .collect())
    }

    async fn resolve_invidious(&self, base: &str, track: &Track) -> Result<Track> {
        let url = format!(
            "{}/api/v1/videos/{}",
            base.trim_end_matches('/'),
            urlencoding(track.id.trim())
        );
        let value = self.get_json(&url, &[]).await?;
        let formats = value
            .get("adaptiveFormats")
            .and_then(Value::as_array)
            .or_else(|| value.get("formatStreams").and_then(Value::as_array))
            .ok_or_else(|| anyhow!("Invidious response had no formats"))?;
        crate::cassette_log!(
            "DEBUG",
            "invidious resolve formats={} id={} host={:?}",
            formats.len(),
            track.id,
            url_host(base)
        );

        let mut audio_tracks = audio_tracks(formats, base, true);
        let audio = audio_tracks
            .first()
            .map(|track| track.url.clone())
            .or_else(|| {
                formats
                    .first()
                    .and_then(|format| field_string(format, &["url"]))
                    .and_then(|raw| absolute_url(base, &raw))
            })
            .ok_or_else(|| anyhow!("Invidious response had no playable URL"))?;
        crate::cassette_log!(
            "INFO",
            "invidious audio tracks id={} count={} labels={:?}",
            track.id,
            audio_tracks.len(),
            audio_tracks
                .iter()
                .map(|audio| audio.label.as_str())
                .collect::<Vec<_>>()
        );
        crate::cassette_log!(
            "INFO",
            "invidious stream selected id={} url_host={:?} url_len={}",
            track.id,
            url_host(&audio),
            audio.len()
        );

        let mut resolved = track.clone();
        resolved.stream_url = Some(audio);
        resolved.audio_tracks = std::mem::take(&mut audio_tracks);
        if resolved.thumbnail_url.is_none() {
            resolved.thumbnail_url = pick_thumbnail(&value, base);
        }
        Ok(resolved)
    }

    async fn resolve_piped(&self, base: &str, track: &Track) -> Result<Track> {
        let url = format!(
            "{}/streams/{}",
            base.trim_end_matches('/'),
            urlencoding(track.id.trim())
        );
        let value = self.get_json(&url, &[]).await?;
        let streams = value
            .get("audioStreams")
            .and_then(Value::as_array)
            .or_else(|| value.get("videoStreams").and_then(Value::as_array))
            .ok_or_else(|| anyhow!("Piped response had no streams"))?;
        crate::cassette_log!(
            "DEBUG",
            "piped resolve streams={} id={} host={:?}",
            streams.len(),
            track.id,
            url_host(base)
        );

        let mut audio_tracks = audio_tracks(streams, base, true);
        let audio = audio_tracks
            .first()
            .map(|track| track.url.clone())
            .or_else(|| {
                streams
                    .first()
                    .and_then(|stream| field_string(stream, &["url"]))
                    .and_then(|raw| absolute_url(base, &raw))
            })
            .ok_or_else(|| anyhow!("Piped response had no playable URL"))?;
        crate::cassette_log!(
            "INFO",
            "piped audio tracks id={} count={} labels={:?}",
            track.id,
            audio_tracks.len(),
            audio_tracks
                .iter()
                .map(|audio| audio.label.as_str())
                .collect::<Vec<_>>()
        );
        crate::cassette_log!(
            "INFO",
            "piped stream selected id={} url_host={:?} url_len={}",
            track.id,
            url_host(&audio),
            audio.len()
        );

        let mut resolved = track.clone();
        resolved.stream_url = Some(audio);
        resolved.audio_tracks = std::mem::take(&mut audio_tracks);
        if resolved.thumbnail_url.is_none() {
            resolved.thumbnail_url = pick_thumbnail(&value, base);
        }
        Ok(resolved)
    }

    async fn resolve_video_invidious(&self, base: &str, track: &Track) -> Result<VideoSource> {
        let url = format!(
            "{}/api/v1/videos/{}",
            base.trim_end_matches('/'),
            urlencoding(track.id.trim())
        );
        let value = self.get_json(&url, &[]).await?;
        let adaptive = value
            .get("adaptiveFormats")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("Invidious response had no adaptive formats"))?;
        let progressive: &[Value] = value
            .get("formatStreams")
            .and_then(Value::as_array)
            .map_or(&[], Vec::as_slice);

        let video = adaptive
            .iter()
            .chain(progressive.iter())
            .filter(|format| {
                field_string(format, &["type"]).is_some_and(|kind| kind.starts_with("video/"))
            })
            .filter(|format| {
                field_string(format, &["url"]).is_some_and(|url| {
                    !url.contains("playlist/index.m3u8")
                        && !url.contains("/itag/616/")
                        && field_string(format, &["itag"]).as_deref() != Some("616")
                })
            })
            .min_by_key(|format| quality_rank(format))
            .ok_or_else(|| anyhow!("Invidious response had no video stream"))?;
        let video_url = field_string(video, &["url"])
            .and_then(|raw| absolute_url(base, &raw))
            .ok_or_else(|| anyhow!("Invidious video stream had no URL"))?;

        let audio_tracks = audio_tracks(&adaptive, base, false);
        crate::cassette_log!(
            "INFO",
            "invidious audio tracks id={} count={} labels={:?}",
            track.id,
            audio_tracks.len(),
            audio_tracks
                .iter()
                .map(|audio| audio.label.as_str())
                .collect::<Vec<_>>()
        );

        let hls_url =
            field_string(&value, &["hlsUrl", "hls_url"]).and_then(|raw| absolute_url(base, &raw));
        crate::cassette_log!(
            "INFO",
            "invidious hls url id={} present={}",
            track.id,
            hls_url.is_some()
        );

        Ok(VideoSource {
            video_url,
            hls_url,
            quality: field_string(video, &["qualityLabel", "quality", "resolution"]),
            audio_tracks,
        })
    }

    async fn resolve_video_piped(&self, base: &str, track: &Track) -> Result<VideoSource> {
        let url = format!(
            "{}/streams/{}",
            base.trim_end_matches('/'),
            urlencoding(track.id.trim())
        );
        let value = self.get_json(&url, &[]).await?;
        let videos = value
            .get("videoStreams")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("Piped response had no video streams"))?;
        let audios: &[Value] = value
            .get("audioStreams")
            .and_then(Value::as_array)
            .map_or(&[], Vec::as_slice);

        let video = videos
            .iter()
            .filter(|format| field_string(format, &["url"]).is_some())
            .min_by_key(|format| quality_rank(format))
            .ok_or_else(|| anyhow!("Piped response had no video stream"))?;
        let video_url = field_string(video, &["url"])
            .and_then(|raw| absolute_url(base, &raw))
            .ok_or_else(|| anyhow!("Piped video stream had no URL"))?;
        let audio_tracks = audio_tracks(audios, base, false);
        crate::cassette_log!(
            "INFO",
            "piped audio tracks id={} count={} labels={:?}",
            track.id,
            audio_tracks.len(),
            audio_tracks
                .iter()
                .map(|audio| audio.label.as_str())
                .collect::<Vec<_>>()
        );

        let hls_url = field_string(&value, &["hlsUrl", "hls_url", "hls"])
            .and_then(|raw| absolute_url(base, &raw));
        crate::cassette_log!(
            "INFO",
            "piped hls url id={} present={}",
            track.id,
            hls_url.is_some()
        );

        Ok(VideoSource {
            video_url,
            hls_url,
            quality: field_string(video, &["quality", "qualityLabel", "resolution", "format"]),
            audio_tracks,
        })
    }
}

fn audio_tracks(formats: &[Value], base: &str, rodio_compatible: bool) -> Vec<AudioTrack> {
    let mut by_language: HashMap<String, (u64, AudioTrack)> = HashMap::new();
    for format in formats.iter().filter(|format| {
        field_string(format, &["type"]).is_some_and(|kind| kind.starts_with("audio/"))
            && field_string(format, &["url"]).is_some()
    }) {
        let Some(raw_url) = field_string(format, &["url"]) else {
            continue;
        };
        let Some(url) = absolute_url(base, &raw_url) else {
            continue;
        };
        let (kind, language, _) = audio_track_tags(&url);
        let language = if language.is_empty() {
            "und".to_string()
        } else {
            language
        };
        let key = format!("{kind}:{language}");
        let track = AudioTrack {
            id: key.clone(),
            label: audio_track_label(&language, &kind),
            language: language.clone(),
            kind,
            url: url.clone(),
            codec: field_string(format, &["type", "codec", "codecs", "format"]).unwrap_or_default(),
            bitrate: field_u64(format, &["bitrate"]),
            sample_rate: field_u64(format, &["audioSampleRate", "sampleRate"]),
            channels: field_u64(format, &["audioChannels", "channels"]),
        };
        let score = audio_quality_score(format, rodio_compatible);
        let replace = by_language
            .get(&key)
            .is_none_or(|(current_score, _)| score > *current_score);
        if replace {
            by_language.insert(key, (score, track));
        }
    }

    let mut tracks: Vec<AudioTrack> = by_language.into_values().map(|(_, track)| track).collect();
    tracks.sort_by(|left, right| {
        let left_rank = if left.kind == "original" { 0 } else { 1 };
        let right_rank = if right.kind == "original" { 0 } else { 1 };
        language_rank(&left.language)
            .cmp(&language_rank(&right.language))
            .then_with(|| left_rank.cmp(&right_rank))
            .then_with(|| left.label.cmp(&right.label))
    });
    tracks
}

fn audio_track_tags(url: &str) -> (String, String, String) {
    let Some(parsed) = Url::parse(url).ok() else {
        return ("original".into(), "en".into(), String::new());
    };
    for (key, value) in parsed.query_pairs() {
        if key != "xtags" {
            continue;
        }
        let mut kind = "original".to_string();
        let mut language = "en".to_string();
        let mut content = String::new();
        for tag in value.split(':') {
            if let Some(rest) = tag.strip_prefix("acont=") {
                kind = rest.to_string();
            } else if let Some(rest) = tag.strip_prefix("lang=") {
                language = rest.to_string();
            } else if tag.starts_with("acont") || tag.starts_with("lang") {
                content = tag.to_string();
            }
        }
        return (kind, language, content);
    }
    ("original".into(), "en".into(), String::new())
}

fn audio_track_label(language: &str, kind: &str) -> String {
    let name = match language {
        "de" => "German",
        "en" => "English",
        "es" | "es-419" => "Spanish",
        "fr" => "French",
        "hi" => "Hindi",
        "id" => "Indonesian",
        "it" => "Italian",
        "ja" => "Japanese",
        "ko" => "Korean",
        "pt" => "Portuguese",
        "ru" => "Russian",
        "und" => "Unknown",
        other => other,
    };
    if kind == "original" {
        format!("{name} (original)")
    } else {
        format!("{name} (dub)")
    }
}

fn language_rank(language: &str) -> u64 {
    match language {
        "pt" => 0,
        "en" => 1,
        "de" => 2,
        "es" | "es-419" => 3,
        "fr" => 4,
        "id" => 5,
        "ru" => 6,
        "ja" => 7,
        "ko" => 8,
        "hi" => 9,
        "it" => 10,
        "und" => 100,
        _ => 50,
    }
}

fn audio_quality_score(format: &Value, rodio_compatible: bool) -> u64 {
    let codec = field_string(format, &["type", "codec", "codecs", "format"]).unwrap_or_default();
    let sample_rate = field_u64(format, &["audioSampleRate", "sampleRate"]);
    let bitrate = field_u64(format, &["bitrate"]);
    let channels = field_u64(format, &["audioChannels", "channels"]);
    let quality = field_string(format, &["audioQuality", "quality"]).unwrap_or_default();
    let codec_bonus = if rodio_compatible {
        if codec.contains("mp4a.40.2") || codec.contains("aac-lc") {
            50_000
        } else if codec.contains("opus") {
            20_000
        } else {
            0
        }
    } else if codec.contains("opus") {
        50_000
    } else if codec.contains("mp4a.40.2") || codec.contains("aac-lc") {
        20_000
    } else {
        0
    };
    let quality_bonus = match quality.as_str() {
        "AUDIO_QUALITY_HIGH" => 400_000,
        "AUDIO_QUALITY_MEDIUM" => 200_000,
        _ => 0,
    };
    codec_bonus + quality_bonus + bitrate + sample_rate * 2 + channels * 10_000
}

fn quality_rank(format: &Value) -> u64 {
    let label = field_string(format, &["qualityLabel", "quality", "resolution", "height"])
        .unwrap_or_default();
    let height = if let Some(number) = label
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse::<u64>()
        .ok()
    {
        number
    } else {
        field_u64(format, &["height"])
    };
    let codec = field_string(format, &["type", "codec", "codecs", "format"]).unwrap_or_default();
    let codec_penalty = if codec.contains("avc1") || codec.contains("h264") {
        0
    } else if codec.contains("vp9") {
        200
    } else if codec.contains("av01") {
        400
    } else {
        800
    };
    let resolution_penalty = match height {
        0 => 10_000,
        1..=360 => 4_000,
        361..=480 => 2_500,
        481..=720 => 800,
        721..=1080 => 0,
        _ => 1_500,
    };
    codec_penalty + resolution_penalty
}

fn field_string(value: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(text) = value.get(*key).and_then(Value::as_str)
            && !text.is_empty()
        {
            return Some(text.to_string());
        }
    }
    None
}

fn field_u64(value: &Value, keys: &[&str]) -> u64 {
    for key in keys {
        if let Some(number) = value.get(*key).and_then(Value::as_u64) {
            return number;
        }
        if let Some(text) = value.get(*key).and_then(Value::as_str)
            && let Ok(number) = text.parse::<u64>()
        {
            return number;
        }
    }
    0
}

fn pick_thumbnail(value: &Value, base: &str) -> Option<String> {
    if let Some(images) = value.get("videoThumbnails").and_then(Value::as_array) {
        let chosen = images
            .iter()
            .find(|image| {
                matches!(
                    field_string(image, &["quality"]).as_deref(),
                    Some("medium") | Some("high") | Some("hd720")
                )
            })
            .or_else(|| images.first());
        if let Some(image) = chosen
            && let Some(raw) = field_string(image, &["url"])
            && let Some(url) = absolute_url(base, &raw)
        {
            return Some(url);
        }
    }
    let raw = field_string(value, &["thumbnailUrl", "thumbnail", "videoThumbnail"])?;
    absolute_url(base, &raw)
}

fn absolute_url(base: &str, raw: &str) -> Option<String> {
    if raw.starts_with("http://") || raw.starts_with("https://") {
        return Some(raw.to_string());
    }
    let raw = if raw.starts_with("//") {
        format!("https:{raw}")
    } else {
        raw.to_string()
    };
    Url::parse(base)
        .ok()?
        .join(&raw)
        .ok()
        .map(|url| url.to_string())
}

pub fn url_host(value: &str) -> Option<String> {
    Url::parse(value)
        .ok()?
        .host_str()
        .map(|host| host.to_string())
}

fn video_id(raw: &str) -> String {
    if let Some(index) = raw.find("v=") {
        return raw[index + 2..]
            .split(['&', '#', '?'])
            .next()
            .unwrap_or("")
            .to_string();
    }
    if let Some(index) = raw.find("youtu.be/") {
        return raw[index + 9..]
            .split(['?', '#', '/'])
            .next()
            .unwrap_or("")
            .to_string();
    }
    raw.rsplit('/').next().unwrap_or(raw).to_string()
}

fn urlencoding(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}
