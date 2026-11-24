use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{self, BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    process::{Child, Command as ProcessCommand, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use serde_json::{Value, json};

use crate::config;

enum Command {
    Play {
        generation: u64,
        url: String,
        audio_tracks: Vec<(String, String)>,
        title: String,
        volume: f32,
        start: f32,
        audio_lang: String,
    },
    Stop,
    Seek {
        generation: u64,
        seconds: f32,
    },
    SetPaused {
        generation: u64,
        paused: bool,
    },
    CycleAudio {
        generation: u64,
        delta: i32,
    },
    SelectAudio {
        generation: u64,
        aid: i64,
    },
    SetVolume {
        generation: u64,
        volume: f32,
    },
    Shutdown,
}

#[derive(Clone, Debug)]
pub enum VideoEvent {
    Started {
        generation: u64,
    },
    Progress {
        generation: u64,
        position: f64,
        duration: f64,
    },
    Paused {
        generation: u64,
        paused: bool,
    },
    AudioChanged {
        generation: u64,
        label: String,
    },
    Stopped,
    Finished {
        generation: u64,
    },
    Error(String),
}

#[derive(Clone)]
struct AudioTrackInfo {
    id: i64,
    label: String,
    selected: bool,
    external: bool,
}

struct ActiveMpv {
    child: Child,
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    socket: PathBuf,
    generation: u64,
    request_id: u64,
    pending: HashMap<u64, String>,
    observed: HashMap<u64, String>,
    tracks: Vec<AudioTrackInfo>,
    audio_labels: Vec<String>,
    position: f64,
    duration: f64,
    paused: bool,
    initial_audio_selected: bool,
    finished: bool,
}

impl ActiveMpv {
    fn send(&mut self, command: Vec<Value>) -> Result<u64, String> {
        self.request_id += 1;
        let request_id = self.request_id;
        let message = json!({ "command": command, "request_id": request_id });
        let line = serde_json::to_string(&message).map_err(|error| error.to_string())?;
        self.writer
            .write_all(line.as_bytes())
            .and_then(|_| self.writer.write_all(b"\n"))
            .and_then(|_| self.writer.flush())
            .map_err(|error| format!("mpv IPC write: {error}"))?;
        Ok(request_id)
    }

    fn get_property(&mut self, name: &str) -> Result<(), String> {
        let request_id = self.send(vec!["get_property".into(), name.into()])?;
        self.pending.insert(request_id, name.to_string());
        Ok(())
    }

    fn observe_properties(&mut self) -> Result<(), String> {
        for (id, name) in [
            (1u64, "time-pos"),
            (2, "duration"),
            (3, "pause"),
            (4, "track-list"),
            (5, "aid"),
        ] {
            self.send(vec![
                "observe_property".into(),
                Value::from(id),
                name.into(),
            ])?;
            self.observed.insert(id, name.to_string());
        }
        self.get_property("time-pos")?;
        self.get_property("duration")?;
        self.get_property("pause")?;
        self.get_property("track-list")?;
        self.get_property("aid")?;
        Ok(())
    }

    fn emit_progress(&self, event_tx: &Sender<VideoEvent>) {
        let _ = event_tx.send(VideoEvent::Progress {
            generation: self.generation,
            position: self.position,
            duration: self.duration,
        });
    }

    fn emit_audio(&self, event_tx: &Sender<VideoEvent>, label: String) {
        let _ = event_tx.send(VideoEvent::AudioChanged {
            generation: self.generation,
            label,
        });
    }

    fn update_tracks(&mut self, value: &Value, event_tx: &Sender<VideoEvent>) {
        self.tracks = parse_tracks(value);
        if let Some(track) = self.tracks.iter().find(|track| track.selected).cloned() {
            self.update_aid(&Value::from(track.id), event_tx);
        }
    }

    fn update_aid(&mut self, value: &Value, event_tx: &Sender<VideoEvent>) {
        let Some(id) = value
            .as_i64()
            .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
        else {
            return;
        };
        let Some(label) = self.label_for_id(id) else {
            return;
        };
        self.initial_audio_selected = true;
        self.emit_audio(event_tx, label);
    }

    fn label_for_id(&self, id: i64) -> Option<String> {
        if let Some(track) = self.tracks.iter().find(|track| track.id == id) {
            if !track.external {
                return Some(track.label.clone());
            }
            let index = self
                .tracks
                .iter()
                .filter(|track| track.external)
                .position(|track| track.id == id)?;
            return self
                .audio_labels
                .get(index)
                .cloned()
                .or_else(|| Some(track.label.clone()));
        }
        usize::try_from(id)
            .ok()
            .and_then(|index| index.checked_sub(1))
            .and_then(|index| self.audio_labels.get(index).cloned())
    }
}

pub struct VideoHandle {
    commands: Sender<Command>,
    events: Receiver<VideoEvent>,
    generation: AtomicU64,
    thread: Option<JoinHandle<()>>,
}

impl VideoHandle {
    pub fn spawn() -> Self {
        let (commands, command_rx) = mpsc::channel();
        let (event_tx, events) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("cassette-video".into())
            .spawn(move || run(command_rx, event_tx))
            .expect("starting video thread");
        Self {
            commands,
            events,
            generation: AtomicU64::new(0),
            thread: Some(thread),
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    pub fn play(
        &self,
        url: String,
        audio_tracks: Vec<(String, String)>,
        title: String,
        volume: f32,
        start: f32,
        audio_lang: &str,
    ) -> u64 {
        let generation = self.next_generation();
        let _ = self.commands.send(Command::Play {
            generation,
            url,
            audio_tracks,
            title,
            volume,
            start,
            audio_lang: audio_lang.to_string(),
        });
        generation
    }

    pub fn stop(&self) -> u64 {
        let generation = self.next_generation();
        let _ = self.commands.send(Command::Stop);
        generation
    }

    pub fn seek(&self, generation: u64, seconds: f32) {
        let _ = self.commands.send(Command::Seek {
            generation,
            seconds,
        });
    }

    pub fn set_paused(&self, generation: u64, paused: bool) {
        let _ = self
            .commands
            .send(Command::SetPaused { generation, paused });
    }

    pub fn cycle_audio(&self, generation: u64, delta: i32) {
        let _ = self
            .commands
            .send(Command::CycleAudio { generation, delta });
    }

    pub fn select_audio(&self, generation: u64, aid: i64) {
        let _ = self.commands.send(Command::SelectAudio { generation, aid });
    }

    pub fn set_volume(&self, generation: u64, volume: f32) {
        let _ = self
            .commands
            .send(Command::SetVolume { generation, volume });
    }

    pub fn shutdown(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
            crate::cassette_log!("INFO", "video thread joined");
        }
    }

    pub fn try_recv_event(&self) -> Option<VideoEvent> {
        self.events.try_recv().ok()
    }

    fn next_generation(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::SeqCst) + 1
    }
}

fn run(command_rx: Receiver<Command>, event_tx: Sender<VideoEvent>) {
    let mut active: Option<ActiveMpv> = None;

    loop {
        while let Ok(command) = command_rx.try_recv() {
            match command {
                Command::Play {
                    generation,
                    url,
                    audio_tracks,
                    title,
                    volume,
                    start,
                    audio_lang,
                } => {
                    stop_active(&mut active);
                    match spawn_mpv(
                        &url,
                        &audio_tracks,
                        &title,
                        volume,
                        start,
                        &audio_lang,
                        generation,
                    ) {
                        Ok(next) => {
                            active = Some(next);
                            let _ = event_tx.send(VideoEvent::Started { generation });
                        }
                        Err(error) => {
                            let _ = event_tx.send(VideoEvent::Error(error));
                        }
                    }
                }
                Command::Stop => {
                    stop_active(&mut active);
                    let _ = event_tx.send(VideoEvent::Stopped);
                }
                Command::Seek {
                    generation,
                    seconds,
                } => {
                    if let Some(current) = active
                        .as_mut()
                        .filter(|current| current.generation == generation)
                    {
                        let _ = current.send(vec![
                            "seek".into(),
                            Value::from(seconds),
                            "absolute".into(),
                        ]);
                    }
                }
                Command::SetPaused { generation, paused } => {
                    if let Some(current) = active
                        .as_mut()
                        .filter(|current| current.generation == generation)
                    {
                        let _ = current.send(vec![
                            "set_property".into(),
                            "pause".into(),
                            paused.into(),
                        ]);
                    }
                }
                Command::CycleAudio { generation, delta } => {
                    if let Some(current) = active
                        .as_mut()
                        .filter(|current| current.generation == generation)
                    {
                        let direction = if delta >= 0 { "up" } else { "down" };
                        let _ =
                            current.send(vec!["cycle".into(), "audio".into(), direction.into()]);
                    }
                }
                Command::SelectAudio { generation, aid } => {
                    if let Some(current) = active
                        .as_mut()
                        .filter(|current| current.generation == generation)
                    {
                        let _ = current.send(vec![
                            "set_property".into(),
                            "aid".into(),
                            Value::from(aid),
                        ]);
                    }
                }
                Command::SetVolume { generation, volume } => {
                    if let Some(current) = active
                        .as_mut()
                        .filter(|current| current.generation == generation)
                    {
                        let _ = current.send(vec![
                            "set_property".into(),
                            "volume".into(),
                            Value::from((volume * 100.0).round()),
                        ]);
                    }
                }
                Command::Shutdown => {
                    stop_active(&mut active);
                    return;
                }
            }
        }

        let Some(current) = active.as_mut() else {
            thread::sleep(Duration::from_millis(10));
            continue;
        };

        match current.child.try_wait() {
            Ok(Some(_)) => {
                if !current.finished {
                    current.finished = true;
                    let _ = event_tx.send(VideoEvent::Finished {
                        generation: current.generation,
                    });
                }
            }
            Ok(None) => {}
            Err(error) => {
                let _ = event_tx.send(VideoEvent::Error(format!("mpv process: {error}")));
                stop_active(&mut active);
                continue;
            }
        }

        let mut line = String::new();
        match current.reader.read_line(&mut line) {
            Ok(0) => {}
            Ok(_) => handle_message(current, line.trim(), &event_tx),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(error) if error.kind() != io::ErrorKind::Interrupted => {
                crate::cassette_log!("ERROR", "mpv IPC read failed error={error}");
                let _ = event_tx.send(VideoEvent::Error(format!("mpv IPC: {error}")));
                stop_active(&mut active);
            }
            Err(_) => {}
        }
    }
}

fn focus_mpv_window(pid: u32) {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
        return;
    }
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(150));
        let expression = format!("hl.dsp.focus({{ window = \"pid:{pid}\" }})");
        for _ in 0..10 {
            let ok = ProcessCommand::new("hyprctl")
                .arg("eval")
                .arg(&expression)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
            if ok {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
    });
}

