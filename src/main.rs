use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use notify_rust::{Notification, Urgency};
use ratatui::{prelude::*, widgets::*};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io,
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Parser)]
#[command(author, version, about = "🍅 rtimer - A Beautiful Terminal Pomodoro Timer")]
struct Args {
    /// Work duration in minutes
    #[arg(short, long, default_value_t = 25)]
    work: u64,
    
    /// Rest duration in minutes
    #[arg(short, long, default_value_t = 5)]
    rest: u64,
    
    /// Long break duration in minutes
    #[arg(short, long, default_value_t = 15)]
    long_break: u64,
    
    /// Sessions before long break
    #[arg(short, long, default_value_t = 4)]
    sessions: u32,
}

#[derive(Serialize, Deserialize, Clone)]
struct Statistics {
    total_sessions: u32,
    total_work_time: u64,
    total_break_time: u64,
    sessions_today: u32,
    last_session_date: String,
}

impl Default for Statistics {
    fn default() -> Self {
        Statistics {
            total_sessions: 0,
            total_work_time: 0,
            total_break_time: 0,
            sessions_today: 0,
            last_session_date: chrono::Local::now().format("%Y-%m-%d").to_string(),
        }
    }
}

enum Phase {
    Work,
    ShortBreak,
    LongBreak,
}

struct AppState {
    time_remaining: Duration,
    paused: bool,
    session_count: u32,
    phase: Phase,
    stats: Statistics,
    work_duration: Duration,
    rest_duration: Duration,
    long_break_duration: Duration,
    sessions_before_long_break: u32,
    show_help: bool,
    show_stats: bool,
    animation_frame: u8,
}

impl AppState {
    fn phase_name(&self) -> &str {
        match self.phase {
            Phase::Work => "🎯 FOCUS TIME",
            Phase::ShortBreak => "☕ SHORT BREAK",
            Phase::LongBreak => "🌴 LONG BREAK",
        }
    }

    fn phase_color(&self) -> Color {
        match self.phase {
            Phase::Work => Color::Rgb(255, 107, 107),
            Phase::ShortBreak => Color::Rgb(126, 214, 223),
            Phase::LongBreak => Color::Rgb(168, 218, 181),
        }
    }

    fn next_phase(&mut self) {
        match self.phase {
            Phase::Work => {
                self.stats.total_work_time += self.work_duration.as_secs() / 60;
                self.stats.total_sessions += 1;
                self.stats.sessions_today += 1;
                
                if self.session_count % self.sessions_before_long_break == 0 {
                    self.phase = Phase::LongBreak;
                    self.time_remaining = self.long_break_duration;
                    send_notification("🌴 Long Break Time!", "You've earned a longer rest. Great work!");
                } else {
                    self.phase = Phase::ShortBreak;
                    self.time_remaining = self.rest_duration;
                    send_notification("☕ Break Time!", "Time to rest and recharge!");
                }
            }
            Phase::ShortBreak | Phase::LongBreak => {
                if matches!(self.phase, Phase::ShortBreak) {
                    self.stats.total_break_time += self.rest_duration.as_secs() / 60;
                } else {
                    self.stats.total_break_time += self.long_break_duration.as_secs() / 60;
                }
                
                self.phase = Phase::Work;
                self.time_remaining = self.work_duration;
                self.session_count += 1;
                send_notification("🎯 Focus Time!", "Let's get back to work!");
            }
        }
    }

    fn reset_timer(&mut self) {
        self.time_remaining = match self.phase {
            Phase::Work => self.work_duration,
            Phase::ShortBreak => self.rest_duration,
            Phase::LongBreak => self.long_break_duration,
        };
        self.paused = false;
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Load or create statistics
    let stats_path = get_stats_path();
    let mut stats = load_stats(&stats_path);
    
    // Reset daily stats if new day
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    if stats.last_session_date != today {
        stats.sessions_today = 0;
        stats.last_session_date = today;
    }

    let mut app_state = AppState {
        time_remaining: Duration::from_secs(args.work * 60),
        paused: false,
        session_count: 1,
        phase: Phase::Work,
        stats,
        work_duration: Duration::from_secs(args.work * 60),
        rest_duration: Duration::from_secs(args.rest * 60),
        long_break_duration: Duration::from_secs(args.long_break * 60),
        sessions_before_long_break: args.sessions,
        show_help: false,
        show_stats: false,
        animation_frame: 0,
    };

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut last_tick = Instant::now();
    let tick_rate = Duration::from_millis(50);

    // Main loop
    let result = run_app(&mut terminal, &mut app_state, tick_rate, &mut last_tick);

    // Cleanup
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    // Save statistics
    save_stats(&stats_path, &app_state.stats)?;

    if let Err(err) = result {
        eprintln!("Error: {:?}", err);
    }

    Ok(())
}

fn run_app<B: Backend>(
    terminal: &mut Terminal<B>,
    app_state: &mut AppState,
    tick_rate: Duration,
    last_tick: &mut Instant,
) -> io::Result<()> {
    loop {
        terminal.draw(|f| ui(f, app_state))?;

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        return Ok(())
                    }
                    KeyCode::Char(' ') => app_state.paused = !app_state.paused,
                    KeyCode::Char('r') => app_state.reset_timer(),
                    KeyCode::Char('h') | KeyCode::Char('?') => {
                        app_state.show_help = !app_state.show_help
                    }
                    KeyCode::Char('s') => app_state.show_stats = !app_state.show_stats,
                    KeyCode::Char('n') => {
                        app_state.next_phase();
                    }
                    _ => {}
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            on_tick(app_state);
            *last_tick = Instant::now();
        }
    }
}

