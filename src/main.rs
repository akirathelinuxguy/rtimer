use clap::Parser;
use chrono::Datelike;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use notify_rust::{Notification, Urgency};
use ratatui::{prelude::*, widgets::*};
use serde::{Deserialize, Serialize};
use std::{fs, io, path::PathBuf, time::{Duration, Instant}};

// ============================================================================
// Type Aliases & Constants
// ============================================================================

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const TICK_RATE: Duration = Duration::from_millis(50);
const AUTO_SAVE_INTERVAL: Duration = Duration::from_secs(5);
const MAX_HISTORY: usize = 100;
const DAILY_FMT: &str = "%Y-%m-%d";

// ============================================================================
// CLI Arguments
// ============================================================================

#[derive(Parser, Clone)]
#[command(author, version, about = "🍅 rtimer - A Beautiful Terminal Pomodoro Timer")]
struct Args {
    #[arg(short, long, value_parser = parse_duration)]
    work: Option<f64>,
    #[arg(short, long, value_parser = parse_duration)]
    rest: Option<f64>,
    #[arg(short, long, value_parser = parse_duration)]
    long_break: Option<f64>,
    #[arg(short, long)]
    sessions: Option<u32>,
    #[arg(short = 't', long)]
    theme: Option<String>,
    #[arg(long)]
    no_sound: bool,
    #[arg(long)]
    resume: bool,
}

fn parse_duration(s: &str) -> std::result::Result<f64, String> {
    let s = s.trim().to_lowercase();
    let mut total = 0.0;
    let mut num = String::new();
    
    for c in s.chars() {
        match c {
            '0'..='9' | '.' => num.push(c),
            'h' => { total += num.parse::<f64>().map_err(|_| "Invalid hours")? * 60.0; num.clear(); }
            'm' => { total += num.parse::<f64>().map_err(|_| "Invalid minutes")?; num.clear(); }
            's' => { total += num.parse::<f64>().map_err(|_| "Invalid seconds")? / 60.0; num.clear(); }
            _ => return Err("Invalid format".into()),
        }
    }
    
    if total > 0.0 { Ok(total) } else { Err("Duration must be > 0".into()) }
}

// ============================================================================
// Data Models
// ============================================================================

#[derive(Serialize, Deserialize, Clone)]
struct Config {
    work_duration: f64,
    rest_duration: f64,
    long_break_duration: f64,
    sessions_before_long_break: u32,
    sound_enabled: bool,
    theme: String,
    auto_start_next: bool,
    extended_break_reminder_hours: f64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            work_duration: 25.0,
            rest_duration: 5.0,
            long_break_duration: 15.0,
            sessions_before_long_break: 4,
            sound_enabled: true,
            theme: "default".into(),
            auto_start_next: true,
            extended_break_reminder_hours: 2.0,
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
struct Note {
    timestamp: String,
    content: String,
    phase: String,
}

#[derive(Serialize, Deserialize, Clone)]
struct SessionRecord {
    timestamp: String,
    phase_type: String,
    duration: u64,
    completed: bool,
}

#[derive(Serialize, Deserialize, Clone)]
struct Statistics {
    total_sessions: u32,
    total_work_time: u64,
    total_break_time: u64,
    sessions_today: u32,
    last_session_date: String,
    session_history: Vec<SessionRecord>,
    weekly_sessions: Vec<u32>,
    notes: Vec<Note>,
}

impl Default for Statistics {
    fn default() -> Self {
        Self {
            total_sessions: 0,
            total_work_time: 0,
            total_break_time: 0,
            sessions_today: 0,
            last_session_date: chrono::Local::now().format(DAILY_FMT).to_string(),
            session_history: Vec::new(),
            weekly_sessions: vec![0; 7],
            notes: Vec::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Copy)]
enum Phase {
    Work,
    ShortBreak,
    LongBreak,
}

impl Phase {
    fn name(&self) -> &str {
        match self {
            Self::Work => "🎯 FOCUS TIME",
            Self::ShortBreak => "☕ SHORT BREAK",
            Self::LongBreak => "🌴 LONG BREAK",
        }
    }
    
    fn to_str(&self) -> &str {
        match self {
            Self::Work => "work",
            Self::ShortBreak => "short_break",
            Self::LongBreak => "long_break",
        }
    }
    