fn spawn_mpv(
    url: &str,
    audio_tracks: &[(String, String)],
    title: &str,
    volume: f32,
    start: f32,
    audio_lang: &str,
    generation: u64,
) -> Result<ActiveMpv, String> {
    let runtime_dir = config::runtime_dir();
    fs::create_dir_all(&runtime_dir).map_err(|error| error.to_string())?;
    let socket = runtime_dir.join(format!("mpv-{}-{}.sock", std::process::id(), generation));
    let _ = fs::remove_file(&socket);

    let log_path = config::mpv_log_path();
    let log = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&log_path)
        .map_err(|error| format!("mpv log: {error}"))?;
    let log_err = log.try_clone().map_err(|error| error.to_string())?;

    let mut command = ProcessCommand::new("mpv");
    command
        .arg("--no-config")
        .arg(format!("--input-ipc-server={}", socket.display()))
        .arg("--force-window=yes")
        .arg("--keep-open=yes")
        .arg("--msg-level=all=info")
        .arg("--audio-client-name=cassette")
        .arg(format!("--force-media-title={title}"))
        .arg(format!("--alang={audio_lang}"));
    if !audio_tracks.is_empty() {
        command.arg("--aid=1");
        for (_, audio_url) in audio_tracks {
            command.arg(format!("--audio-file={audio_url}"));
        }
    }
    command
        .arg(format!("--volume={}", (volume * 100.0).round()))
        .arg(format!("--start={start}"))
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));

    let child = command
        .spawn()
        .map_err(|error| format!("starting mpv: {error}"))?;
    focus_mpv_window(child.id());

    let mut stream = None;
    for _ in 0..80 {
        if let Ok(connected) = UnixStream::connect(&socket) {
            stream = Some(connected);
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let stream = stream.ok_or_else(|| "could not connect to mpv IPC socket".to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_millis(50)))
        .map_err(|error| error.to_string())?;
    let writer = stream.try_clone().map_err(|error| error.to_string())?;
    let mut active = ActiveMpv {
        child,
        reader: BufReader::new(stream),
        writer,
        socket,
        generation,
        request_id: 0,
        pending: HashMap::new(),
        observed: HashMap::new(),
        tracks: Vec::new(),
        audio_labels: audio_tracks
            .iter()
            .map(|(label, _)| label.clone())
            .collect(),
        position: 0.0,
        duration: 0.0,
        paused: false,
        initial_audio_selected: !audio_tracks.is_empty(),
        finished: false,
    };
    active.observe_properties()?;
    Ok(active)
}

