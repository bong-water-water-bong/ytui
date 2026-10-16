mod glyphs;
mod player;
mod render;
mod term;
mod yt;

use anyhow::{bail, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use glyphs::{CH, CW};
use player::{Audio, Exit, Opts};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use render::{Cell, Img, Mode, Renderer};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;
use yt::{Stream, Video};

const HELP: &str = "\
ytui - YouTube in the terminal, rendered as colour ASCII / Unicode mosaics

USAGE:
  ytui [QUERY | URL]            search (or open a video / playlist / channel URL)
  ytui --snapshot SRC [--at SECONDS] [--cols N --rows N]
                                print one rendered frame (SRC = URL, id, or file)

OPTIONS:
  --mode M       ascii | classic | sextant | quadrant | halfblock | braille  (default ascii)
  --audio A      auto | pulse | alsa | none   (auto: pulse if available, muted over ssh)
  --fps N        render frame rate (default 30)
  --height N     max stream height to fetch (default 480)
  -n N           search results (default 25)

KEYS (browser):  / search   enter play   j/k move   m mode   q quit
KEYS (player):   space pause   ←/→ 10s   ↑/↓ 60s   0-9 jump   m mode   b ascii bg
                 [ ] global contrast   { } edge contrast   n/p next/prev   q back

ENV: YTUI_YTDLP (yt-dlp path), YTUI_FONT (monospace .ttf used for glyph shapes)
";

enum Msg {
    Results(Result<Vec<Video>>),
    Thumb(ThumbKey, Option<Vec<u8>>),
    Resolved(usize, Result<Stream>),
}

type ThumbKey = (String, u16, u16);

struct App {
    query: String,
    editing: bool,
    results: Vec<Video>,
    list: ListState,
    status: String,
    busy: bool,
    thumbs: HashMap<ThumbKey, Vec<u8>>,
    thumb_pending: HashSet<ThumbKey>,
    want_thumb: Option<ThumbKey>,
    renderer: Renderer,
    cells: Vec<Cell>,
    opts: Opts,
    max_h: u32,
    n: usize,
    tx: Sender<Msg>,
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut mode = Mode::Ascii;
    let mut audio = Audio::auto();
    let mut fps = 30;
    let mut max_h = 480;
    let mut n = 25;
    let mut snapshot: Option<String> = None;
    let mut at = 0.0;
    let (mut scols, mut srows) = (None, None);
    let mut query = Vec::new();
    while let Some(a) = args.next() {
        let mut val = |name: &str| args.next().ok_or_else(|| anyhow::anyhow!("{name} needs a value"));
        match a.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                return Ok(());
            }
            "--mode" => {
                let v = val("--mode")?;
                mode = Mode::parse(&v).ok_or_else(|| anyhow::anyhow!("unknown mode {v}"))?;
            }
            "--audio" => {
                audio = match val("--audio")?.as_str() {
                    "auto" => Audio::auto(),
                    "pulse" => Audio::Pulse,
                    "alsa" => Audio::Alsa,
                    "none" | "off" => Audio::None,
                    v => bail!("unknown audio output {v}"),
                }
            }
            "--fps" => fps = val("--fps")?.parse()?,
            "--height" => max_h = val("--height")?.parse()?,
            "-n" => n = val("-n")?.parse()?,
            "--snapshot" => snapshot = Some(val("--snapshot")?),
            "--at" => at = val("--at")?.parse()?,
            "--cols" => scols = Some(val("--cols")?.parse()?),
            "--rows" => srows = Some(val("--rows")?.parse()?),
            _ => query.push(a),
        }
    }
    let mut renderer = Renderer::new(mode);

    if let Some(src) = snapshot {
        let (tc, tr) = crossterm::terminal::size().map(|(c, r)| (c as usize, r as usize)).unwrap_or((100, 30));
        let (input, aspect) = if std::path::Path::new(&src).exists() {
            (src.clone(), 16.0 / 9.0)
        } else {
            let s = yt::resolve(&src, max_h)?;
            (s.video_url, s.aspect)
        };
        let (w, h, _, _) = player::fit(aspect, scols.unwrap_or(tc), srows.unwrap_or(tr.saturating_sub(1)));
        let px = player::grab_frame(&input, at, w * CW, h * CH).ok_or_else(|| anyhow::anyhow!("could not decode a frame"))?;
        let mut cells = Vec::new();
        renderer.render(&Img { w: w * CW, h: h * CH, px: &px }, w, h, &mut cells);
        print!("{}", term::to_ansi(&cells, w, h));
        return Ok(());
    }

    let (tx, rx) = channel();
    let mut app = App {
        query: query.join(" "),
        editing: query.is_empty(),
        results: Vec::new(),
        list: ListState::default(),
        status: format!("glyphs: {}", renderer.glyphs.source),
        busy: false,
        thumbs: HashMap::new(),
        thumb_pending: HashSet::new(),
        want_thumb: None,
        renderer,
        cells: Vec::new(),
        opts: Opts { audio, fps },
        max_h,
        n,
        tx,
    };
    if !app.query.is_empty() {
        start_search(&mut app);
    }
    let mut terminal = ratatui::init();
    let res = run(&mut terminal, &mut app, rx);
    ratatui::restore();
    res
}