    fn from_str(s: &str) -> Self {
        match s {
            "short_break" => Self::ShortBreak,
            "long_break" => Self::LongBreak,
            _ => Self::Work,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct TimerState {
    time_remaining_secs: u64,
    phase: String,
    session_count: u32,
    paused: bool,
}

#[derive(Clone, Copy)]
struct Theme {
    work_color: Color,
    short_break_color: Color,
    long_break_color: Color,
    border_color: Color,
    accent_color: Color,
}

#[derive(PartialEq, Clone, Copy)]
enum View {
    Timer,
    Help,
    StatsSummary,
    StatsDetailed,
    StatsHistory,
    Settings,
    Notes,
}

#[derive(PartialEq, Clone, Copy)]
enum SettingsField {
    WorkDuration,
    RestDuration,
    LongBreakDuration,
    SessionsBeforeLongBreak,
    Theme,
    SoundEnabled,
    AutoStartNext,
    ExtendedBreakReminder,
}

impl SettingsField {
    fn next(self) -> Self {
        match self {
            Self::WorkDuration => Self::RestDuration,
            Self::RestDuration => Self::LongBreakDuration,
            Self::LongBreakDuration => Self::SessionsBeforeLongBreak,
            Self::SessionsBeforeLongBreak => Self::Theme,
            Self::Theme => Self::SoundEnabled,
            Self::SoundEnabled => Self::AutoStartNext,
            Self::AutoStartNext => Self::ExtendedBreakReminder,
            Self::ExtendedBreakReminder => Self::WorkDuration,
        }
    }
    
    fn prev(self) -> Self {
        match self {
            Self::WorkDuration => Self::ExtendedBreakReminder,
            Self::RestDuration => Self::WorkDuration,
            Self::LongBreakDuration => Self::RestDuration,
            Self::SessionsBeforeLongBreak => Self::LongBreakDuration,
            Self::Theme => Self::SessionsBeforeLongBreak,
            Self::SoundEnabled => Self::Theme,
            Self::AutoStartNext => Self::SoundEnabled,
            Self::ExtendedBreakReminder => Self::AutoStartNext,
        }
    }
}

#[derive(PartialEq, Clone, Copy)]
enum NotesMode {
    Viewing,
    Adding,
    Editing,
    ConfirmingDelete,
}

// ============================================================================
// Application State
// ============================================================================

struct AppState {
    time_remaining: Duration,
    paused: bool,
    session_count: u32,
    phase: Phase,
    work_duration: Duration,
    rest_duration: Duration,
    long_break_duration: Duration,
    sessions_before_long_break: u32,
    stats: Statistics,
    current_view: View,
    theme: Theme,
    theme_name: String,
    sound_enabled: bool,
    animation_frame: u8,
    minimized: bool,
    settings_field: SettingsField,
    settings_editing: bool,
    settings_input: String,
    notes_mode: NotesMode,
    notes_input: String,
    selected_note_index: Option<usize>,
    needs_save: bool,
    last_save: Instant,
    auto_start_next: bool,
    extended_break_hours: f64,
    last_break_check: Instant,
    work_time_since_break: Duration,
}

impl AppState {
    fn new(config: Config, stats: Statistics, saved_state: Option<TimerState>) -> Self {
        let theme = get_theme(&config.theme);
        let work = Duration::from_secs_f64(config.work_duration * 60.0);
        let rest = Duration::from_secs_f64(config.rest_duration * 60.0);
        let long = Duration::from_secs_f64(config.long_break_duration * 60.0);
        
        let (time_remaining, paused, session_count, phase) = if let Some(saved) = saved_state {
            (
                Duration::from_secs(saved.time_remaining_secs),
                saved.paused,
                saved.session_count,
                Phase::from_str(&saved.phase),
            )
        } else {
            (work, false, 1, Phase::Work)
        };
        
        let selected_note_index = if !stats.notes.is_empty() {
            Some(stats.notes.len() - 1)
        } else {
            None
        };
        
        Self {
            time_remaining,
            paused,
            session_count,
            phase,
            work_duration: work,
            rest_duration: rest,
            long_break_duration: long,
            sessions_before_long_break: config.sessions_before_long_break,
            stats,
            current_view: View::Timer,
            theme,
            theme_name: config.theme.clone(),
            sound_enabled: config.sound_enabled,
            animation_frame: 0,
            minimized: false,
            settings_field: SettingsField::WorkDuration,
            settings_editing: false,
            settings_input: String::new(),
            notes_mode: NotesMode::Viewing,
            notes_input: String::new(),
            selected_note_index,
            needs_save: false,
            last_save: Instant::now(),
            auto_start_next: config.auto_start_next,
            extended_break_hours: config.extended_break_reminder_hours,
            last_break_check: Instant::now(),
            work_time_since_break: Duration::ZERO,
        }
    }
    
    fn phase_color(&self) -> Color {
        match self.phase {
            Phase::Work => self.theme.work_color,
            Phase::ShortBreak => self.theme.short_break_color,
            Phase::LongBreak => self.theme.long_break_color,
        }
    }
    
    fn total_duration(&self) -> Duration {
        match self.phase {
            Phase::Work => self.work_duration,
            Phase::ShortBreak => self.rest_duration,
            Phase::LongBreak => self.long_break_duration,
        }
    }
    
    fn progress_ratio(&self) -> f64 {
        let total = self.total_duration().as_secs_f64();
        let remaining = self.time_remaining.as_secs_f64();
        (1.0 - (remaining / total)).clamp(0.0, 1.0)
    }

    fn next_phase(&mut self) {
        self.record_session();
        
        match self.phase {
            Phase::Work => {
                self.stats.total_work_time += self.work_duration.as_secs() / 60;
                self.stats.total_sessions += 1;
                self.stats.sessions_today += 1;
                self.update_weekly_stats();
                self.session_count += 1;
                self.work_time_since_break += self.work_duration;
                
                if self.session_count % self.sessions_before_long_break == 0 {
                    self.phase = Phase::LongBreak;
                    self.time_remaining = self.long_break_duration;
                    notify("Long Break Time! 🌴", "Great work! Take a longer break.", self.sound_enabled);
                    self.work_time_since_break = Duration::ZERO;
                } else {
                    self.phase = Phase::ShortBreak;
                    self.time_remaining = self.rest_duration;
                    notify("Break Time! ☕", "Time for a short break.", self.sound_enabled);
                }
            }
            Phase::ShortBreak | Phase::LongBreak => {
                self.stats.total_break_time += self.total_duration().as_secs() / 60;
                self.phase = Phase::Work;
                self.time_remaining = self.work_duration;
                notify("Back to Work! 🎯", "Let's focus on your next session.", self.sound_enabled);
            }
        }
        
        self.paused = !self.auto_start_next;
        self.needs_save = true;
    }
    
    fn record_session(&mut self) {
        let now = chrono::Local::now();
        let completed = self.time_remaining.as_secs() < 5;
        
        self.stats.session_history.push(SessionRecord {
            timestamp: now.to_rfc3339(),
            phase_type: match self.phase {
                Phase::Work => "Work",
                Phase::ShortBreak => "Short Break",
                Phase::LongBreak => "Long Break",
            }.into(),
            duration: self.total_duration().as_secs() / 60,
            completed,
        });
        
        if self.stats.session_history.len() > MAX_HISTORY {
            self.stats.session_history.remove(0);
        }
    }
    
    fn update_weekly_stats(&mut self) {
        let weekday = chrono::Local::now().weekday().num_days_from_monday() as usize;
        if weekday < 7 {
            self.stats.weekly_sessions[weekday] += 1;
        }
    }
    
    fn check_extended_break(&mut self) {
        if self.last_break_check.elapsed() < Duration::from_secs(60) {
            return;
        }
        
        self.last_break_check = Instant::now();
        let hours = self.work_time_since_break.as_secs_f64() / 3600.0;
        
        if hours >= self.extended_break_hours {
            notify(
                "⚠️  Extended Break Recommended",
                &format!("You've been working for {:.1} hours. Consider taking a longer break!", hours),
                self.sound_enabled
            );
            self.work_time_since_break = Duration::ZERO;
        }
    }

    fn update(&mut self) {
        if !self.paused && self.time_remaining > Duration::ZERO {
            self.time_remaining = self.time_remaining.saturating_sub(TICK_RATE);
            
            if self.phase == Phase::Work {
                self.check_extended_break();
            }
            
            if self.time_remaining.as_secs() == 0 {
                self.next_phase();
            }
        }
        
        self.animation_frame = self.animation_frame.wrapping_add(1) % 20;
        
        if self.needs_save && self.last_save.elapsed() >= AUTO_SAVE_INTERVAL {
            self.save_stats();
            self.last_save = Instant::now();
        }
    }
    
    fn save_config(&self) {
        let config = Config {
            work_duration: self.work_duration.as_secs_f64() / 60.0,
            rest_duration: self.rest_duration.as_secs_f64() / 60.0,
            long_break_duration: self.long_break_duration.as_secs_f64() / 60.0,
            sessions_before_long_break: self.sessions_before_long_break,
            sound_enabled: self.sound_enabled,
            theme: self.theme_name.clone(),
            auto_start_next: self.auto_start_next,
            extended_break_reminder_hours: self.extended_break_hours,
        };
        let _ = save_json(&get_path("config.json"), &config);
    }
    
    fn save_stats(&mut self) {
        if save_json(&get_path("stats.json"), &self.stats).is_ok() {
            self.needs_save = false;
        }
    }
    
    fn save_on_quit(&mut self) {
        self.save_stats();
        
        let state = TimerState {
            time_remaining_secs: self.time_remaining.as_secs(),
            phase: self.phase.to_str().into(),
            session_count: self.session_count,
            paused: self.paused,
        };
        let _ = save_json(&get_path("timer_state.json"), &state);
    }
}

// ============================================================================
// Event Handlers
// ============================================================================

fn handle_input(key: event::KeyEvent, app: &mut AppState) -> bool {
    // Input modes
    if matches!(app.notes_mode, NotesMode::Adding | NotesMode::Editing) {
        match key.code {
            KeyCode::Char(c) => app.notes_input.push(c),
            KeyCode::Backspace => { app.notes_input.pop(); }
            KeyCode::Enter => {
                if !app.notes_input.trim().is_empty() {
                    if app.notes_mode == NotesMode::Editing {
                        if let Some(idx) = app.selected_note_index {
                            if idx < app.stats.notes.len() {
                                app.stats.notes[idx].content = app.notes_input.trim().into();
                                app.needs_save = true;
                            }
                        }
                    } else {
                        let now = chrono::Local::now();
                        app.stats.notes.push(Note {
                            timestamp: now.to_rfc3339(),
                            content: app.notes_input.trim().into(),
                            phase: app.phase.to_str().into(),
                        });
                        app.selected_note_index = Some(app.stats.notes.len() - 1);
                        app.needs_save = true;
                    }
                }
                app.notes_mode = NotesMode::Viewing;
                app.notes_input.clear();
            }
            KeyCode::Esc => {
                app.notes_mode = NotesMode::Viewing;
                app.notes_input.clear();
            }
            _ => {}
        }
        return false;
    }
    
    if app.settings_editing {
        match key.code {
            KeyCode::Char(c) => app.settings_input.push(c),
            KeyCode::Backspace => { app.settings_input.pop(); }
            KeyCode::Enter => apply_setting(app),
            KeyCode::Esc => {
                app.settings_editing = false;
                app.settings_input.clear();
            }
            _ => {}
        }
        return false;
    }
    
    // Delete confirmation
    if app.notes_mode == NotesMode::ConfirmingDelete {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                if let Some(idx) = app.selected_note_index {
                    if idx < app.stats.notes.len() {
                        app.stats.notes.remove(idx);
                        app.selected_note_index = if app.stats.notes.is_empty() {
                            None
                        } else {
                            Some(idx.min(app.stats.notes.len() - 1))
                        };
                        app.needs_save = true;
                    }
                }
                app.notes_mode = NotesMode::Viewing;
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                app.notes_mode = NotesMode::Viewing;
            }
            _ => {}
        }
        return false;
    }
    
    // View-specific handlers
    match app.current_view {
        View::Notes => handle_notes_view(key, app),
        View::Settings => handle_settings_view(key, app),
        _ => handle_main_view(key, app),
    }
}

fn handle_notes_view(key: event::KeyEvent, app: &mut AppState) -> bool {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('t') => {
            app.current_view = View::Timer;
            app.notes_mode = NotesMode::Viewing;
        }
        KeyCode::Char('a') | KeyCode::Char('n') => {
            app.notes_mode = NotesMode::Adding;
            app.notes_input.clear();
        }
        KeyCode::Char('e') => {
            if let Some(idx) = app.selected_note_index {
                if idx < app.stats.notes.len() {
                    app.notes_input = app.stats.notes[idx].content.clone();
                    app.notes_mode = NotesMode::Editing;
                }
            }
        }
        KeyCode::Char('d') => {
            if app.selected_note_index.is_some() {
                app.notes_mode = NotesMode::ConfirmingDelete;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if !app.stats.notes.is_empty() {
                app.selected_note_index = Some(match app.selected_note_index {
                    Some(idx) => (idx + 1).min(app.stats.notes.len() - 1),
                    None => 0,
                });
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if !app.stats.notes.is_empty() {
                app.selected_note_index = Some(match app.selected_note_index {
                    Some(idx) => idx.saturating_sub(1),
                    None => app.stats.notes.len() - 1,
                });
            }
        }
        _ => {}
    }
    false
}

fn handle_settings_view(key: event::KeyEvent, app: &mut AppState) -> bool {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('c') => {
            app.current_view = View::Timer;
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.settings_field = app.settings_field.next();
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.settings_field = app.settings_field.prev();
        }
        KeyCode::Enter | KeyCode::Char('e') => {
            start_editing(app);
        }
        KeyCode::Char(' ') => {
            match app.settings_field {
                SettingsField::SoundEnabled => {
                    app.sound_enabled = !app.sound_enabled;
                    app.save_config();
                }
                SettingsField::AutoStartNext => {
                    app.auto_start_next = !app.auto_start_next;
                    app.save_config();
                }
                _ => {}
            }
        }
        KeyCode::Left | KeyCode::Char('h') => {
            if app.settings_field == SettingsField::Theme {
                cycle_theme(app, false);
            }
        }
        KeyCode::Right | KeyCode::Char('l') => {
            if app.settings_field == SettingsField::Theme {
                cycle_theme(app, true);
            }
        }
        _ => {}
    }
    false
}

fn handle_main_view(key: event::KeyEvent, app: &mut AppState) -> bool {
    if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) || 
       (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)) {
        return true;
    }
    