fn stop_active(active: &mut Option<ActiveMpv>) {
    if let Some(mut current) = active.take() {
        let _ = current.send(vec!["quit".into()]);
        let _ = current.child.wait();
        let _ = fs::remove_file(&current.socket);
    }
}

fn handle_message(active: &mut ActiveMpv, line: &str, event_tx: &Sender<VideoEvent>) {
    let Ok(message) = serde_json::from_str::<Value>(line) else {
        return;
    };
    let event = message.get("event").and_then(Value::as_str);
    match event {
        Some("property-change") => {
            let name = message
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    message.get("id").and_then(Value::as_u64).and_then(|id| {
                        active
                            .observed
                            .get(&id)
                            .or_else(|| active.pending.get(&id))
                            .cloned()
                    })
                });
            if let Some(name) = name.as_deref() {
                if let Some(value) = message.get("data") {
                    handle_property(active, name, value, event_tx);
                }
            }
        }
        Some("file-loaded") => {
            let _ = active.get_property("track-list");
            let _ = active.get_property("duration");
        }
        Some("end-file") => {
            let reason = message
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("eof");
            if !active.finished {
                active.finished = true;
                let _ = event_tx.send(VideoEvent::Finished {
                    generation: active.generation,
                });
            }
            crate::cassette_log!("INFO", "mpv end-file reason={reason}");
        }
        _ => {
            if let (Some(request_id), Some(error)) = (
                message.get("request_id").and_then(Value::as_u64),
                message.get("error").and_then(Value::as_str),
            ) {
                if error == "success" {
                    if let Some(name) = active.pending.get(&request_id).cloned() {
                        if let Some(value) = message.get("data") {
                            handle_property(active, &name, value, event_tx);
                        }
                    }
                }
                active.pending.remove(&request_id);
            }
        }
    }
}