fn on_tick(app_state: &mut AppState) {
    if !app_state.paused {
        let elapsed = Duration::from_millis(50);
        app_state.time_remaining = app_state.time_remaining.saturating_sub(elapsed);

        if app_state.time_remaining == Duration::ZERO {
            app_state.next_phase();
        }
    }
    
    // Update animation frame
    app_state.animation_frame = (app_state.animation_frame + 1) % 20;
}

fn ui(f: &mut Frame, app_state: &AppState) {
    let size = f.size();

    // Create main layout
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(size);

    // Header
    render_header(f, chunks[0], app_state);

    // Main content
    if app_state.show_help {
        render_help(f, chunks[1]);
    } else if app_state.show_stats {
        render_stats(f, chunks[1], app_state);
    } else {
        render_timer(f, chunks[1], app_state);
    }

    // Footer
    render_footer(f, chunks[2], app_state);
}

fn render_header(f: &mut Frame, area: Rect, app_state: &AppState) {
    let title = format!("🍅 rtimer v0.2.0 │ Session #{}", app_state.session_count);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(Style::default().fg(app_state.phase_color()));

    let text = Paragraph::new(title)
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD))
        .block(block);

    f.render_widget(text, area);
}

fn render_timer(f: &mut Frame, area: Rect, app_state: &AppState) {
    let color = app_state.phase_color();
    
    // Calculate progress
    let total_duration = match app_state.phase {
        Phase::Work => app_state.work_duration,
        Phase::ShortBreak => app_state.rest_duration,
        Phase::LongBreak => app_state.long_break_duration,
    };
    
    let progress = 1.0 - (app_state.time_remaining.as_secs_f64() / total_duration.as_secs_f64());

    // Create centered layout
    let timer_area = centered_rect(70, 60, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(color))
        .style(Style::default().bg(Color::Black));

    // Split into sections
    let inner = block.inner(timer_area);
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(7),
            Constraint::Length(2),
            Constraint::Length(3),
            Constraint::Min(1),
        ])
        .split(inner);

    f.render_widget(block, timer_area);

    // Phase name
    let phase_text = Paragraph::new(app_state.phase_name())
        .style(
            Style::default()
                .fg(color)
                .add_modifier(Modifier::BOLD),
        )
        .alignment(Alignment::Center);
    f.render_widget(phase_text, sections[0]);

    // Big timer display
    let mins = app_state.time_remaining.as_secs() / 60;
    let secs = app_state.time_remaining.as_secs() % 60;
    let time_text = format!("{:02}:{:02}", mins, secs);
    
    let timer_display = Paragraph::new(time_text)
        .style(
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .alignment(Alignment::Center);
    f.render_widget(timer_display, sections[1]);

    // Status indicator
    let status_icon = if app_state.paused {
        "⏸  PAUSED"
    } else {
        let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let frame_idx = (app_state.animation_frame / 2) as usize % frames.len();
        if matches!(app_state.phase, Phase::Work) {
            frames[frame_idx]
        } else {
            "▶ RUNNING"
        }
    };
    
    let status = Paragraph::new(status_icon)
        .style(Style::default().fg(if app_state.paused { Color::Yellow } else { color }))
        .alignment(Alignment::Center);
    f.render_widget(status, sections[2]);

    // Progress bar
    let progress_bar = Gauge::default()
        .block(Block::default())
        .gauge_style(Style::default().fg(color).bg(Color::DarkGray))
        .ratio(progress);
    f.render_widget(progress_bar, sections[3]);

    // Session progress
    let sessions_until_long = app_state.sessions_before_long_break
        - (app_state.session_count % app_state.sessions_before_long_break);
    let progress_text = format!(
        "Sessions until long break: {} │ Today: {}",
        sessions_until_long, app_state.stats.sessions_today
    );
    let progress_label = Paragraph::new(progress_text)
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center);
    f.render_widget(progress_label, sections[4]);
}