    if matches!(key.code, KeyCode::Char('m') | KeyCode::Char('M')) {
        app.minimized = !app.minimized;
        return false;
    }
    
    if app.minimized {
        return false;
    }
    
    match key.code {
        KeyCode::Char(' ') => app.paused = !app.paused,
        KeyCode::Char('r') => {
            app.time_remaining = app.total_duration();
            app.paused = false;
        }
        KeyCode::Char('n') => app.next_phase(),
        KeyCode::Char('d') => app.current_view = View::Settings,
        KeyCode::Char('t') => {
            app.current_view = View::Notes;
            app.notes_mode = NotesMode::Viewing;
        }
        KeyCode::Char('h') | KeyCode::Char('?') => {
            app.current_view = if app.current_view == View::Help {
                View::Timer
            } else {
                View::Help
            };
        }
        KeyCode::Char('s') => {
            app.current_view = if app.current_view == View::Timer {
                View::StatsSummary
            } else {
                View::Timer
            };
        }
        KeyCode::Tab => {
            app.current_view = match app.current_view {
                View::StatsSummary => View::StatsDetailed,
                View::StatsDetailed => View::StatsHistory,
                View::StatsHistory => View::StatsSummary,
                _ => app.current_view,
            };
        }
        KeyCode::Char('e') => {
            if matches!(app.current_view, View::StatsSummary | View::StatsDetailed | View::StatsHistory) {
                let _ = export_csv(&app.stats);
            }
        }
        _ => {}
    }
    
