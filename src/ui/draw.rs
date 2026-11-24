use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    symbols::border::DOUBLE,
    text::{Line, Span},
    widgets::{Block, Clear, Gauge, List, ListItem, ListState, Paragraph},
};
use ratatui_image::Image as ImageWidget;

use super::palette::{Palette, UiStyle, palette, ui_style};
use crate::app::{App, CatalogEntry, InputKind, Tab, local_dir_label};

pub fn draw(frame: &mut Frame<'_>, app: &App) {
    let p = palette(app.config.theme);
    let style = ui_style(app.config.theme);
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(p.bg).fg(p.fg)),
        area,
    );

    let footer_height = match style {
        UiStyle::Classic | UiStyle::Grid => 3,
        UiStyle::Compact | UiStyle::Focus => 2,
        UiStyle::Minimal => 1,
    };
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(footer_height),
    ])
    .split(area);
    render_header(frame, chunks[0], app, &p, style);

    if app.video_source.is_some() {
        render_video(frame, chunks[1], app, &p, style);
    } else if matches!(style, UiStyle::Classic | UiStyle::Grid) {
        let main = Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(chunks[1]);
        render_tracks(frame, main[0], app, &p, style);
        render_details(frame, main[1], app, &p, style);
    } else {
        render_tracks(frame, chunks[1], app, &p, style);
    }
    render_footer(frame, chunks[2], app, &p, style);

    if let Some(kind) = app.input_kind {
        render_input(frame, area, kind, &app.input, &p, style);
    }
}

fn panel_block(title: String, p: &Palette, style: UiStyle) -> Block<'static> {
    let block = match style {
        UiStyle::Classic | UiStyle::Compact => Block::bordered(),
        UiStyle::Grid => Block::bordered().border_set(DOUBLE),
        UiStyle::Minimal | UiStyle::Focus => Block::default(),
    };
    let block = match style {
        UiStyle::Minimal | UiStyle::Focus => block,
        _ => block.title(title),
    };
    block
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.bg).fg(p.fg))
}

