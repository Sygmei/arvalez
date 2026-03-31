use std::{
    process::ExitStatus,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

use anyhow::{Context, Result};
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, List, ListItem, Paragraph},
};

use crate::corpus::{
    CorpusProgressSnapshot, CorpusProgressState, CORPUS_HEARTBEAT_INTERVAL,
};

pub(crate) const CORPUS_UI_TICK_INTERVAL: Duration = Duration::from_millis(200);
pub(crate) const CORPUS_UI_ACTIVE_SAMPLE_LIMIT: usize = 8;

pub(crate) enum CorpusMonitor {
    Heartbeat(thread::JoinHandle<()>),
    Ui(thread::JoinHandle<()>),
}

impl CorpusMonitor {
    pub(crate) fn join(self) {
        match self {
            Self::Heartbeat(handle) | Self::Ui(handle) => {
                let _ = handle.join();
            }
        }
    }
}

pub(crate) fn spawn_corpus_heartbeat(
    completed: Arc<AtomicUsize>,
    progress_state: Arc<Mutex<CorpusProgressState>>,
    done: Arc<AtomicBool>,
    total_specs: usize,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let frames = ["|", "/", "-", "\\"];
        let mut frame_index = 0usize;

        loop {
            thread::sleep(CORPUS_HEARTBEAT_INTERVAL);
            if done.load(Ordering::Relaxed) {
                break;
            }

            let completed_count = completed.load(Ordering::Relaxed);
            let snapshot = {
                let progress = progress_state
                    .lock()
                    .expect("corpus progress state should not be poisoned");
                build_corpus_progress_snapshot(&progress)
            };
            let active_count = snapshot.active_specs.len();
            let sample = snapshot
                .active_specs
                .into_iter()
                .take(3)
                .collect::<Vec<_>>();

            if active_count == 0 || completed_count >= total_specs {
                continue;
            }

            let remaining = total_specs.saturating_sub(completed_count);
            let suffix = if active_count > sample.len() {
                format!(" | ... +{}", active_count - sample.len())
            } else {
                String::new()
            };
            let sample_text = if sample.is_empty() {
                "working...".to_owned()
            } else {
                format!("{}{}", sample.join(" | "), suffix)
            };

            eprintln!(
                "[heartbeat {}] active {} worker(s), completed {}/{} (remaining {}): {}",
                frames[frame_index % frames.len()],
                active_count,
                completed_count,
                total_specs,
                remaining,
                sample_text
            );
            frame_index += 1;
        }
    })
}

pub(crate) fn spawn_corpus_ui(
    completed: Arc<AtomicUsize>,
    progress_state: Arc<Mutex<CorpusProgressState>>,
    done: Arc<AtomicBool>,
    ui_active: Arc<AtomicBool>,
    total_specs: usize,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        if let Err(error) =
            run_corpus_ui(completed, progress_state, done, ui_active, total_specs)
        {
            eprintln!("failed to render corpus UI: {error:#}");
        }
    })
}