    false
}

fn start_editing(app: &mut AppState) {
    let input = match app.settings_field {
        SettingsField::WorkDuration => format_mins(app.work_duration),
        SettingsField::RestDuration => format_mins(app.rest_duration),
        SettingsField::LongBreakDuration => format_mins(app.long_break_duration),
        SettingsField::SessionsBeforeLongBreak => app.sessions_before_long_break.to_string(),
        SettingsField::ExtendedBreakReminder => {
            let h = app.extended_break_hours;
            if h.fract() == 0.0 { format!("{}", h as u64) } else { format!("{:.1}", h) }
        }
        _ => return,
    };
    
    app.settings_input = input;
    app.settings_editing = true;
}

fn format_mins(d: Duration) -> String {
    let m = d.as_secs_f64() / 60.0;
    if m.fract() == 0.0 {
        format!("{}", m as u64)
    } else {
        format!("{:.2}", m)
    }
}

fn apply_setting(app: &mut AppState) {
    let parsed = app.settings_input.parse::<f64>();
    
    match app.settings_field {
        SettingsField::WorkDuration => {
            if let Ok(m) = parsed {
                if (0.0..=240.0).contains(&m) {
                    app.work_duration = Duration::from_secs_f64(m * 60.0);
                    app.save_config();
                }
            }
        }
        SettingsField::RestDuration => {
            if let Ok(m) = parsed {
                if (0.0..=60.0).contains(&m) {
                    app.rest_duration = Duration::from_secs_f64(m * 60.0);
                    app.save_config();
                }
            }
        }
        SettingsField::LongBreakDuration => {
            if let Ok(m) = parsed {
                if (0.0..=120.0).contains(&m) {
                    app.long_break_duration = Duration::from_secs_f64(m * 60.0);
                    app.save_config();
                }
            }
        }
        SettingsField::SessionsBeforeLongBreak => {
            if let Ok(s) = app.settings_input.parse::<u32>() {
                if (1..=10).contains(&s) {
                    app.sessions_before_long_break = s;
                    app.save_config();
                }
            }
        }
        SettingsField::ExtendedBreakReminder => {
            if let Ok(h) = parsed {
                if (0.5..=8.0).contains(&h) {
                    app.extended_break_hours = h;
                    app.save_config();
                }
            }
        }
        _ => {}
    }
    
    app.settings_editing = false;
    app.settings_input.clear();
}

fn cycle_theme(app: &mut AppState, forward: bool) {
    const THEMES: &[&str] = &["default", "nord", "dracula", "gruvbox", "solarized"];
    let idx = THEMES.iter().position(|&t| t == app.theme_name).unwrap_or(0);
    let new_idx = if forward {
        (idx + 1) % THEMES.len()
    } else {
        if idx == 0 { THEMES.len() - 1 } else { idx - 1 }
    };
    
    app.theme_name = THEMES[new_idx].into();
    app.theme = get_theme(&app.theme_name);
    app.save_config();
}

// ============================================================================
// UI Rendering
// ============================================================================

fn render_ui(f: &mut Frame, app: &AppState) {
    if app.minimized {
        render_minimized(f, app);
    } else {
        match app.current_view {
            View::Timer => render_timer(f, app),
            View::Help => render_help(f, app),
            View::StatsSummary => render_stats_summary(f, app),
            View::StatsDetailed => render_stats_detailed(f, app),
            View::StatsHistory => render_stats_history(f, app),
            View::Settings => render_settings(f, app),
            View::Notes => render_notes(f, app),
        }
    }
}

fn render_minimized(f: &mut Frame, app: &AppState) {
    let area = centered_rect(40, 30, f.size());
    let secs = app.time_remaining.as_secs();
    let time_str = format!("{:02}:{:02}", secs / 60, secs % 60);
    let status = if app.paused { "⏸ PAUSED" } else { "▶ RUNNING" };
    
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(app.phase.name(), Style::default()
            .fg(app.phase_color()).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(Span::styled(time_str, Style::default()
            .fg(app.phase_color()).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(Span::styled(status, Style::default()
            .fg(if app.paused { Color::Yellow } else { Color::Green }))),
        Line::from(""),
        Line::from(""),
        Line::from(Span::styled("Press M to restore", Style::default()
            .fg(Color::DarkGray).add_modifier(Modifier::ITALIC))),
    ];
    
    let widget = Paragraph::new(lines)
        .alignment(Alignment::Center)
        .block(Block::default()
            .title(" 🍅 RTIMER (Minimized) ")
            .title_alignment(Alignment::Center)
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(app.theme.border_color)));
    
    f.render_widget(widget, area);
}

fn render_timer(f: &mut Frame, app: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1), Constraint::Length(3)])
        .split(f.size());
    
    // Header
    let header = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(app.theme.border_color))
        .title(Span::styled(" 🍅 RTIMER ", Style::default()
            .fg(app.theme.accent_color).add_modifier(Modifier::BOLD)));
    f.render_widget(header, chunks[0]);
    
    // Main content
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(10),
            Constraint::Length(3), Constraint::Length(1),
            Constraint::Length(5), Constraint::Length(1),
            Constraint::Length(2), Constraint::Length(1),
            Constraint::Length(2), Constraint::Length(1),
            Constraint::Length(3), Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Percentage(10),
        ])
        .split(chunks[1]);
    
    // Phase
    f.render_widget(
        Paragraph::new(app.phase.name())
            .style(Style::default().fg(app.phase_color()).add_modifier(Modifier::BOLD))
            .alignment(Alignment::Center),
        sections[1]
    );
    
    // Timer
    let secs = app.time_remaining.as_secs();
    let time_str = format!("{:02}:{:02}", secs / 60, secs % 60);
    f.render_widget(
        Paragraph::new(time_str)
            .style(Style::default().fg(app.phase_color()).add_modifier(Modifier::BOLD))
            .alignment(Alignment::Center),
        sections[3]
    );
    
    // Date/time
    let now = chrono::Local::now();
    let date_lines = vec![
        Line::from(Span::styled(now.format("%A, %B %d, %Y").to_string(), Style::default().fg(Color::Gray))),
        Line::from(Span::styled(now.format("%I:%M %p").to_string(), Style::default().fg(Color::DarkGray))),
    ];
    f.render_widget(Paragraph::new(date_lines).alignment(Alignment::Center), sections[5]);
    
    // Status
    let status = if app.paused {
        format!("⏸  PAUSED{}", ".".repeat((app.animation_frame / 5) as usize % 4))
    } else {
        format!("{} RUNNING", if app.animation_frame < 10 { "●" } else { "○" })
    };
    f.render_widget(
        Paragraph::new(status)
            .style(Style::default()
                .fg(if app.paused { Color::Yellow } else { Color::Green })
                .add_modifier(Modifier::BOLD))
            .alignment(Alignment::Center),
        sections[7]
    );
    
    // Progress
    f.render_widget(
        Gauge::default()
            .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded))
            .gauge_style(Style::default().fg(app.phase_color()).bg(Color::Black))
            .percent((app.progress_ratio() * 100.0) as u16),
        sections[9]
    );
    
    // Session info
    let session_text = format!(
        "Session {} of {}  •  {} completed today",
        ((app.session_count - 1) % app.sessions_before_long_break) + 1,
        app.sessions_before_long_break,
        app.stats.sessions_today
    );
    f.render_widget(
        Paragraph::new(session_text).style(Style::default().fg(Color::Gray)).alignment(Alignment::Center),
        sections[11]
    );
    
    // Controls
    let controls = vec![
        Line::from(vec![
            span_key("Space", app), Span::raw(" Pause/Resume  •  "),
            span_key("R", app), Span::raw(" Reset  •  "),
            span_key("N", app), Span::raw(" Skip  •  "),
            span_key("M", app), Span::raw(" Minimize"),
        ]),
        Line::from(vec![
            span_key("T", app), Span::raw(" Notes  •  "),
            span_key("S", app), Span::raw(" Stats  •  "),
            span_key("D", app), Span::raw(" Settings  •  "),
            span_key("H", app), Span::raw(" Help  •  "),
            span_key("Q", app), Span::raw(" Quit"),
        ]),
    ];
    f.render_widget(
        Paragraph::new(controls).alignment(Alignment::Center).style(Style::default().fg(Color::DarkGray)),
        chunks[2]
    );
}