fn render_help(f: &mut Frame, area: Rect) {
    let help_area = centered_rect(80, 70, area);

    let help_text = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("Keyboard Shortcuts", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Space  ", Style::default().fg(Color::Yellow)),
            Span::raw("  Pause/Resume timer"),
        ]),
        Line::from(vec![
            Span::styled("  r      ", Style::default().fg(Color::Yellow)),
            Span::raw("  Reset current phase"),
        ]),
        Line::from(vec![
            Span::styled("  n      ", Style::default().fg(Color::Yellow)),
            Span::raw("  Skip to next phase"),
        ]),
        Line::from(vec![
            Span::styled("  s      ", Style::default().fg(Color::Yellow)),
            Span::raw("  Show statistics"),
        ]),
        Line::from(vec![
            Span::styled("  h / ?  ", Style::default().fg(Color::Yellow)),
            Span::raw("  Toggle this help"),
        ]),
        Line::from(vec![
            Span::styled("  q / Esc", Style::default().fg(Color::Yellow)),
            Span::raw("  Quit application"),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Pomodoro Technique", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(""),
        Line::from("  1. Work for 25 minutes (focus time)"),
        Line::from("  2. Take a 5 minute break"),
        Line::from("  3. After 4 sessions, take a longer 15 minute break"),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Press 'h' to close this help", Style::default().fg(Color::Green).add_modifier(Modifier::ITALIC)),
        ]),
    ];

    let help = Paragraph::new(help_text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(" Help ")
                .title_alignment(Alignment::Center)
                .style(Style::default().fg(Color::Cyan)),
        )
        .alignment(Alignment::Left);

    f.render_widget(help, help_area);
}

fn render_stats(f: &mut Frame, area: Rect, app_state: &AppState) {
    let stats_area = centered_rect(70, 60, area);

    let hours_worked = app_state.stats.total_work_time / 60;
    let mins_worked = app_state.stats.total_work_time % 60;
    let hours_break = app_state.stats.total_break_time / 60;
    let mins_break = app_state.stats.total_break_time % 60;

    let stats_text = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("📊 Your Statistics", Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(""),
        Line::from(""),
        Line::from(vec![
            Span::raw("  Total Sessions Completed:  "),
            Span::styled(
                format!("{}", app_state.stats.total_sessions),
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::raw("  Sessions Today:  "),
            Span::styled(
                format!("{}", app_state.stats.sessions_today),
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::raw("  Total Work Time:  "),
            Span::styled(
                format!("{}h {}m", hours_worked, mins_worked),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::raw("  Total Break Time:  "),
            Span::styled(
                format!("{}h {}m", hours_break, mins_break),
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Press 's' to close statistics", Style::default().fg(Color::Green).add_modifier(Modifier::ITALIC)),
        ]),
    ];

    let stats_widget = Paragraph::new(stats_text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(" Statistics ")
                .title_alignment(Alignment::Center)
                .style(Style::default().fg(Color::Magenta)),
        )
        .alignment(Alignment::Left);

    f.render_widget(stats_widget, stats_area);
}

fn render_footer(f: &mut Frame, area: Rect, app_state: &AppState) {
    let footer_text = if app_state.show_help || app_state.show_stats {
        "Press h/s to return to timer"
    } else {
        "Space: Pause/Resume │ r: Reset │ n: Skip │ s: Stats │ h: Help │ q: Quit"
    };

    let footer = Paragraph::new(footer_text)
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded),
        );

    f.render_widget(footer, area);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn send_notification(title: &str, body: &str) {
    let _ = Notification::new()
        .summary(title)
        .body(body)
        .appname("rtimer")
        .icon("alarm-clock")
        .urgency(Urgency::Critical)
        .show();
}

fn get_stats_path() -> PathBuf {
    let mut path = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    path.push("rtimer");
    fs::create_dir_all(&path).ok();
    path.push("stats.json");
    path
}

fn load_stats(path: &PathBuf) -> Statistics {
    if let Ok(contents) = fs::read_to_string(path) {
        serde_json::from_str(&contents).unwrap_or_default()
    } else {
        Statistics::default()
    }
}

fn save_stats(path: &PathBuf, stats: &Statistics) -> io::Result<()> {
    let json = serde_json::to_string_pretty(stats)?;
    fs::write(path, json)?;
    Ok(())
}