pub(crate) fn run_corpus_ui(
    completed: Arc<AtomicUsize>,
    progress_state: Arc<Mutex<CorpusProgressState>>,
    done: Arc<AtomicBool>,
    ui_active: Arc<AtomicBool>,
    total_specs: usize,
) -> Result<()> {
    enable_raw_mode().context("failed to enable raw mode for corpus UI")?;
    let mut stdout = std::io::stderr();
    execute!(stdout, EnterAlternateScreen).context("failed to enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("failed to initialize corpus UI")?;
    let spinner_frames = ["|", "/", "-", "\\"];
    let mut spinner_index = 0usize;

    loop {
        let completed_count = completed.load(Ordering::Relaxed);
        let snapshot = {
            let progress = progress_state
                .lock()
                .expect("corpus progress state should not be poisoned");
            build_corpus_progress_snapshot(&progress)
        };

        terminal
            .draw(|frame| {
                let area = frame.area();
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3),
                        Constraint::Length(7),
                        Constraint::Min(8),
                        Constraint::Length(6),
                    ])
                    .split(area);

                let progress_ratio = if total_specs == 0 {
                    0.0
                } else {
                    completed_count as f64 / total_specs as f64
                };
                let progress_label = format!(
                    "{} {}/{} ({:.1}%)",
                    spinner_frames[spinner_index % spinner_frames.len()],
                    completed_count,
                    total_specs,
                    progress_ratio * 100.0
                );
                let gauge = Gauge::default()
                    .block(
                        Block::default()
                            .title("Corpus Progress")
                            .borders(Borders::ALL),
                    )
                    .gauge_style(
                        Style::default()
                            .fg(Color::Cyan)
                            .bg(Color::Black)
                            .add_modifier(Modifier::BOLD),
                    )
                    .label(progress_label)
                    .ratio(progress_ratio);
                frame.render_widget(gauge, chunks[0]);

                let active_count = snapshot.active_specs.len();
                let remaining = total_specs.saturating_sub(completed_count);
                let stats = Paragraph::new(vec![
                    Line::from(vec![
                        Span::styled(
                            format!("Passed: {}", snapshot.passed_specs),
                            Style::default()
                                .fg(Color::Green)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::raw("    "),
                        Span::styled(
                            format!("Failed: {}", snapshot.failed_specs),
                            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    Line::from(format!("Active workers: {active_count}")),
                    Line::from(format!("Remaining specs: {remaining}")),
                    Line::from(
                        "Press q to hide the UI and continue with plain progress.",
                    ),
                ])
                .block(Block::default().title("Run Stats").borders(Borders::ALL));
                frame.render_widget(stats, chunks[1]);

                let middle = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
                    .split(chunks[2]);

                let active_items = if snapshot.active_specs.is_empty() {
                    vec![ListItem::new("No active specs right now")]
                } else {
                    snapshot
                        .active_specs
                        .iter()
                        .take(CORPUS_UI_ACTIVE_SAMPLE_LIMIT)
                        .map(|spec| ListItem::new(spec.clone()))
                        .collect::<Vec<_>>()
                };
                let active_list = List::new(active_items)
                    .block(Block::default().title("Active Specs").borders(Borders::ALL));
                frame.render_widget(active_list, middle[0]);

                let recent_items = if snapshot.recent_completed.is_empty() {
                    vec![ListItem::new("No completed specs yet")]
                } else {
                    snapshot
                        .recent_completed
                        .iter()
                        .map(|entry| {
                            let style = if entry.status == "passed" {
                                Style::default().fg(Color::Green)
                            } else {
                                Style::default().fg(Color::Red)
                            };
                            ListItem::new(Line::from(vec![
                                Span::styled(format!("[{}] ", entry.status), style),
                                Span::raw(entry.spec.clone()),
                            ]))
                        })
                        .collect::<Vec<_>>()
                };
                let recent_list = List::new(recent_items).block(
                    Block::default()
                        .title("Recent Completions")
                        .borders(Borders::ALL),
                );
                frame.render_widget(recent_list, middle[1]);

                let footer = Paragraph::new(vec![
                    Line::from("The corpus run continues even if one spec crashes."),
                    Line::from(
                        "Close the UI with q if you want the plain line-based progress instead.",
                    ),
                ])
                .block(Block::default().title("Notes").borders(Borders::ALL));
                frame.render_widget(footer, chunks[3]);
            })
            .context("failed to draw corpus UI")?;

        spinner_index += 1;

        if done.load(Ordering::Relaxed) {
            break;
        }

        if event::poll(CORPUS_UI_TICK_INTERVAL).context("failed while polling corpus UI")? {
            if let Event::Key(key) =
                event::read().context("failed while reading corpus UI input")?
            {
                if key.code == KeyCode::Char('q') {
                    ui_active.store(false, Ordering::Relaxed);
                    break;
                }
            }
        }
    }

    disable_raw_mode().context("failed to disable raw mode for corpus UI")?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)
        .context("failed to leave alternate screen")?;
    terminal.show_cursor().context("failed to restore cursor")?;
    Ok(())
}

pub(crate) fn build_corpus_progress_snapshot(
    progress: &CorpusProgressState,
) -> CorpusProgressSnapshot {
    CorpusProgressSnapshot {
        active_specs: progress.active_specs.iter().cloned().collect(),
        recent_completed: progress.recent_completed.iter().cloned().collect(),
        passed_specs: progress.passed_specs,
        failed_specs: progress.failed_specs,
    }
}

#[cfg(unix)]
pub(crate) fn exit_status_signal(status: &ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status.signal()
}

#[cfg(not(unix))]
pub(crate) fn exit_status_signal(_status: &ExitStatus) -> Option<i32> {
    None
}