fn span_key<'a>(text: &'a str, app: &AppState) -> Span<'a> {
    Span::styled(text, Style::default().fg(app.theme.accent_color).add_modifier(Modifier::BOLD))
}

fn render_help(f: &mut Frame, app: &AppState) {
    let area = centered_rect(70, 85, f.size());
    
    let help_text = vec![
        Line::from(""),
        Line::from(Span::styled("⌨️  KEYBOARD SHORTCUTS", Style::default().fg(app.theme.accent_color).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from("  Timer Controls:"),
        help_line("Space", "Toggle pause/resume"),
        help_line("R", "Reset current timer"),
        help_line("N", "Skip to next phase"),
        help_line("M", "Minimize to compact view"),
        Line::from(""),
        Line::from("  Navigation:"),
        help_line("T", "Open notes view"),
        help_line("S", "Open statistics"),
        help_line("D", "Open settings"),
        help_line("H / ?", "Toggle help"),
        help_line("Tab", "Cycle through stat views"),
        Line::from(""),
        Line::from("  Notes View:"),
        help_line("A / N", "Add new note"),
        help_line("E", "Edit selected note"),
        help_line("D", "Delete selected note"),
        help_line("↑↓ / JK", "Navigate between notes"),
        Line::from(""),
        Line::from("  General:"),
        help_line("Q / Esc", "Exit / Go back"),
        help_line("Ctrl+C", "Force quit"),
        Line::from(""),
        Line::from(Span::styled("💡 Auto-save enabled • Extended break reminders • Customizable themes", 
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC))),
    ];
    
    f.render_widget(
        Paragraph::new(help_text)
            .alignment(Alignment::Left)
            .block(Block::default()
                .title(" Help ")
                .title_alignment(Alignment::Center)
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(app.theme.border_color))),
        area
    );
}

