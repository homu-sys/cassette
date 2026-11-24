use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::PathBuf,
    time::Duration,
};

use anyhow::Result;
use image::DynamicImage;
use ratatui::layout::Size;
use ratatui_image::{Resize, picker::Picker, protocol::Protocol};
use reqwest::Client;

use crate::{api::url_host, model::Track};

#[derive(Clone)]
pub struct ArtLoader {
    http: Client,
    cache_dir: PathBuf,
    picker: Picker,
}

impl ArtLoader {
    pub fn new(cache_dir: PathBuf) -> Result<Self> {
        crate::cassette_log!("INFO", "thumbnail renderer init cache={:?}", cache_dir);
        fs::create_dir_all(&cache_dir)?;
        let http = Client::builder()
            .user_agent(concat!("cassette/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(20))
            .build()?;
        let picker = Picker::from_query_stdio().unwrap_or_else(|error| {
            crate::cassette_log!("WARN", "thumbnail protocol query failed error={error}");
            Picker::halfblocks()
        });
        crate::cassette_log!(
            "INFO",
            "thumbnail protocol selected {:?}",
            picker.protocol_type()
        );
        Ok(Self {
            http,
            cache_dir,
            picker,
        })
    }

    pub fn protocol_label(&self) -> String {
        format!("{:?}", self.picker.protocol_type())
    }

    pub fn protocol(&self, image: DynamicImage, size: Size) -> Option<Protocol> {
        self.picker
            .new_protocol(image, size, Resize::Fit(None))
            .ok()
    }

    pub async fn load(&self, track: &Track) -> Option<DynamicImage> {
        crate::cassette_log!(
            "DEBUG",
            "art load start id={} title={:?} source={:?}",
            track.id,
            track.title,
            track.source
        );
        let bytes = if let Some(path) = &track.thumbnail_path {
            crate::cassette_log!("DEBUG", "art local file path={:?}", path);
            fs::read(path).ok()?
        } else if let Some(url) = &track.thumbnail_url {
            let destination = self.cache_dir.join(format!("url-{}.png", hash(url)));
            if destination.exists() {
                crate::cassette_log!("DEBUG", "art cache hit path={:?}", destination);
                fs::read(&destination).ok()?
            } else {
                crate::cassette_log!(
                    "INFO",
                    "art download start url_host={:?} url_len={}",
                    url_host(url),
                    url.len()
                );
                let response = self
                    .http
                    .get(url)
                    .send()
                    .await
                    .ok()?
                    .error_for_status()
                    .ok()?;
                crate::cassette_log!(
                    "DEBUG",
                    "art download response status={} url_host={:?}",
                    response.status(),
                    url_host(url)
                );
                let raw = response.bytes().await.ok()?;
                crate::cassette_log!("DEBUG", "art download bytes={}", raw.len());
                let decoded = image::load_from_memory(&raw).ok()?.to_rgba8();
                decoded.save(&destination).ok()?;
                crate::cassette_log!("DEBUG", "art cache saved path={:?}", destination);
                fs::read(&destination).ok()?
            }
        } else {
            crate::cassette_log!("DEBUG", "art skipped because track has no thumbnail");
            return None;
        };
        let image = image::load_from_memory(&bytes).ok();
        crate::cassette_log!("DEBUG", "art decode ok={}", image.is_some());
        image
    }
}

fn hash(value: &str) -> String {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}