fn start_search(app: &mut App) {
    let (q, n, tx) = (app.query.trim().to_string(), app.n, app.tx.clone());
    if q.is_empty() {
        return;
    }
    app.busy = true;
    app.status = format!("searching \"{q}\"…");
    std::thread::spawn(move || {
        let _ = tx.send(Msg::Results(yt::search(&q, n)));
    });
}

fn start_play(app: &mut App) {
    let Some(i) = app.list.selected() else { return };
    let Some(v) = app.results.get(i) else { return };
    app.busy = true;
    app.status = format!("opening \"{}\"…", v.title);
    let (url, h, tx) = (v.url.clone(), app.max_h, app.tx.clone());
    std::thread::spawn(move || {
        let _ = tx.send(Msg::Resolved(i, yt::resolve(&url, h)));
    });
}

fn run(terminal: &mut DefaultTerminal, app: &mut App, rx: Receiver<Msg>) -> Result<()> {
    loop {
        terminal.draw(|f| ui(f, app))?;
        if let Some(key) = app.want_thumb.take() {
            if app.thumb_pending.insert(key.clone()) {
                let tx = app.tx.clone();
                std::thread::spawn(move || {
                    let url = format!("https://i.ytimg.com/vi/{}/mqdefault.jpg", key.0);
                    let px = player::grab_frame(&url, 0.0, key.1 as usize * CW, key.2 as usize * CH);
                    let _ = tx.send(Msg::Thumb(key, px));
                });
            }
        }
        while let Ok(m) = rx.try_recv() {
            match m {
                Msg::Results(Ok(v)) => {
                    app.busy = false;
                    app.status = format!("{} results", v.len());
                    app.results = v;
                    app.list.select(Some(0));
                }
                Msg::Results(Err(e)) | Msg::Resolved(_, Err(e)) => {
                    app.busy = false;
                    app.status = format!("error: {e}");
                }
                Msg::Thumb(k, Some(px)) => {
                    app.thumbs.insert(k, px);
                }
                Msg::Thumb(_, None) => {}
                Msg::Resolved(i, Ok(stream)) => {
                    app.busy = false;
                    app.status.clear();
                    let mut cur = (i, stream);
                    loop {
                        let exit = player::play(&cur.1, &mut app.renderer, app.opts);
                        terminal.clear()?;
                        let next = match exit {
                            Err(e) => {
                                app.status = format!("playback error: {e}");
                                None
                            }
                            Ok(Exit::Quit) => return Ok(()),
                            Ok(Exit::Back) => None,
                            Ok(Exit::Ended) | Ok(Exit::Next) => Some(cur.0 + 1),
                            Ok(Exit::Prev) => Some(cur.0.saturating_sub(1)),
                        };
                        let Some(j) = next.filter(|&j| j < app.results.len()) else { break };
                        app.list.select(Some(j));
                        app.status = format!("opening \"{}\"…", app.results[j].title);
                        terminal.draw(|f| ui(f, app))?;
                        match yt::resolve(&app.results[j].url, app.max_h) {
                            Ok(s) => cur = (j, s),
                            Err(e) => {
                                app.status = format!("error: {e}");
                                break;
                            }
                        }
                        app.status.clear();
                    }
                }
            }
        }
        if !event::poll(Duration::from_millis(60))? {
            continue;
        }
        let Event::Key(k) = event::read()? else { continue };
        if k.kind == KeyEventKind::Release {
            continue;
        }
        if k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(());
        }
        if app.editing {
            match k.code {
                KeyCode::Enter => {
                    app.editing = false;
                    start_search(app);
                }
                KeyCode::Esc => app.editing = false,
                KeyCode::Backspace => {
                    app.query.pop();
                }
                KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => app.query.clear(),
                KeyCode::Char(c) => app.query.push(c),
                _ => {}
            }
            continue;
        }
        let len = app.results.len();
        let sel = app.list.selected().unwrap_or(0);
        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
            KeyCode::Char('/') | KeyCode::Char('s') | KeyCode::Char('i') => app.editing = true,
            KeyCode::Down | KeyCode::Char('j') if len > 0 => app.list.select(Some((sel + 1).min(len - 1))),
            KeyCode::Up | KeyCode::Char('k') if len > 0 => app.list.select(Some(sel.saturating_sub(1))),
            KeyCode::PageDown if len > 0 => app.list.select(Some((sel + 10).min(len - 1))),
            KeyCode::PageUp => app.list.select(Some(sel.saturating_sub(10))),
            KeyCode::Char('g') | KeyCode::Home => app.list.select(Some(0)),
            KeyCode::Char('G') | KeyCode::End if len > 0 => app.list.select(Some(len - 1)),
            KeyCode::Char('m') => app.renderer.mode = app.renderer.mode.next(),
            KeyCode::Enter if !app.busy => start_play(app),
            _ => {}
        }
    }
}