fn help_line<'a>(key: &'a str, desc: &'a str) -> Line<'a> {
    Line::from(vec![
        Span::raw("    "),
        Span::styled(key, Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::raw(format!("  {}", desc)),
    ])
}

fn render_stats_summary(f: &mut Frame, app: &AppState) {
    let area = centered_rect(70, 85, f.size());
    
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled("📊 STATISTICS OVERVIEW", Style::default().fg(app.theme.accent_color).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(Span::styled("  Press Tab to cycle views  •  E to export CSV", Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC))),
        Line::from(""),
        Line::from(""),
        Line::from(Span::styled("  📅 Today:", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
        stat_line("Sessions completed", app.stats.sessions_today.to_string()),
        Line::from(""),
        Line::from(Span::styled("  📈 All Time:", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))),
        stat_line("Total sessions", app.stats.total_sessions.to_string()),
        stat_line("Total focus time", format!("{:.1} hours", app.stats.total_work_time as f64 / 60.0)),
        stat_line("Total break time", format!("{:.1} hours", app.stats.total_break_time as f64 / 60.0)),
        Line::from(""),
        Line::from(Span::styled("  📝 Notes:", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
        stat_line("Total notes", app.stats.notes.len().to_string()),
    ];
    
    f.render_widget(
        Paragraph::new(lines)
            .block(Block::default()
                .title(" Statistics ")
                .title_alignment(Alignment::Center)
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(app.theme.border_color))),
        area
    );
}

fn stat_line(label: &str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::raw(format!("     {}: ", label)),
        Span::styled(value, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
    ])
}

fn render_stats_detailed(f: &mut Frame, app: &AppState) {
    let area = centered_rect(70, 85, f.size());
    
    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled("📊 WEEKLY BREAKDOWN", Style::default().fg(app.theme.accent_color).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(Span::styled("  Sessions per day this week:", Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC))),
        Line::from(""),
    ];
    
    let days = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    let max = *app.stats.weekly_sessions.iter().max().unwrap_or(&1).max(&1);
    
    for (i, &count) in app.stats.weekly_sessions.iter().enumerate() {
        let width = (count as f64 / max as f64 * 30.0) as usize;
        lines.push(Line::from(vec![
            Span::styled(format!("  {} ", days[i]), Style::default().fg(Color::Gray)),
            Span::styled("█".repeat(width), Style::default().fg(app.theme.accent_color)),
            Span::raw(format!(" {}", count)),
        ]));
    }
    
    f.render_widget(
        Paragraph::new(lines)
            .block(Block::default()
                .title(" Weekly Stats ")
                .title_alignment(Alignment::Center)
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(app.theme.border_color))),
        area
    );
}

fn render_stats_history(f: &mut Frame, app: &AppState) {
    let area = centered_rect(75, 85, f.size());
    
    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled("📜 RECENT SESSION HISTORY", Style::default().fg(app.theme.accent_color).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(Span::styled("  Last 15 sessions:", Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC))),
        Line::from(""),
    ];
    
    if app.stats.session_history.is_empty() {
        lines.push(Line::from(Span::styled("  No sessions yet!", Style::default().fg(Color::DarkGray))));
    } else {
        for s in app.stats.session_history.iter().rev().take(15) {
            let dt = s.timestamp.split('T')
                .next()
                .and_then(|d| s.timestamp.split('T').nth(1)?.split('.').next().map(|t| format!("{} {}", d, &t[..5])))
                .unwrap_or_else(|| "Unknown".into());
            
            let icon = match s.phase_type.as_str() {
                "Work" => "🎯",
                "Short Break" => "☕",
                "Long Break" => "🌴",
                _ => "📝",
            };
            
            let (status, color) = if s.completed { ("✓", Color::Green) } else { ("⏸", Color::Yellow) };
            
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::raw(icon),
                Span::raw(" "),
                Span::styled(dt, Style::default().fg(Color::Gray)),
                Span::raw(" • "),
                Span::styled(&s.phase_type, Style::default().fg(Color::White)),
                Span::raw(" • "),
                Span::styled(format!("{}m", s.duration), Style::default().fg(Color::Cyan)),
                Span::raw(" "),
                Span::styled(status, Style::default().fg(color)),
            ]));
        }
    }
    
    f.render_widget(
        Paragraph::new(lines)
            .block(Block::default()
                .title(" Session History ")
                .title_alignment(Alignment::Center)
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(app.theme.border_color))),
        area
    );
}