fn handle_property(
    active: &mut ActiveMpv,
    name: &str,
    value: &Value,
    event_tx: &Sender<VideoEvent>,
) {
    match name {
        "time-pos" => {
            active.position = value.as_f64().unwrap_or(0.0);
            active.emit_progress(event_tx);
        }
        "duration" => {
            active.duration = value.as_f64().unwrap_or(0.0);
            active.emit_progress(event_tx);
        }
        "pause" => {
            active.paused = value.as_bool().unwrap_or(false);
            let _ = event_tx.send(VideoEvent::Paused {
                generation: active.generation,
                paused: active.paused,
            });
        }
        "track-list" => active.update_tracks(value, event_tx),
        "aid" => active.update_aid(value, event_tx),
        _ => {}
    }
}

fn parse_tracks(value: &Value) -> Vec<AudioTrackInfo> {
    let Some(tracks) = value.as_array() else {
        return Vec::new();
    };
    tracks
        .iter()
        .filter(|track| track.get("type").and_then(Value::as_str) == Some("audio"))
        .filter_map(|track| {
            let lang = track
                .get("lang")
                .and_then(Value::as_str)
                .unwrap_or("und")
                .to_string();
            let title = track
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            Some(AudioTrackInfo {
                id: track.get("id").and_then(Value::as_i64)?,
                label: audio_label(&lang, &title),
                selected: track
                    .get("selected")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                external: track
                    .get("external")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
        })
        .collect()
}

fn audio_label(lang: &str, title: &str) -> String {
    let name = match lang {
        "de" => "German",
        "en" => "English",
        "es" | "es-419" => "Spanish",
        "fr" => "French",
        "id" => "Indonesian",
        "it" => "Italian",
        "ja" => "Japanese",
        "ko" => "Korean",
        "pt" => "Portuguese",
        "ru" => "Russian",
        "und" => "Unknown",
        other => other,
    };
    let kind = if title.contains("original") || (name == "English" && !title.contains("dub")) {
        "original"
    } else {
        "dub"
    };
    format!("{name} ({kind})")
}