fn render_header(frame: &mut Frame<'_>, area: Rect, app: &App, p: &Palette, style: UiStyle) {
    let title = Span::styled(
        match style {
            UiStyle::Minimal => "cassette",
            UiStyle::Focus => " FOCUS ",
            UiStyle::Grid => "▚ cassette ▚",
            UiStyle::Compact => "cassette>",
            UiStyle::Classic => " cassette ",
        },
        Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
    );
    let status = Span::styled(format!(" {} ", app.status), Style::default().fg(p.fg));
    let spans = match style {
        UiStyle::Minimal => vec![title, status],
        _ => {
            let backend = Span::styled(
                format!(" {} ", app.client.instance_label()),
                Style::default().fg(p.secondary),
            );
            let tab = Span::styled(format!(" {:?} ", app.tab), Style::default().fg(p.tertiary));
            let theme = Span::styled(
                format!(" {} ", app.config.theme),
                Style::default().fg(p.muted),
            );
            vec![title, backend, tab, theme, status]
        }
    };
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_tracks(frame: &mut Frame<'_>, area: Rect, app: &App, p: &Palette, style: UiStyle) {
    if app.tab == Tab::Playlists && app.active_playlist.is_none() {
        render_playlist_list(frame, area, app, p, style);
        return;
    }

    let title = match app.tab {
        Tab::Search => "YouTube Music results".to_string(),
        Tab::Local => "Local library".to_string(),
        Tab::Playlists => app
            .active_playlist()
            .map(|playlist| playlist.title.clone())
            .unwrap_or_else(|| app.catalog_breadcrumb()),
    };
    let block = panel_block(title, p, style);
    let tracks = app.visible_tracks();

    if tracks.is_empty() {
        let message = match app.tab {
            Tab::Search => "Press / to search YouTube Music.",
            Tab::Local => "Press a to add a music folder.",
            Tab::Playlists => "Press Backspace to return to playlists.",
        };
        frame.render_widget(Paragraph::new(message).block(block), area);
        return;
    }

    let items = tracks.iter().enumerate().map(|(index, track)| {
        let marker = if index == app.selected
            && app
                .playing
                .as_ref()
                .is_some_and(|playing| playing.id == track.id)
        {
            ">"
        } else if index == app.selected {
            "*"
        } else {
            " "
        };
        ListItem::new(Line::from(vec![
            Span::styled(
                format!("{marker} "),
                Style::default()
                    .fg(p.secondary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(track.title.clone(), Style::default().fg(p.fg)),
            Span::raw(" — "),
            Span::styled(track.artist.clone(), Style::default().fg(p.muted)),
            Span::raw(" "),
            Span::styled(track.duration_label(), Style::default().fg(p.accent)),
        ]))
    });

    let mut state = ListState::default();
    state.select(Some(app.selected));
    let highlight = match style {
        UiStyle::Focus => Style::default().fg(p.bg).bg(p.secondary),
        UiStyle::Minimal => Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
        _ => Style::default()
            .fg(p.secondary)
            .add_modifier(Modifier::REVERSED),
    };
    let list = List::new(items).block(block).highlight_style(highlight);
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_playlist_list(frame: &mut Frame<'_>, area: Rect, app: &App, p: &Palette, style: UiStyle) {
    let block = panel_block(app.catalog_breadcrumb(), p, style);
    let entries = app.visible_catalog_entries();
    if entries.is_empty() {
        frame.render_widget(
            Paragraph::new(
                "No playlists available. Check CASSETTE_GITHUB_TOKEN for private repos.",
            )
            .block(block),
            area,
        );
        return;
    }

    let items = entries.iter().enumerate().map(|(index, entry)| {
        let marker = if index == app.selected { "*" } else { " " };
        let mut spans = vec![Span::styled(
            format!("{marker} "),
            Style::default()
                .fg(p.secondary)
                .add_modifier(Modifier::BOLD),
        )];
        match entry {
            CatalogEntry::Folder(name) => {
                spans.push(Span::styled(
                    format!("{name}/"),
                    Style::default()
                        .fg(p.secondary)
                        .add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::raw(" — folder"));
            }
            CatalogEntry::Playlist(playlist) => {
                spans.push(Span::styled(
                    playlist.title.clone(),
                    Style::default().fg(p.fg),
                ));
                spans.push(Span::raw(" — "));
                spans.push(Span::styled(
                    format!("{} episodes", playlist.tracks.len()),
                    Style::default().fg(p.accent),
                ));
            }
        }
        ListItem::new(Line::from(spans))
    });

    let mut state = ListState::default();
    state.select(Some(app.selected));
    let highlight = match style {
        UiStyle::Focus => Style::default().fg(p.bg).bg(p.secondary),
        UiStyle::Minimal => Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
        _ => Style::default()
            .fg(p.secondary)
            .add_modifier(Modifier::REVERSED),
    };
    let list = List::new(items).block(block).highlight_style(highlight);
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_details(frame: &mut Frame<'_>, area: Rect, app: &App, p: &Palette, style: UiStyle) {
    let chunks = Layout::vertical([Constraint::Percentage(62), Constraint::Min(0)]).split(area);
    let art_block = panel_block("Thumbnail".into(), p, style);
    let art_area = art_block.inner(chunks[0]);
    frame.render_widget(art_block, chunks[0]);

    if let Some(protocol) = &app.art_protocol {
        let size = protocol.size();
        let width = size.width.min(art_area.width);
        let height = size.height.min(art_area.height);
        let x = art_area.x + art_area.width.saturating_sub(width) / 2;
        let y = art_area.y + art_area.height.saturating_sub(height) / 2;
        frame.render_widget(
            ImageWidget::new(protocol).allow_clipping(true),
            Rect::new(x, y, width, height),
        );
    } else {
        frame.render_widget(
            Paragraph::new("No thumbnail").style(Style::default().fg(p.muted)),
            art_area,
        );
    }

    let lines = if let Some(track) = &app.playing {
        vec![
            Line::from(vec![
                Span::styled("Now: ", Style::default().fg(p.success)),
                Span::styled(track.title.clone(), Style::default().fg(p.fg)),
            ]),
            Line::from(format!("Artist: {}", track.artist)),
            Line::from(format!("Album: {}", track.album)),
            Line::from(format!(
                "Source: {} · {}",
                track.source,
                track.duration_label()
            )),
            Line::from(format!("Protocol: {}", app.protocol_label)),
            Line::from(format!(
                "Audio: {}",
                app.active_audio_track_label().unwrap_or("unknown")
            )),
            Line::from(format!(
                "Local dir: {}",
                local_dir_label(&app.config.local_dirs)
            )),
            Line::from(format!("Volume: {:.0}%", app.config.volume * 100.0)),
        ]
    } else if app.tab == Tab::Playlists
        && app.active_playlist.is_none()
        && app.selected_catalog_folder().is_some()
    {
        let folder = app.selected_catalog_folder().unwrap_or("folder");
        vec![
            Line::from(vec![
                Span::styled("Folder: ", Style::default().fg(p.success)),
                Span::styled(folder.to_string(), Style::default().fg(p.fg)),
            ]),
            Line::from(app.catalog_breadcrumb()),
            Line::from("Enter: open · Backspace: up · p: playlists"),
        ]
    } else if let Some(playlist) = app.selected_playlist() {
        vec![
            Line::from(vec![
                Span::styled("Playlist: ", Style::default().fg(p.success)),
                Span::styled(playlist.title.clone(), Style::default().fg(p.fg)),
            ]),
            Line::from(playlist.description.clone()),
            Line::from(format!("Episodes: {}", playlist.tracks.len())),
            Line::from("Enter: open · p: playlists · q: quit"),
        ]
    } else {
        vec![
            Line::from("Nothing playing"),
            Line::from(format!("Protocol: {}", app.protocol_label)),
            Line::from(format!(
                "Local dir: {}",
                local_dir_label(&app.config.local_dirs)
            )),
        ]
    };
    frame.render_widget(
        Paragraph::new(lines).block(panel_block("Details".into(), p, style)),
        chunks[1],
    );
}

fn render_video(frame: &mut Frame<'_>, area: Rect, app: &App, p: &Palette, style: UiStyle) {
    let title = app
        .playing
        .as_ref()
        .map(|track| track.title.as_str())
        .unwrap_or("video");
    let quality = app
        .video_source
        .as_ref()
        .and_then(|video| video.quality.as_deref())
        .unwrap_or("mpv");
    let audio = app.active_audio_track_label().unwrap_or("Portuguese (dub)");
    let state = if app.paused { "Paused" } else { "Playing" };
    let block = panel_block(format!(" Video · {title} · {quality} "), p, style).title_bottom(
        " v/V video · Space pause · x stop · ←/→ seek · l/L audio · d/D dub · n next · p playlists ",
    );
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);

    let (position, duration) = app.progress;
    let lines = vec![
        Line::from(Span::styled(
            format!("mpv · {state}"),
            Style::default().fg(p.accent),
        )),
        Line::from(format!("Time: {:.0}s / {:.0}s", position, duration)),
        Line::from(format!("Audio: {audio}")),
        Line::from("Video is playing in a separate mpv window."),
    ];
    frame.render_widget(Paragraph::new(lines), inner);

    if inner.height > 4 {
        let ratio = if duration > 0.0 {
            f64::from((position / duration).clamp(0.0, 1.0))
        } else {
            0.0
        };
        let gauge = Rect {
            x: inner.x,
            y: inner.y + inner.height - 2,
            width: inner.width,
            height: 1,
        };
        frame.render_widget(
            Gauge::default()
                .ratio(ratio)
                .gauge_style(Style::default().fg(p.accent)),
            gauge,
        );
    }
}

fn render_footer(frame: &mut Frame<'_>, area: Rect, app: &App, p: &Palette, style: UiStyle) {
    let (position, duration) = app.progress;
    let ratio = if duration > 0.0 {
        (position / duration).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let label = format!(
        "{:02}:{:02} / {:02}:{:02}{}",
        position as u32 / 60,
        position as u32 % 60,
        duration as u32 / 60,
        duration as u32 % 60,
        if app.paused { " (paused)" } else { "" }
    );
    let status = Span::styled(app.status.clone(), Style::default().fg(p.success));

    match style {
        UiStyle::Minimal => {
            frame.render_widget(Paragraph::new(Line::from(status)), area);
        }
        UiStyle::Compact | UiStyle::Focus => {
            let chunks =
                Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(area);
            frame.render_widget(
                Gauge::default()
                    .ratio(ratio as f64)
                    .label(label)
                    .gauge_style(Style::default().fg(p.accent)),
                chunks[0],
            );
            let help = match style {
                UiStyle::Compact => {
                    "/ / a folder · s src · t style · Tab tab · p lists · v video · q quit"
                }
                _ => "t style · Enter open · v video · Space pause · x stop · ←/→ seek · q quit",
            };
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(help, Style::default().fg(p.muted)),
                    status,
                ])),
                chunks[1],
            );
        }
        UiStyle::Classic | UiStyle::Grid => {
            let chunks = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(area);
            frame.render_widget(
                Gauge::default()
                    .ratio(ratio as f64)
                    .label(label)
                    .gauge_style(Style::default().fg(p.accent)),
                chunks[0],
            );
            let help = Span::styled(
                "/ search  a add folder  s backend  t theme  Tab tab  p playlists  j/k move  Enter open/audio  v video  l/L audio  d/D dub  Space pause  x stop  ←/→ seek  n/, next/prev  +/- volume  q quit",
                Style::default().fg(p.muted),
            );
            frame.render_widget(Paragraph::new(Line::from(help)), chunks[1]);
            frame.render_widget(Paragraph::new(Line::from(status)), chunks[2]);
        }
    }
}

fn render_input(
    frame: &mut Frame<'_>,
    area: Rect,
    kind: InputKind,
    input: &str,
    p: &Palette,
    style: UiStyle,
) {
    let width = area.width.saturating_sub(12).min(72).max(24);
    let height = 3;
    let popup = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    let title = match kind {
        InputKind::Search => "Search YouTube Music",
        InputKind::AddDir => "Add local music folder",
    };
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(format!("> {input}|"))
            .block(panel_block(title.to_string(), p, style))
            .style(Style::default().bg(p.bg).fg(p.fg)),
        popup,
    );
}