fn render_settings(f: &mut Frame, app: &AppState) {
    let area = centered_rect(70, 85, f.size());
    
    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled("⚙️  SETTINGS", Style::default().fg(app.theme.accent_color).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(Span::styled("  ↑↓/jk: Navigate  •  Enter: Edit  •  Space: Toggle  •  ←→/hl: Themes", 
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC))),
        Line::from(""),
    ];
    
    let settings = [
        (SettingsField::WorkDuration, "🎯 Work Duration", format_mins(app.work_duration) + " min"),
        (SettingsField::RestDuration, "☕ Rest Duration", format_mins(app.rest_duration) + " min"),
        (SettingsField::LongBreakDuration, "🌴 Long Break", format_mins(app.long_break_duration) + " min"),
        (SettingsField::SessionsBeforeLongBreak, "🔄 Sessions Before Long Break", format!("{} sessions", app.sessions_before_long_break)),
        (SettingsField::Theme, "🎨 Theme", format!("< {} >", app.theme_name)),
        (SettingsField::SoundEnabled, "🔔 Sound", if app.sound_enabled { "ON" } else { "OFF" }.into()),
        (SettingsField::AutoStartNext, "▶️  Auto-Start", if app.auto_start_next { "ON" } else { "OFF" }.into()),
        (SettingsField::ExtendedBreakReminder, "⏰ Break Reminder", format!("After {:.1}h", app.extended_break_hours)),
    ];
    
    for (field, label, value) in settings {
        let selected = app.settings_field == field;
        let editing = selected && app.settings_editing;
        
        lines.push(Line::from(""));
        
        if editing {
            lines.push(Line::from(vec![
                Span::styled("  > ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                Span::styled(label, Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            ]));
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(&app.settings_input, Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                Span::styled("█", Style::default().fg(Color::Green)),
            ]));
        } else {
            let (prefix, label_style, value_style) = if selected {
                ("  > ", Style::default().fg(app.theme.accent_color).add_modifier(Modifier::BOLD),
                 Style::default().fg(Color::White).add_modifier(Modifier::BOLD))
            } else {
                ("    ", Style::default().fg(Color::Gray), Style::default().fg(Color::DarkGray))
            };
            
            lines.push(Line::from(vec![Span::styled(prefix, label_style), Span::styled(label, label_style)]));
            lines.push(Line::from(vec![Span::raw("    "), Span::styled(value, value_style)]));
        }
    }
    
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  💾 Auto-saved", Style::default().fg(Color::Green).add_modifier(Modifier::ITALIC))));
    
    f.render_widget(
        Paragraph::new(lines)
            .block(Block::default()
                .title(" Settings ")
                .title_alignment(Alignment::Center)
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(app.theme.border_color))),
        area
    );
}

fn render_notes(f: &mut Frame, app: &AppState) {
    let area = centered_rect(80, 85, f.size());
    
    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled("📝 NOTES", Style::default().fg(app.theme.accent_color).add_modifier(Modifier::BOLD))),
        Line::from(""),
    ];
    
    let help = match app.notes_mode {
        NotesMode::Viewing => "  a/n: Add  •  e: Edit  •  d: Delete  •  ↑↓/jk: Navigate  •  t/Esc: Close",
        NotesMode::Adding => "  Type note and press Enter to save  •  Esc to cancel",
        NotesMode::Editing => "  Edit note and press Enter to save  •  Esc to cancel",
        NotesMode::ConfirmingDelete => "  Y: Confirm  •  N/Esc: Cancel",
    };
    lines.push(Line::from(Span::styled(help, Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC))));
    lines.push(Line::from(""));
    
    if matches!(app.notes_mode, NotesMode::Adding | NotesMode::Editing) {
        let title = if app.notes_mode == NotesMode::Adding { "✏️  NEW NOTE" } else { "✏️  EDITING" };
        lines.push(Line::from(Span::styled(format!("  {}", title), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(&app.notes_input, Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::styled("█", Style::default().fg(Color::Green)),
        ]));
        lines.push(Line::from(""));
        lines.push(Line::from("  ─────────────────────────────────────────────────────────────────────"));
        lines.push(Line::from(""));
    }
    
    if app.notes_mode == NotesMode::ConfirmingDelete {
        if let Some(idx) = app.selected_note_index {
            if idx < app.stats.notes.len() {
                lines.push(Line::from(Span::styled("  ⚠️  DELETE NOTE?", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))));
                lines.push(Line::from(""));
                lines.push(Line::from(vec![Span::raw("  "), Span::styled(&app.stats.notes[idx].content, Style::default().fg(Color::White))]));
                lines.push(Line::from(""));
                lines.push(Line::from("  ─────────────────────────────────────────────────────────────────────"));
                lines.push(Line::from(""));
            }
        }
    }
    
    if app.stats.notes.is_empty() {
        lines.push(Line::from(Span::styled("  No notes yet! Press 'a' to add one.", Style::default().fg(Color::Gray))));
    } else {
        lines.push(Line::from(Span::styled(format!("  {} NOTES", app.stats.notes.len()), 
            Style::default().fg(app.theme.accent_color).add_modifier(Modifier::BOLD))));
        lines.push(Line::from(""));
        
        for (idx, note) in app.stats.notes.iter().enumerate().rev() {
            let selected = app.selected_note_index == Some(idx);
            let dt = note.timestamp.split('T')
                .next()
                .and_then(|d| note.timestamp.split('T').nth(1)?.split('.').next().map(|t| format!("{} {}", d, t)))
                .unwrap_or_else(|| "Unknown".into());
            
            let icon = match note.phase.as_str() {
                "work" => "🎯",
                "short_break" => "☕",
                "long_break" => "🌴",
                _ => "📝",
            };
            
            let prefix = if selected { "► " } else { "  " };
            let style = if selected { 
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD) 
            } else { 
                Style::default().fg(Color::Gray) 
            };
            
            lines.push(Line::from(vec![
                Span::styled(prefix, if selected { Style::default().fg(app.theme.accent_color).add_modifier(Modifier::BOLD) } else { Style::default() }),
                Span::raw(icon),
                Span::raw("  "),
                Span::styled(dt, style),
            ]));
            lines.push(Line::from(vec![Span::raw("     "), Span::styled(&note.content, style)]));
            lines.push(Line::from(""));
        }
    }
    
    f.render_widget(
        Paragraph::new(lines)
            .block(Block::default()
                .title(" Notes ")
                .title_alignment(Alignment::Center)
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(app.theme.border_color))),
        area
    );
}

fn centered_rect(w: u16, h: u16, r: Rect) -> Rect {
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - h) / 2),
            Constraint::Percentage(h),
            Constraint::Percentage((100 - h) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - w) / 2),
            Constraint::Percentage(w),
            Constraint::Percentage((100 - w) / 2),
        ])
        .split(v[1])[1]
}

