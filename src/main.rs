mod log;

mod api;
mod app;
mod art;
mod config;
mod library;
mod model;
mod player;
mod playlists;
mod ui;
mod video;

use std::{io, time::Duration};

use anyhow::{Context, Result};
use crossterm::{
    event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::{
    api::MusicClient,
    app::{App, InputKind, Tab},
    art::ArtLoader,
    config::cache_dir,
    model::TrackSource,
    player::PlayerHandle,
    video::VideoHandle,
};

#[tokio::main]
async fn main() -> Result<()> {
    log::init();
    crate::cassette_log!(
        "INFO",
        "cassette starting version={}",
        env!("CARGO_PKG_VERSION")
    );

    let config = config::Config::load()?;
    crate::cassette_log!(
        "INFO",
        "config loaded path={} backend={:?} invidious={} piped={} local_dirs={}",
        config::Config::path().display(),
        config.backend,
        config.invidious_instances.len(),
        config.piped_instances.len(),
        config.local_dirs.len()
    );

    let local_tracks = library::scan_dirs(&config.local_dirs)?;
    crate::cassette_log!("INFO", "startup local scan tracks={}", local_tracks.len());

    let playlist_source = match config.backend {
        config::BackendKind::Invidious => TrackSource::Invidious,
        config::BackendKind::Piped => TrackSource::Piped,
    };
    let playlist_files = match playlists::load(&config.playlist_header_url).await {
        Ok(files) => {
            crate::cassette_log!(
                "INFO",
                "playlist catalog loaded header={} playlists={}",
                config.playlist_header_url,
                files.len()
            );
            files
        }
        Err(error) => {
            crate::cassette_log!(
                "WARN",
                "playlist catalog fetch failed error={error}; using cache"
            );
            let files = playlists::load_cache();
            crate::cassette_log!("INFO", "playlist cache loaded playlists={}", files.len());
            files
        }
    };
    let playlist_data = playlists::into_playlists(playlist_files, playlist_source);

    let client = MusicClient::new(config.clone())?;
    crate::cassette_log!(
        "INFO",
        "api client ready active={}",
        client.instance_label()
    );

    let player = PlayerHandle::spawn(config.volume);
    crate::cassette_log!("INFO", "player thread spawned volume={}", config.volume);

    let video = VideoHandle::spawn();
    crate::cassette_log!("INFO", "video thread spawned");

    enable_raw_mode().context("could not enable raw mode")?;
    execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste)
        .context("could not enter alternate screen")?;
    let terminal_guard = TerminalGuard;
    crate::cassette_log!("INFO", "terminal raw mode enabled");

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend).context("could not create terminal")?;
    crate::cassette_log!("INFO", "terminal initialized");

    let art = ArtLoader::new(cache_dir()).context("could not initialize thumbnail renderer")?;
    let protocol_label = art.protocol_label();
    crate::cassette_log!(
        "INFO",
        "thumbnail renderer ready protocol={}",
        protocol_label
    );

    let mut app = App::new(
        config,
        client,
        player,
        video,
        art,
        protocol_label,
        local_tracks,
        playlist_data,
    );
    app.status = format!("Log: {}", log::path().display());
    app.refresh_art();
    crate::cassette_log!("INFO", "app ready");

    loop {
        terminal.draw(|frame| ui::draw(frame, &app))?;

        if event::poll(Duration::from_millis(80))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            crate::cassette_log!(
                "DEBUG",
                "key code={:?} modifiers={:?} input={:?} buffer={:?}",
                key.code,
                key.modifiers,
                app.input_kind,
                app.input
            );

            if app.input_kind.is_some() {
                match key.code {
                    KeyCode::Esc => {
                        crate::cassette_log!("INFO", "action=cancel_input");
                        app.cancel_input();
                    }
                    KeyCode::Backspace => {
                        crate::cassette_log!("DEBUG", "action=pop_input");
                        app.pop_input();
                    }
                    KeyCode::Enter => {
                        crate::cassette_log!("INFO", "action=submit_input value={:?}", app.input);
                        app.submit_input();
                    }
                    KeyCode::Char(character) if !character.is_control() => {
                        crate::cassette_log!("TRACE", "action=push_input char={:?}", character);
                        app.push_input(character);
                    }
                    _ => {
                        crate::cassette_log!("DEBUG", "action=unhandled_input_key");
                    }
                }
            } else {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => {
                        crate::cassette_log!("INFO", "action=quit");
                        app.quit();
                    }
                    KeyCode::Char('/') => {
                        crate::cassette_log!("INFO", "action=begin_search");
                        app.begin_input(InputKind::Search);
                    }
                    KeyCode::Char('a') => {
                        crate::cassette_log!("INFO", "action=begin_add_dir");
                        app.begin_input(InputKind::AddDir);
                    }
                    KeyCode::Char('s') => {
                        crate::cassette_log!(
                            "INFO",
                            "action=switch_backend from={:?}",
                            app.config.backend
                        );
                        if let Err(error) = app.switch_backend() {
                            crate::cassette_log!("ERROR", "backend switch failed error={error}");
                            app.status = format!("Backend switch failed: {error}");
                        } else {
                            crate::cassette_log!(
                                "INFO",
                                "backend switch ok to={:?}",
                                app.config.backend
                            );
                        }
                    }
                    KeyCode::Char('t') | KeyCode::Char('T') => {
                        crate::cassette_log!("INFO", "action=cycle_theme");
                        app.cycle_theme();
                    }
                    KeyCode::Tab => {
                        let next = match app.tab {
                            Tab::Search => Tab::Local,
                            Tab::Local => Tab::Playlists,
                            Tab::Playlists => Tab::Search,
                        };
                        crate::cassette_log!(
                            "INFO",
                            "action=switch_tab from={:?} to={:?}",
                            app.tab,
                            next
                        );
                        app.set_tab(next);
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        crate::cassette_log!(
                            "DEBUG",
                            "action=move_selection delta=1 index={} count={}",
                            app.selected,
                            if app.tab == Tab::Playlists && app.active_playlist.is_none() {
                                app.visible_catalog_count()
                            } else {
                                app.visible_tracks().len()
                            }
                        );
                        app.move_selection(1);
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        crate::cassette_log!(
                            "DEBUG",
                            "action=move_selection delta=-1 index={} count={}",
                            app.selected,
                            if app.tab == Tab::Playlists && app.active_playlist.is_none() {
                                app.visible_catalog_count()
                            } else {
                                app.visible_tracks().len()
                            }
                        );
                        app.move_selection(-1);
                    }
                    KeyCode::Enter => {
                        if app.tab == Tab::Playlists && app.active_playlist.is_none() {
                            crate::cassette_log!(
                                "INFO",
                                "action=open_playlist index={} title={:?}",
                                app.selected,
                                app.selected_playlist()
                                    .map(|playlist| playlist.title.as_str())
                            );
                            app.open_catalog_item();
                        } else {
                            crate::cassette_log!(
                                "INFO",
                                "action=play_selected index={} title={:?} source={:?}",
                                app.selected,
                                app.selected_track().map(|track| track.title.as_str()),
                                app.selected_track().map(|track| track.source)
                            );
                            app.play_selected().await;
                        }
                    }
                    KeyCode::Char('v') | KeyCode::Char('V') => {
                        crate::cassette_log!(
                            "INFO",
                            "action=play_video_selected index={} title={:?}",
                            app.selected,
                            app.selected_track().map(|track| track.title.as_str())
                        );
                        app.play_video_selected().await;
                    }
                    KeyCode::Char(' ') => {
                        crate::cassette_log!(
                            "INFO",
                            "action=toggle_pause paused_before={}",
                            app.paused
                        );
                        app.toggle_pause();
                    }
                    KeyCode::Char('x') => {
                        crate::cassette_log!("INFO", "action=stop");
                        app.stop_video();
                        app.player.stop();
                        app.progress = (0.0, 0.0);
                        app.paused = false;
                        app.status = "Stopped".into();
                    }
                    KeyCode::Left => {
                        crate::cassette_log!(
                            "INFO",
                            "action=seek delta=-10 position={} duration={}",
                            app.progress.0,
                            app.progress.1
                        );
                        let (position, duration) = app.progress;
                        if duration > 0.0 {
                            let target = (position - 10.0).max(0.0);
                            if app.video_source.is_some() {
                                app.video.seek(app.video.generation(), target);
                                app.progress.0 = target;
                            } else {
                                app.player.seek((target / duration) as f64);
                            }
                        }
                    }
                    KeyCode::Right => {
                        crate::cassette_log!(
                            "INFO",
                            "action=seek delta=10 position={} duration={}",
                            app.progress.0,
                            app.progress.1
                        );
                        let (position, duration) = app.progress;
                        if duration > 0.0 {
                            let target = (position + 10.0).min(duration);
                            if app.video_source.is_some() {
                                app.video.seek(app.video.generation(), target);
                                app.progress.0 = target;
                            } else {
                                app.player.seek((target / duration) as f64);
                            }
                        }
                    }
                    KeyCode::Char('n') => {
                        crate::cassette_log!("INFO", "action=play_next");
                        if app.video_source.is_some() {
                            app.play_next_video().await;
                        } else {
                            app.play_next().await;
                        }
                    }
                    KeyCode::Char('p') | KeyCode::Char('P') => {
                        crate::cassette_log!("INFO", "action=open_playlists");
                        if app.video_source.is_some() {
                            app.stop_video();
                        }
                        app.reset_catalog();
                        app.set_tab(Tab::Playlists);
                        app.status = if app.visible_catalog_count() == 0 {
                            "No playlists available".into()
                        } else {
                            format!("{} items", app.visible_catalog_count())
                        };
                    }
                    KeyCode::Backspace => {
                        app.close_catalog_folder();
                    }
                    KeyCode::Char('l') | KeyCode::Char('L') => {
                        crate::cassette_log!("INFO", "action=cycle_audio_track");
                        app.cycle_audio_track(if key.code == KeyCode::Char('L') {
                            -1
                        } else {
                            1
                        });
                    }
                    KeyCode::Char('d') | KeyCode::Char('D') => {
                        crate::cassette_log!("INFO", "action=cycle_dub");
                        app.cycle_dub(if key.code == KeyCode::Char('D') {
                            -1
                        } else {
                            1
                        });
                    }
                    KeyCode::Char(',') => {
                        crate::cassette_log!("INFO", "action=play_previous");
                        if app.video_source.is_some() {
                            app.play_previous_video().await;
                        } else {
                            app.play_previous().await;
                        }
                    }
                    KeyCode::Char('+') | KeyCode::Char('=') => {
                        crate::cassette_log!("INFO", "action=volume_up");
                        app.adjust_volume(0.05);
                    }
                    KeyCode::Char('-') => {
                        crate::cassette_log!("INFO", "action=volume_down");
                        app.adjust_volume(-0.05);
                    }
                    _ => {
                        crate::cassette_log!("DEBUG", "action=unhandled_key");
                    }
                }
            }
        }

        while let Some(event) = app.try_recv_event() {
            app.handle_event(event);
        }
        while let Some(event) = app.video.try_recv_event() {
            app.handle_video_event(event);
        }
        while let Some(event) = app.player.try_recv_event() {
            app.handle_player_event(event);
        }

        if let Some(generation) = app.take_video_auto_next() {
            crate::cassette_log!(
                "INFO",
                "video auto next requested from_generation={}",
                generation
            );
            app.play_next_video().await;
        }

        if app.should_quit {
            break;
        }
    }

    crate::cassette_log!("INFO", "shutdown complete");
    drop(terminal_guard);
    println!("Log: {}", log::path().display());
    Ok(())
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            LeaveAlternateScreen,
            DisableBracketedPaste,
            crossterm::cursor::Show
        );
    }
}