fn ui(f: &mut Frame, app: &mut App) {
    let [top, body, bar] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(5), Constraint::Length(1)]).areas(f.area());
    let accent = Color::Rgb(255, 60, 60);

    let sb = Block::bordered()
        .title(if app.editing { " Search — enter to run, esc to browse " } else { " Search — / to edit " })
        .border_style(Style::default().fg(if app.editing { accent } else { Color::DarkGray }));
    f.render_widget(Paragraph::new(app.query.as_str()).block(sb), top);
    if app.editing {
        let x = top.x + 1 + app.query.chars().count().min(top.width as usize - 3) as u16;
        f.set_cursor_position((x, top.y + 1));
    }

    let [left, right] = Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(body);
    let items: Vec<ListItem> = app
        .results
        .iter()
        .map(|v| {
            let mut meta = vec![Span::styled(format!("  {}", v.channel), Style::default().fg(Color::Gray))];
            if v.live {
                meta.push(Span::styled("  ● LIVE", Style::default().fg(accent)));
            } else if let Some(d) = v.duration {
                meta.push(Span::styled(format!("  {}", yt::fmt_time(d)), Style::default().fg(Color::DarkGray)));
            }
            if let Some(n) = v.views {
                meta.push(Span::styled(format!("  {} views", yt::fmt_views(n)), Style::default().fg(Color::DarkGray)));
            }
            ListItem::new(Text::from(vec![Line::from(v.title.clone()), Line::from(meta)]))
        })
        .collect();
    let list = List::new(items)
        .block(Block::bordered().title(" Results ").border_style(Style::default().fg(Color::DarkGray)))
        .highlight_style(Style::default().bg(Color::Rgb(60, 10, 10)).add_modifier(Modifier::BOLD))
        .highlight_symbol("▌");
    f.render_stateful_widget(list, left, &mut app.list);

    let pb = Block::bordered()
        .title(format!(" Preview · {} ", app.renderer.mode.name()))
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = pb.inner(right);
    f.render_widget(pb, right);
    if let Some(v) = app.list.selected().and_then(|i| app.results.get(i)).cloned() {
        let (w, h, x0, _) = player::fit(16.0 / 9.0, inner.width as usize, (inner.height as usize).saturating_sub(5).max(1));
        let key: ThumbKey = (v.id.clone(), w as u16, h as u16);
        if let Some(px) = app.thumbs.get(&key) {
            app.renderer.render(&Img { w: w * CW, h: h * CH, px }, w, h, &mut app.cells);
            let buf = f.buffer_mut();
            for y in 0..h {
                for x in 0..w {
                    let c = app.cells[y * w + x];
                    let cell = &mut buf[(inner.x + (x0 + x) as u16, inner.y + y as u16)];
                    cell.set_char(c.ch)
                        .set_fg(Color::Rgb(c.fg[0], c.fg[1], c.fg[2]))
                        .set_bg(Color::Rgb(c.bg[0], c.bg[1], c.bg[2]));
                }
            }
        } else {
            app.want_thumb = Some(key);
        }
        let mut info = vec![Line::from(Span::styled(v.title.clone(), Style::default().add_modifier(Modifier::BOLD)))];
        let mut meta = vec![v.channel.clone()];
        if let Some(d) = v.duration {
            meta.push(yt::fmt_time(d));
        }
        if let Some(n) = v.views {
            meta.push(format!("{} views", yt::fmt_views(n)));
        }
        info.push(Line::from(Span::styled(meta.join(" · "), Style::default().fg(Color::Gray))));
        let ty = inner.y + h as u16 + 1;
        if ty < inner.y + inner.height {
            let area = Rect { x: inner.x, y: ty, width: inner.width, height: inner.y + inner.height - ty };
            f.render_widget(Paragraph::new(info).wrap(Wrap { trim: true }), area);
        }
    }

    let keys = " / search  ↵ play  j/k move  m mode  q quit ";
    let status = if app.busy { format!(" ⟳ {}", app.status) } else { format!(" {}", app.status) };
    let pad = (bar.width as usize).saturating_sub(status.chars().count() + keys.chars().count());
    f.render_widget(
        Paragraph::new(format!("{status}{}{keys}", " ".repeat(pad)))
            .style(Style::default().fg(Color::Rgb(230, 230, 230)).bg(Color::Rgb(40, 0, 0))),
        bar,
    );
}