// ============================================================================
// Utilities
// ============================================================================

fn notify(title: &str, body: &str, sound: bool) {
    let _ = Notification::new()
        .summary(title)
        .body(body)
        .appname("rtimer")
        .icon("alarm-clock")
        .urgency(Urgency::Critical)
        .show();

    if sound {
        std::thread::spawn(|| {
            for (cmd, file) in [
                ("paplay", "/usr/share/sounds/freedesktop/stereo/complete.oga"),
                ("aplay", "/usr/share/sounds/sound-icons/guitar-11.wav"),
                ("aplay", "/usr/share/sounds/generic.wav"),
            ] {
                if std::path::Path::new(file).exists() {
                    let _ = std::process::Command::new(cmd)
                        .arg(file)
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn();
                    break;
                }
            }
        });
    }
}

fn get_theme(name: &str) -> Theme {
    match name {
        "nord" => Theme {
            work_color: Color::Rgb(136, 192, 255),
            short_break_color: Color::Rgb(255, 20, 60),
            long_break_color: Color::Rgb(0, 255, 100),
            border_color: Color::Rgb(100, 200, 255),
            accent_color: Color::Rgb(255, 100, 255),
        },
        "dracula" => Theme {
            work_color: Color::Rgb(189, 147, 249),
            short_break_color: Color::Rgb(255, 0, 85),
            long_break_color: Color::Rgb(0, 255, 0),
            border_color: Color::Rgb(200, 100, 255),
            accent_color: Color::Rgb(255, 0, 255),
        },
        "gruvbox" => Theme {
            work_color: Color::Rgb(254, 128, 25),
            short_break_color: Color::Rgb(255, 50, 0),
            long_break_color: Color::Rgb(255, 255, 0),
            border_color: Color::Rgb(255, 200, 100),
            accent_color: Color::Rgb(255, 150, 0),
        },
        "solarized" => Theme {
            work_color: Color::Rgb(42, 161, 152),
            short_break_color: Color::Rgb(255, 0, 0),
            long_break_color: Color::Rgb(150, 255, 0),
            border_color: Color::Rgb(100, 200, 255),
            accent_color: Color::Rgb(255, 200, 0),
        },
        _ => Theme {
            work_color: Color::Rgb(100, 181, 246),
            short_break_color: Color::Rgb(255, 0, 100),
            long_break_color: Color::Rgb(0, 255, 150),
            border_color: Color::Rgb(0, 200, 255),
            accent_color: Color::Rgb(255, 100, 0),
        },
    }
}

fn get_path(filename: &str) -> PathBuf {
    let mut path = PathBuf::from(".");
    path.push("rtimer");
    let _ = fs::create_dir_all(&path);
    path.push(filename);
    path
}

fn load_json<T: for<'de> Deserialize<'de> + Default>(path: &PathBuf) -> T {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_json<T: Serialize>(path: &PathBuf, data: &T) -> io::Result<()> {
    fs::write(path, serde_json::to_string_pretty(data)?)
}

fn reset_daily_stats(stats: &mut Statistics) {
    let today = chrono::Local::now().format(DAILY_FMT).to_string();
    if stats.last_session_date != today {
        stats.sessions_today = 0;
        stats.last_session_date = today;
        stats.weekly_sessions.rotate_left(1);
        stats.weekly_sessions[6] = 0;
    }
}

fn export_csv(stats: &Statistics) -> io::Result<()> {
    let mut csv = format!(
        "Date,Total Sessions,Sessions Today,Work Time (h),Break Time (h)\n{},{},{},{:.2},{:.2}\n\n",
        stats.last_session_date,
        stats.total_sessions,
        stats.sessions_today,
        stats.total_work_time as f64 / 60.0,
        stats.total_break_time as f64 / 60.0
    );
    
    csv.push_str("Session History\nTimestamp,Phase,Duration (min),Completed\n");
    for s in stats.session_history.iter().rev().take(50) {
        csv.push_str(&format!("{},{},{},{}\n", s.timestamp, s.phase_type, s.duration, if s.completed { "Yes" } else { "No" }));
    }
    
    csv.push_str("\nNotes\nTimestamp,Phase,Content\n");
    for n in stats.notes.iter().rev() {
        let content = if n.content.contains(',') || n.content.contains('"') {
            format!("\"{}\"", n.content.replace('"', "\"\""))
        } else {
            n.content.clone()
        };
        csv.push_str(&format!("{},{},{}\n", n.timestamp, n.phase, content));
    }
    
    fs::write(get_path("stats_export.csv"), csv)
}

// ============================================================================
// Main
// ============================================================================

fn main() -> Result<()> {
    let args = Args::parse();
    let mut config = load_json::<Config>(&get_path("config.json"));
    
    // CLI overrides
    if let Some(w) = args.work { config.work_duration = w; }
    if let Some(r) = args.rest { config.rest_duration = r; }
    if let Some(l) = args.long_break { config.long_break_duration = l; }
    if let Some(s) = args.sessions { config.sessions_before_long_break = s; }
    if let Some(t) = args.theme { config.theme = t; }
    if args.no_sound { config.sound_enabled = false; }
    
    let mut stats = load_json::<Statistics>(&get_path("stats.json"));
    reset_daily_stats(&mut stats);
    
    let saved = if args.resume {
        load_json::<Option<TimerState>>(&get_path("timer_state.json"))
    } else {
        None
    };
    
    let mut app = AppState::new(config, stats, saved);
    
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let res = run(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;

    res
}

fn run(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut AppState) -> Result<()> {
    let mut last_tick = Instant::now();

    loop {
        terminal.draw(|f| render_ui(f, app))?;

        let timeout = TICK_RATE.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if handle_input(key, app) {
                    app.save_on_quit();
                    return Ok(());
                }
            }
        }

        if last_tick.elapsed() >= TICK_RATE {
            app.update();
            last_tick = Instant::now();
        }
    }
}
