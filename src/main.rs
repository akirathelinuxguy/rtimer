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
use std::{
    fs,
    io,
    path::PathBuf,
    time::{Duration, Instant},
};

// ============================================================================
// CLI Arguments
// ============================================================================

#[derive(Parser, Clone)]
#[command(author, version, about = "🍅 rtimer - A Beautiful Terminal Pomodoro Timer")]
struct Args {
    /// Work duration (e.g., "25m", "1h30m", "90m", "0.5m"). Overrides config if provided.
    #[arg(short, long, value_parser = parse_duration_arg)]
    work: Option<f64>,

    /// Rest duration (e.g., "5m", "300s", "0.5m"). Overrides config if provided.
    #[arg(short, long, value_parser = parse_duration_arg)]
    rest: Option<f64>,

    /// Long break duration (e.g., "15m", "0.25h"). Overrides config if provided.
    #[arg(short, long, value_parser = parse_duration_arg)]
    long_break: Option<f64>,

    /// Sessions before long break. Overrides config if provided.
    #[arg(short, long)]
    sessions: Option<u32>,

    /// Theme (default, nord, dracula, gruvbox, solarized). Overrides config if provided.
    #[arg(short = 't', long)]
    theme: Option<String>,

    /// Disable sound notifications
    #[arg(long)]
    no_sound: bool,

    /// Resume from saved state
    #[arg(long)]
    resume: bool,
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
            theme: "default".to_string(),
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
            last_session_date: chrono::Local::now().format("%Y-%m-%d").to_string(),
            session_history: Vec::new(),
            weekly_sessions: vec![0; 7],
            notes: Vec::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
enum Phase {
    Work,
    ShortBreak,
    LongBreak,
}

#[derive(Serialize, Deserialize)]
struct TimerState {
    time_remaining_secs: u64,
    phase: String,
    session_count: u32,
    paused: bool,
}

#[derive(Clone)]
struct MergedSettings {
    work_duration: f64,
    rest_duration: f64,
    long_break_duration: f64,
    sessions_before_long_break: u32,
}

#[derive(Clone)]
struct Theme {
    work_color: Color,
    short_break_color: Color,
    long_break_color: Color,
    border_color: Color,
    accent_color: Color,
}

#[derive(PartialEq, Clone)]
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

#[derive(PartialEq)]
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
    // Timer state
    time_remaining: Duration,
    paused: bool,
    session_count: u32,
    phase: Phase,
    
    // Configuration
    work_duration: Duration,
    rest_duration: Duration,
    long_break_duration: Duration,
    sessions_before_long_break: u32,
    
    // Statistics
    stats: Statistics,
    
    // UI state
    current_view: View,
    theme: Theme,
    sound_enabled: bool,
    animation_frame: u8,
    minimized: bool,
    
    // Settings menu state
    settings_selected_field: SettingsField,
    settings_editing: bool,
    settings_input: String,
    theme_name: String,
    
    // Enhanced notes state
    notes_mode: NotesMode,
    notes_input: String,
    note_scroll_offset: usize,
    selected_note_index: Option<usize>, // Index in the notes vector
    
    // Save tracking
    needs_stats_save: bool,
    last_auto_save: Instant,
    
    // New features
    auto_start_next: bool,
    extended_break_reminder_hours: f64,
    last_extended_break_check: Instant,
    total_work_time_since_break: Duration,
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
        1.0 - (remaining / total).max(0.0).min(1.0)
    }

    fn next_phase(&mut self) {
        self.record_session_completion();
        
        match self.phase {
            Phase::Work => {
                self.stats.total_work_time += self.work_duration.as_secs() / 60;
                self.stats.total_sessions += 1;
                self.stats.sessions_today += 1;
                self.update_weekly_stats();
                self.session_count += 1;
                
                // Track work time for extended break reminder
                self.total_work_time_since_break += self.work_duration;
                
                if self.session_count % self.sessions_before_long_break == 0 {
                    self.phase = Phase::LongBreak;
                    self.time_remaining = self.long_break_duration;
                    send_notification("Long Break Time! 🌴", 
                        "Great work! Take a longer break.", self.sound_enabled);
                    
                    // Reset extended break timer after long break
                    self.total_work_time_since_break = Duration::from_secs(0);
                } else {
                    self.phase = Phase::ShortBreak;
                    self.time_remaining = self.rest_duration;
                    send_notification("Break Time! ☕", 
                        "Time for a short break.", self.sound_enabled);
                }
            }
            Phase::ShortBreak | Phase::LongBreak => {
                self.stats.total_break_time += self.total_duration().as_secs() / 60;
                self.phase = Phase::Work;
                self.time_remaining = self.work_duration;
                send_notification("Back to Work! 🎯", 
                    "Let's focus on your next session.", self.sound_enabled);
            }
        }
        
        // Auto-start next phase if enabled, otherwise pause
        self.paused = !self.auto_start_next;
        self.mark_stats_dirty();
    }
    
    fn record_session_completion(&mut self) {
        let now = chrono::Local::now();
        let phase_type = match self.phase {
            Phase::Work => "Work",
            Phase::ShortBreak => "Short Break",
            Phase::LongBreak => "Long Break",
        }.to_string();
        
        let duration = self.total_duration().as_secs() / 60;
        let completed = self.time_remaining.as_secs() < 5; // Consider completed if < 5 seconds remaining
        
        self.stats.session_history.push(SessionRecord {
            timestamp: now.to_rfc3339(),
            phase_type,
            duration,
            completed,
        });
        
        // Keep only last 100 sessions
        if self.stats.session_history.len() > 100 {
            self.stats.session_history.remove(0);
        }
    }
    
    fn update_weekly_stats(&mut self) {
        let weekday = chrono::Local::now().weekday().num_days_from_monday() as usize;
        if weekday < self.stats.weekly_sessions.len() {
            self.stats.weekly_sessions[weekday] += 1;
        }
    }
    
    fn check_extended_break_reminder(&mut self) {
        // Only check every minute to avoid spam
        if self.last_extended_break_check.elapsed() < Duration::from_secs(60) {
            return;
        }
        
        self.last_extended_break_check = Instant::now();
        
        // Check if we've been working too long
        let hours_worked = self.total_work_time_since_break.as_secs_f64() / 3600.0;
        if hours_worked >= self.extended_break_reminder_hours {
            send_notification(
                "⚠️  Extended Break Recommended", 
                &format!("You've been working for {:.1} hours. Consider taking a longer break!", hours_worked),
                self.sound_enabled
            );
            // Reset the timer so we don't spam notifications
            self.total_work_time_since_break = Duration::from_secs(0);
        }
    }

    fn toggle_pause(&mut self) {
        self.paused = !self.paused;
    }
    
    fn toggle_minimize(&mut self) {
        self.minimized = !self.minimized;
    }

    fn reset_timer(&mut self) {
        self.time_remaining = self.total_duration();
        self.paused = false;
    }
    
    fn open_notes(&mut self) {
        self.current_view = View::Notes;
        self.notes_mode = NotesMode::Viewing;
        self.selected_note_index = if !self.stats.notes.is_empty() {
            Some(self.stats.notes.len() - 1) // Select last note by default
        } else {
            None
        };
    }
    
    fn open_settings(&mut self) {
        self.current_view = View::Settings;
    }
    
    // ========================================================================
    // Settings Management
    // ========================================================================
    
    fn settings_next_field(&mut self) {
        self.settings_selected_field = match self.settings_selected_field {
            SettingsField::WorkDuration => SettingsField::RestDuration,
            SettingsField::RestDuration => SettingsField::LongBreakDuration,
            SettingsField::LongBreakDuration => SettingsField::SessionsBeforeLongBreak,
            SettingsField::SessionsBeforeLongBreak => SettingsField::Theme,
            SettingsField::Theme => SettingsField::SoundEnabled,
            SettingsField::SoundEnabled => SettingsField::AutoStartNext,
            SettingsField::AutoStartNext => SettingsField::ExtendedBreakReminder,
            SettingsField::ExtendedBreakReminder => SettingsField::WorkDuration,
        };
    }
    
    fn settings_prev_field(&mut self) {
        self.settings_selected_field = match self.settings_selected_field {
            SettingsField::WorkDuration => SettingsField::ExtendedBreakReminder,
            SettingsField::RestDuration => SettingsField::WorkDuration,
            SettingsField::LongBreakDuration => SettingsField::RestDuration,
            SettingsField::SessionsBeforeLongBreak => SettingsField::LongBreakDuration,
            SettingsField::Theme => SettingsField::SessionsBeforeLongBreak,
            SettingsField::SoundEnabled => SettingsField::Theme,
            SettingsField::AutoStartNext => SettingsField::SoundEnabled,
            SettingsField::ExtendedBreakReminder => SettingsField::AutoStartNext,
        };
    }
    
    fn settings_start_editing(&mut self) {
        match self.settings_selected_field {
            SettingsField::Theme | SettingsField::SoundEnabled | SettingsField::AutoStartNext => {
                // These are toggled/cycled, not edited
                return;
            }
            SettingsField::WorkDuration => {
                let mins = self.work_duration.as_secs_f64() / 60.0;
                self.settings_input = if mins.fract() == 0.0 {
                    format!("{}", mins as u64)
                } else {
                    format!("{:.2}", mins)
                };
            }
            SettingsField::RestDuration => {
                let mins = self.rest_duration.as_secs_f64() / 60.0;
                self.settings_input = if mins.fract() == 0.0 {
                    format!("{}", mins as u64)
                } else {
                    format!("{:.2}", mins)
                };
            }
            SettingsField::LongBreakDuration => {
                let mins = self.long_break_duration.as_secs_f64() / 60.0;
                self.settings_input = if mins.fract() == 0.0 {
                    format!("{}", mins as u64)
                } else {
                    format!("{:.2}", mins)
                };
            }
            SettingsField::SessionsBeforeLongBreak => {
                self.settings_input = format!("{}", self.sessions_before_long_break);
            }
            SettingsField::ExtendedBreakReminder => {
                self.settings_input = if self.extended_break_reminder_hours.fract() == 0.0 {
                    format!("{}", self.extended_break_reminder_hours as u64)
                } else {
                    format!("{:.1}", self.extended_break_reminder_hours)
                };
            }
        }
        self.settings_editing = true;
    }
    
    fn settings_cancel_editing(&mut self) {
        self.settings_editing = false;
        self.settings_input.clear();
    }
    
    fn settings_apply_change(&mut self) {
        match self.settings_selected_field {
            SettingsField::WorkDuration => {
                if let Ok(mins) = self.settings_input.parse::<f64>() {
                    if mins > 0.0 && mins <= 240.0 {
                        self.work_duration = Duration::from_secs_f64(mins * 60.0);
                        self.save_config();
                    }
                }
            }
            SettingsField::RestDuration => {
                if let Ok(mins) = self.settings_input.parse::<f64>() {
                    if mins > 0.0 && mins <= 60.0 {
                        self.rest_duration = Duration::from_secs_f64(mins * 60.0);
                        self.save_config();
                    }
                }
            }
            SettingsField::LongBreakDuration => {
                if let Ok(mins) = self.settings_input.parse::<f64>() {
                    if mins > 0.0 && mins <= 120.0 {
                        self.long_break_duration = Duration::from_secs_f64(mins * 60.0);
                        self.save_config();
                    }
                }
            }
            SettingsField::SessionsBeforeLongBreak => {
                if let Ok(sessions) = self.settings_input.parse::<u32>() {
                    if sessions > 0 && sessions <= 10 {
                        self.sessions_before_long_break = sessions;
                        self.save_config();
                    }
                }
            }
            SettingsField::ExtendedBreakReminder => {
                if let Ok(hours) = self.settings_input.parse::<f64>() {
                    if hours >= 0.5 && hours <= 8.0 {
                        self.extended_break_reminder_hours = hours;
                        self.save_config();
                    }
                }
            }
            _ => {}
        }
        self.settings_cancel_editing();
    }
    
    fn settings_toggle_boolean(&mut self) {
        match self.settings_selected_field {
            SettingsField::SoundEnabled => {
                self.sound_enabled = !self.sound_enabled;
                self.save_config();
            }
            SettingsField::AutoStartNext => {
                self.auto_start_next = !self.auto_start_next;
                self.save_config();
            }
            _ => {}
        }
    }
    
    fn settings_cycle_theme(&mut self, forward: bool) {
        if self.settings_selected_field == SettingsField::Theme {
            let themes = ["default", "nord", "dracula", "gruvbox", "solarized"];
            let current_idx = themes.iter().position(|&t| t == self.theme_name).unwrap_or(0);
            
            let new_idx = if forward {
                (current_idx + 1) % themes.len()
            } else {
                if current_idx == 0 { themes.len() - 1 } else { current_idx - 1 }
            };
            
            self.theme_name = themes[new_idx].to_string();
            self.theme = get_theme(&self.theme_name);
            self.save_config();
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
            extended_break_reminder_hours: self.extended_break_reminder_hours,
        };
        let _ = save_config(&get_config_path(), &config);
    }
    
    // ========================================================================
    // Enhanced Notes Management
    // ========================================================================
    
    fn start_note(&mut self) {
        self.notes_mode = NotesMode::Adding;
        self.notes_input.clear();
    }
    
    fn save_note(&mut self) {
        if !self.notes_input.trim().is_empty() {
            let now = chrono::Local::now();
            let phase_str = match self.phase {
                Phase::Work => "work",
                Phase::ShortBreak => "short_break",
                Phase::LongBreak => "long_break",
            };
            
            self.stats.notes.push(Note {
                timestamp: now.to_rfc3339(),
                content: self.notes_input.trim().to_string(),
                phase: phase_str.to_string(),
            });
            
            self.mark_stats_dirty();
            self.selected_note_index = Some(self.stats.notes.len() - 1);
        }
        
        self.notes_mode = NotesMode::Viewing;
        self.notes_input.clear();
    }
    
    fn cancel_note(&mut self) {
        self.notes_mode = NotesMode::Viewing;
        self.notes_input.clear();
    }
    
    fn start_edit_note(&mut self) {
        if let Some(idx) = self.selected_note_index {
            if idx < self.stats.notes.len() {
                self.notes_input = self.stats.notes[idx].content.clone();
                self.notes_mode = NotesMode::Editing;
            }
        }
    }
    
    fn save_edited_note(&mut self) {
        if let Some(idx) = self.selected_note_index {
            if idx < self.stats.notes.len() && !self.notes_input.trim().is_empty() {
                self.stats.notes[idx].content = self.notes_input.trim().to_string();
                self.mark_stats_dirty();
            }
        }
        self.notes_mode = NotesMode::Viewing;
        self.notes_input.clear();
    }
    
    fn confirm_delete_note(&mut self) {
        if self.selected_note_index.is_some() {
            self.notes_mode = NotesMode::ConfirmingDelete;
        }
    }
    
    fn delete_selected_note(&mut self) {
        if let Some(idx) = self.selected_note_index {
            if idx < self.stats.notes.len() {
                self.stats.notes.remove(idx);
                self.mark_stats_dirty();
                
                // Update selection
                if self.stats.notes.is_empty() {
                    self.selected_note_index = None;
                } else if idx >= self.stats.notes.len() {
                    self.selected_note_index = Some(self.stats.notes.len() - 1);
                }
            }
        }
        self.notes_mode = NotesMode::Viewing;
    }
    
    fn cancel_delete(&mut self) {
        self.notes_mode = NotesMode::Viewing;
    }
    
    fn select_next_note(&mut self) {
        if !self.stats.notes.is_empty() {
            self.selected_note_index = Some(match self.selected_note_index {
                Some(idx) => (idx + 1).min(self.stats.notes.len() - 1),
                None => 0,
            });
        }
    }
    
    fn select_prev_note(&mut self) {
        if !self.stats.notes.is_empty() {
            self.selected_note_index = Some(match self.selected_note_index {
                Some(idx) => idx.saturating_sub(1),
                None => self.stats.notes.len() - 1,
            });
        }
    }
    
    fn scroll_notes(&mut self, delta: isize) {
        let max_notes = self.stats.notes.len();
        if max_notes > 10 {
            let new_offset = (self.note_scroll_offset as isize + delta)
                .max(0)
                .min((max_notes - 10) as isize);
            self.note_scroll_offset = new_offset as usize;
        }
    }
    
    fn mark_stats_dirty(&mut self) {
        self.needs_stats_save = true;
    }
    
    fn auto_save_if_needed(&mut self) {
        // Auto-save every 5 seconds if there are changes
        if self.needs_stats_save && self.last_auto_save.elapsed() >= Duration::from_secs(5) {
            self.save_stats_now();
            self.last_auto_save = Instant::now();
        }
    }
    
    fn save_stats_now(&mut self) {
        if let Ok(()) = save_stats(&get_stats_path(), &self.stats) {
            self.needs_stats_save = false;
        }
    }
    
    fn save_on_quit(&mut self) {
        // Save stats
        self.save_stats_now();
        
        // Save timer state for resume
        let _ = save_timer_state(self);
    }
}

// ============================================================================
// Main & Event Loop
// ============================================================================

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse CLI args
    let args = Args::parse();
    
    // Load configuration
    let config_path = get_config_path();
    let mut config = load_config(&config_path);
    
    // Override config with CLI args if provided
    if let Some(work) = args.work {
        config.work_duration = work;
    }
    if let Some(rest) = args.rest {
        config.rest_duration = rest;
    }
    if let Some(long_break) = args.long_break {
        config.long_break_duration = long_break;
    }
    if let Some(sessions) = args.sessions {
        config.sessions_before_long_break = sessions;
    }
    if let Some(theme) = args.theme {
        config.theme = theme;
    }
    if args.no_sound {
        config.sound_enabled = false;
    }
    
    // Load statistics
    let stats_path = get_stats_path();
    let mut stats = load_stats(&stats_path);
    reset_daily_stats_if_needed(&mut stats);
    
    // Create merged settings
    let settings = MergedSettings {
        work_duration: config.work_duration,
        rest_duration: config.rest_duration,
        long_break_duration: config.long_break_duration,
        sessions_before_long_break: config.sessions_before_long_break,
    };
    
    // Get theme
    let theme = get_theme(&config.theme);
    
    // Create app state
    let mut app = if args.resume {
        if let Some(saved_state) = load_timer_state() {
            create_app_state_from_saved(saved_state, settings, stats, theme, config.sound_enabled, config.theme, config.auto_start_next, config.extended_break_reminder_hours)
        } else {
            create_app_state_from_args(settings, stats, theme, config.sound_enabled, config.theme, config.auto_start_next, config.extended_break_reminder_hours)
        }
    } else {
        create_app_state_from_args(settings, stats, theme, config.sound_enabled, config.theme, config.auto_start_next, config.extended_break_reminder_hours)
    };
    
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Run app
    let res = run_app(&mut terminal, &mut app);

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        println!("Error: {:?}", err);
    }

    Ok(())
}

fn run_app(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut AppState) -> io::Result<()> {
    let mut last_tick = Instant::now();
    let tick_rate = Duration::from_millis(50);

    loop {
        terminal.draw(|f| render_ui(f, app))?;
        
        // Auto-save periodically
        app.auto_save_if_needed();

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if handle_key_event(key, app) {
                    // Save before quitting
                    app.save_on_quit();
                    return Ok(());
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            update_timer(app);
            last_tick = Instant::now();
        }
    }
}

fn handle_key_event(key: event::KeyEvent, app: &mut AppState) -> bool {
    // Handle notes editing/adding mode
    if app.notes_mode == NotesMode::Adding || app.notes_mode == NotesMode::Editing {
        match key.code {
            KeyCode::Char(c) => {
                app.notes_input.push(c);
            }
            KeyCode::Backspace => {
                app.notes_input.pop();
            }
            KeyCode::Enter => {
                if app.notes_mode == NotesMode::Editing {
                    app.save_edited_note();
                } else {
                    app.save_note();
                }
            }
            KeyCode::Esc => {
                app.cancel_note();
            }
            _ => {}
        }
        return false;
    }
    
    // Handle delete confirmation mode
    if app.notes_mode == NotesMode::ConfirmingDelete {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                app.delete_selected_note();
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                app.cancel_delete();
            }
            _ => {}
        }
        return false;
    }
    
    // Handle notes view navigation
    if app.current_view == View::Notes {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('t') => {
                app.current_view = View::Timer;
                app.notes_mode = NotesMode::Viewing;
            }
            KeyCode::Char('a') | KeyCode::Char('n') => {
                app.start_note();
            }
            KeyCode::Char('e') => {
                app.start_edit_note();
            }
            KeyCode::Char('d') => {
                app.confirm_delete_note();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                app.select_next_note();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                app.select_prev_note();
            }
            KeyCode::PageUp => {
                app.scroll_notes(-5);
            }
            KeyCode::PageDown => {
                app.scroll_notes(5);
            }
            _ => {}
        }
        return false;
    }
    
    // Handle settings editing mode
    if app.settings_editing {
        match key.code {
            KeyCode::Char(c) => {
                app.settings_input.push(c);
            }
            KeyCode::Backspace => {
                app.settings_input.pop();
            }
            KeyCode::Enter => {
                app.settings_apply_change();
            }
            KeyCode::Esc => {
                app.settings_cancel_editing();
            }
            _ => {}
        }
        return false;
    }
    
    // Handle settings menu navigation
    if app.current_view == View::Settings {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('c') => {
                app.current_view = View::Timer;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                app.settings_next_field();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                app.settings_prev_field();
            }
            KeyCode::Enter | KeyCode::Char('e') => {
                app.settings_start_editing();
            }
            KeyCode::Char(' ') => {
                app.settings_toggle_boolean();
            }
            KeyCode::Left | KeyCode::Char('h') => {
                if app.settings_selected_field == SettingsField::Theme {
                    app.settings_cycle_theme(false);
                }
            }
            KeyCode::Right | KeyCode::Char('l') => {
                if app.settings_selected_field == SettingsField::Theme {
                    app.settings_cycle_theme(true);
                }
            }
            _ => {}
        }
        return false;
    }

    // Handle normal key events
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => return true,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return true,
        
        // Minimize toggle (works in any view)
        KeyCode::Char('m') | KeyCode::Char('M') => {
            app.toggle_minimize();
            return false;
        }
        
        _ => {}
    }
    
    // If minimized, don't process other keys except minimize toggle
    if app.minimized {
        return false;
    }
    
    // Continue with normal key handling
    match key.code {
        // Timer controls
        KeyCode::Char(' ') => app.toggle_pause(),
        KeyCode::Char('r') => app.reset_timer(),
        KeyCode::Char('n') => app.next_phase(),
        KeyCode::Char('d') => app.open_settings(),
        
        // View navigation
        KeyCode::Char('t') => app.open_notes(),
        KeyCode::Char('h') | KeyCode::Char('?') => {
            app.current_view = if app.current_view == View::Help {
                View::Timer
            } else {
                View::Help
            };
        }
        KeyCode::Char('s') => {
            app.current_view = match app.current_view {
                View::Timer => View::StatsSummary,
                _ => View::Timer,
            };
        }
        KeyCode::Tab => {
            app.current_view = match app.current_view {
                View::StatsSummary => View::StatsDetailed,
                View::StatsDetailed => View::StatsHistory,
                View::StatsHistory => View::StatsSummary,
                _ => app.current_view.clone(),
            };
        }
        KeyCode::Char('e') => {
            if matches!(app.current_view, View::StatsSummary | View::StatsDetailed | View::StatsHistory) {
                let _ = export_stats_csv(&app.stats, &get_stats_path());
            }
        }
        _ => {}
    }
    
    false
}

fn update_timer(app: &mut AppState) {
    if !app.paused && app.time_remaining > Duration::from_secs(0) {
        app.time_remaining = app.time_remaining.saturating_sub(Duration::from_millis(50));
        
        // Check for extended break reminder during work sessions
        if app.phase == Phase::Work && !app.paused {
            app.check_extended_break_reminder();
        }
        
        if app.time_remaining.as_secs() == 0 {
            app.next_phase();
        }
    }
    
    app.animation_frame = (app.animation_frame + 1) % 20;
}

// ============================================================================
// UI Rendering
// ============================================================================

fn render_ui(f: &mut Frame, app: &AppState) {
    if app.minimized {
        render_minimized_view(f, app);
    } else {
        match app.current_view {
            View::Timer => render_timer_view(f, app),
            View::Help => render_help_view(f, app),
            View::StatsSummary => render_stats_summary(f, app),
            View::StatsDetailed => render_stats_detailed(f, app),
            View::StatsHistory => render_stats_history(f, app),
            View::Settings => render_settings_view(f, app),
            View::Notes => render_notes_view(f, app),
        }
    }
}

fn render_minimized_view(f: &mut Frame, app: &AppState) {
    let area = f.size();
    
    // Create a small centered box
    let mini_area = centered_rect(40, 30, area);
    
    let secs = app.time_remaining.as_secs();
    let mins = secs / 60;
    let secs = secs % 60;
    let time_str = format!("{:02}:{:02}", mins, secs);
    
    let status = if app.paused { "⏸ PAUSED" } else { "▶ RUNNING" };
    
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(app.phase_name(), Style::default()
            .fg(app.phase_color())
            .add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(Span::styled(time_str, Style::default()
            .fg(app.phase_color())
            .add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(Span::styled(status, Style::default()
            .fg(if app.paused { Color::Yellow } else { Color::Green }))),
        Line::from(""),
        Line::from(""),
        Line::from(Span::styled("Press M to restore", Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC))),
    ];
    
    let widget = Paragraph::new(lines)
        .alignment(Alignment::Center)
        .block(Block::default()
            .title(" 🍅 RTIMER (Minimized) ")
            .title_alignment(Alignment::Center)
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(app.theme.border_color)));
    
    f.render_widget(widget, mini_area);
}

fn render_timer_view(f: &mut Frame, app: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Header
            Constraint::Min(1),     // Main content
            Constraint::Length(3),  // Footer
        ])
        .split(f.size());
    
    // Header
    let header = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(app.theme.border_color))
        .title(Span::styled(" 🍅 RTIMER ", Style::default()
            .fg(app.theme.accent_color)
            .add_modifier(Modifier::BOLD)));
    f.render_widget(header, chunks[0]);
    
    // Main content area with centered timer
    let main_area = chunks[1];
    let vertical_sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(10),
            Constraint::Length(3),    // Phase name
            Constraint::Length(1),
            Constraint::Length(5),    // Timer
            Constraint::Length(1),
            Constraint::Length(2),    // Date/time
            Constraint::Length(1),
            Constraint::Length(2),    // Status
            Constraint::Length(1),
            Constraint::Length(3),    // Progress bar
            Constraint::Length(1),
            Constraint::Length(2),    // Session info
            Constraint::Percentage(10),
        ])
        .split(main_area);
    
    let sections = vertical_sections;
    
    // Phase name
    let phase_text = Paragraph::new(app.phase_name())
        .style(Style::default()
            .fg(app.phase_color())
            .add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center);
    f.render_widget(phase_text, sections[1]);
    
    // Timer display
    let secs = app.time_remaining.as_secs();
    let mins = secs / 60;
    let secs = secs % 60;
    let time_str = format!("{:02}:{:02}", mins, secs);
    
    let timer_text = Paragraph::new(time_str)
        .style(Style::default()
            .fg(app.phase_color())
            .add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center)
        .block(Block::default());
    f.render_widget(timer_text, sections[3]);
    
    // Date and time display
    let now = chrono::Local::now();
    let date_str = now.format("%A, %B %d, %Y").to_string();
    let time_str = now.format("%I:%M %p").to_string();
    
    let date_lines = vec![
        Line::from(vec![
            Span::styled(date_str, Style::default().fg(Color::Gray)),
        ]),
        Line::from(vec![
            Span::styled(time_str, Style::default().fg(Color::DarkGray)),
        ]),
    ];
    
    let date_widget = Paragraph::new(date_lines)
        .alignment(Alignment::Center);
    f.render_widget(date_widget, sections[5]);
    
    // Status with animation
    let status = if app.paused {
        let dots = ".".repeat((app.animation_frame / 5) as usize % 4);
        format!("⏸  PAUSED{}", dots)
    } else {
        let pulse = if app.animation_frame < 10 { "●" } else { "○" };
        format!("{} RUNNING", pulse)
    };
    
    let status_text = Paragraph::new(status)
        .style(Style::default()
            .fg(if app.paused { Color::Yellow } else { Color::Green })
            .add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center);
    f.render_widget(status_text, sections[7]);
    
    // Progress bar
    let progress = Gauge::default()
        .block(Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded))
        .gauge_style(Style::default()
            .fg(app.phase_color())
            .bg(Color::Black))
        .percent((app.progress_ratio() * 100.0) as u16);
    f.render_widget(progress, sections[9]);
    
    // Session info
    let session_text = format!(
        "Session {} of {}  •  {} completed today",
        ((app.session_count - 1) % app.sessions_before_long_break) + 1,
        app.sessions_before_long_break,
        app.stats.sessions_today
    );
    
    let session_info = Paragraph::new(session_text)
        .style(Style::default().fg(Color::Gray))
        .alignment(Alignment::Center);
    f.render_widget(session_info, sections[11]);
    
    // Controls help
    let controls = vec![
        Line::from(vec![
            Span::styled("Space", Style::default().fg(app.theme.accent_color).add_modifier(Modifier::BOLD)),
            Span::raw(" Pause/Resume  •  "),
            Span::styled("R", Style::default().fg(app.theme.accent_color).add_modifier(Modifier::BOLD)),
            Span::raw(" Reset  •  "),
            Span::styled("N", Style::default().fg(app.theme.accent_color).add_modifier(Modifier::BOLD)),
            Span::raw(" Skip  •  "),
            Span::styled("M", Style::default().fg(app.theme.accent_color).add_modifier(Modifier::BOLD)),
            Span::raw(" Minimize"),
        ]),
        Line::from(vec![
            Span::styled("T", Style::default().fg(app.theme.accent_color).add_modifier(Modifier::BOLD)),
            Span::raw(" Notes  •  "),
            Span::styled("S", Style::default().fg(app.theme.accent_color).add_modifier(Modifier::BOLD)),
            Span::raw(" Stats  •  "),
            Span::styled("D", Style::default().fg(app.theme.accent_color).add_modifier(Modifier::BOLD)),
            Span::raw(" Settings  •  "),
            Span::styled("H", Style::default().fg(app.theme.accent_color).add_modifier(Modifier::BOLD)),
            Span::raw(" Help  •  "),
            Span::styled("Q", Style::default().fg(app.theme.accent_color).add_modifier(Modifier::BOLD)),
            Span::raw(" Quit"),
        ]),
    ];
    
    let controls_widget = Paragraph::new(controls)
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::DarkGray));
    f.render_widget(controls_widget, chunks[2]);
}

fn render_help_view(f: &mut Frame, app: &AppState) {
    let area = centered_rect(70, 85, f.size());
    
    let help_text = vec![
        Line::from(""),
        Line::from(Span::styled("⌨️  KEYBOARD SHORTCUTS", Style::default().fg(app.theme.accent_color).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from("  Timer Controls:"),
        Line::from(vec![
            Span::raw("    "),
            Span::styled("Space", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::raw("  Toggle pause/resume"),
        ]),
        Line::from(vec![
            Span::raw("    "),
            Span::styled("R", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::raw("      Reset current timer"),
        ]),
        Line::from(vec![
            Span::raw("    "),
            Span::styled("N", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::raw("      Skip to next phase"),
        ]),
        Line::from(vec![
            Span::raw("    "),
            Span::styled("M", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::raw("      Minimize to compact view"),
        ]),
        Line::from(""),
        Line::from("  Navigation:"),
        Line::from(vec![
            Span::raw("    "),
            Span::styled("T", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::raw("      Open notes view"),
        ]),
        Line::from(vec![
            Span::raw("    "),
            Span::styled("S", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::raw("      Open statistics"),
        ]),
        Line::from(vec![
            Span::raw("    "),
            Span::styled("D", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::raw("      Open settings"),
        ]),
        Line::from(vec![
            Span::raw("    "),
            Span::styled("H / ?", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::raw("   Toggle help"),
        ]),
        Line::from(vec![
            Span::raw("    "),
            Span::styled("Tab", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::raw("     Cycle through stat views"),
        ]),
        Line::from(""),
        Line::from("  Notes View (Enhanced):"),
        Line::from(vec![
            Span::raw("    "),
            Span::styled("A / N", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw("   Add new note"),
        ]),
        Line::from(vec![
            Span::raw("    "),
            Span::styled("E", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw("      Edit selected note"),
        ]),
        Line::from(vec![
            Span::raw("    "),
            Span::styled("D", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw("      Delete selected note (with confirmation)"),
        ]),
        Line::from(vec![
            Span::raw("    "),
            Span::styled("↑↓ / JK", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(" Navigate between notes"),
        ]),
        Line::from(""),
        Line::from("  General:"),
        Line::from(vec![
            Span::raw("    "),
            Span::styled("Q / Esc", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
            Span::raw(" Exit / Go back"),
        ]),
        Line::from(vec![
            Span::raw("    "),
            Span::styled("Ctrl+C", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
            Span::raw("  Force quit"),
        ]),
        Line::from(""),
        Line::from(""),
        Line::from(Span::styled("💡 TIPS:", Style::default().fg(Color::Green).add_modifier(Modifier::ITALIC))),
        Line::from(Span::styled("    • All data is saved automatically!", Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC))),
        Line::from(Span::styled("    • Enable Auto-Start in Settings for continuous flow", Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC))),
        Line::from(Span::styled("    • Extended break reminders help prevent burnout", Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC))),
    ];
    
    let widget = Paragraph::new(help_text)
        .alignment(Alignment::Left)
        .block(Block::default()
            .title(" Help ")
            .title_alignment(Alignment::Center)
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(app.theme.border_color)));
    
    f.render_widget(widget, area);
}

fn render_stats_summary(f: &mut Frame, app: &AppState) {
    let area = centered_rect(70, 85, f.size());
    
    let total_time_hours = app.stats.total_work_time as f64 / 60.0;
    let break_time_hours = app.stats.total_break_time as f64 / 60.0;
    
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled("📊 STATISTICS OVERVIEW", Style::default().fg(app.theme.accent_color).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(Span::styled("  Press Tab to cycle through detailed views  •  E to export CSV", Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC))),
        Line::from(""),
        Line::from(""),
        Line::from(vec![
            Span::styled("  📅 Today:", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::raw("     Sessions completed: "),
            Span::styled(format!("{}", app.stats.sessions_today), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  📈 All Time:", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::raw("     Total sessions: "),
            Span::styled(format!("{}", app.stats.total_sessions), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::raw("     Total focus time: "),
            Span::styled(format!("{:.1} hours", total_time_hours), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::raw("     Total break time: "),
            Span::styled(format!("{:.1} hours", break_time_hours), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  📝 Notes:", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::raw("     Total notes: "),
            Span::styled(format!("{}", app.stats.notes.len()), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        ]),
    ];
    
    let widget = Paragraph::new(lines)
        .alignment(Alignment::Left)
        .block(Block::default()
            .title(" Statistics (Press s to open) ")
            .title_alignment(Alignment::Center)
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(app.theme.border_color)));
    
    f.render_widget(widget, area);
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
    let max_sessions = *app.stats.weekly_sessions.iter().max().unwrap_or(&1).max(&1);
    
    for (i, &count) in app.stats.weekly_sessions.iter().enumerate() {
        let bar_width = if max_sessions > 0 {
            (count as f64 / max_sessions as f64 * 30.0) as usize
        } else {
            0
        };
        let bar = "█".repeat(bar_width);
        
        lines.push(Line::from(vec![
            Span::styled(format!("  {} ", days[i]), Style::default().fg(Color::Gray)),
            Span::styled(bar, Style::default().fg(app.theme.accent_color)),
            Span::raw(format!(" {}", count)),
        ]));
    }
    
    let widget = Paragraph::new(lines)
        .alignment(Alignment::Left)
        .block(Block::default()
            .title(" Weekly Stats (Tab to cycle) ")
            .title_alignment(Alignment::Center)
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(app.theme.border_color)));
    
    f.render_widget(widget, area);
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
        for session in app.stats.session_history.iter().rev().take(15) {
            let datetime = session.timestamp.split('T')
                .next()
                .and_then(|d| {
                    let time = session.timestamp.split('T').nth(1)?.split('.').next()?;
                    Some(format!("{} {}", d, &time[..5]))
                })
                .unwrap_or_else(|| "Unknown".to_string());
            
            let icon = match session.phase_type.as_str() {
                "Work" => "🎯",
                "Short Break" => "☕",
                "Long Break" => "🌴",
                _ => "📝",
            };
            
            let status_icon = if session.completed { "✓" } else { "⏸" };
            let status_color = if session.completed { Color::Green } else { Color::Yellow };
            
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(icon, Style::default()),
                Span::raw(" "),
                Span::styled(datetime, Style::default().fg(Color::Gray)),
                Span::raw(" • "),
                Span::styled(&session.phase_type, Style::default().fg(Color::White)),
                Span::raw(" • "),
                Span::styled(format!("{}m", session.duration), Style::default().fg(Color::Cyan)),
                Span::raw(" "),
                Span::styled(status_icon, Style::default().fg(status_color)),
            ]));
        }
    }
    
    let widget = Paragraph::new(lines)
        .alignment(Alignment::Left)
        .block(Block::default()
            .title(" Session History (Tab to cycle) ")
            .title_alignment(Alignment::Center)
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(app.theme.border_color)));
    
    f.render_widget(widget, area);
}

fn render_settings_view(f: &mut Frame, app: &AppState) {
    let area = centered_rect(70, 85, f.size());
    
    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled("⚙️  SETTINGS", Style::default().fg(app.theme.accent_color).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(Span::styled("  Use ↑↓/jk to navigate  •  Enter to edit  •  Space to toggle  •  ←→/hl for themes  •  d/Esc to close", Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC))),
        Line::from(""),
    ];
    
    let fields = vec![
        (SettingsField::WorkDuration, "🎯 Work Duration", {
            let mins = app.work_duration.as_secs_f64() / 60.0;
            if mins.fract() == 0.0 {
                format!("{} minutes", mins as u64)
            } else {
                format!("{:.2} minutes", mins)
            }
        }),
        (SettingsField::RestDuration, "☕ Rest Duration", {
            let mins = app.rest_duration.as_secs_f64() / 60.0;
            if mins.fract() == 0.0 {
                format!("{} minutes", mins as u64)
            } else {
                format!("{:.2} minutes", mins)
            }
        }),
        (SettingsField::LongBreakDuration, "🌴 Long Break Duration", {
            let mins = app.long_break_duration.as_secs_f64() / 60.0;
            if mins.fract() == 0.0 {
                format!("{} minutes", mins as u64)
            } else {
                format!("{:.2} minutes", mins)
            }
        }),
        (SettingsField::SessionsBeforeLongBreak, "🔄 Sessions Before Long Break", format!("{} sessions", app.sessions_before_long_break)),
        (SettingsField::Theme, "🎨 Theme", format!("< {} >", app.theme_name)),
        (SettingsField::SoundEnabled, "🔔 Sound Notifications", if app.sound_enabled { "ON" } else { "OFF" }.to_string()),
        (SettingsField::AutoStartNext, "▶️  Auto-Start Next Phase", if app.auto_start_next { "ON" } else { "OFF" }.to_string()),
        (SettingsField::ExtendedBreakReminder, "⏰ Extended Break Reminder", {
            let hours = app.extended_break_reminder_hours;
            if hours.fract() == 0.0 {
                format!("After {} hours", hours as u64)
            } else {
                format!("After {:.1} hours", hours)
            }
        }),
    ];
    
    for (field, label, value) in fields.iter() {
        let is_selected = app.settings_selected_field == *field;
        let is_editing = is_selected && app.settings_editing;
        
        lines.push(Line::from(""));
        
        if is_editing {
            // Show editing mode
            lines.push(Line::from(vec![
                Span::styled("  > ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                Span::styled(*label, Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            ]));
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(&app.settings_input, Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                Span::styled("█", Style::default().fg(Color::Green)),
            ]));
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled("Enter: Save  •  Esc: Cancel", Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC)),
            ]));
        } else if is_selected {
            // Show selected
            lines.push(Line::from(vec![
                Span::styled("  > ", Style::default().fg(app.theme.accent_color).add_modifier(Modifier::BOLD)),
                Span::styled(*label, Style::default().fg(app.theme.accent_color).add_modifier(Modifier::BOLD)),
            ]));
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(value.as_str(), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            ]));
            
            // Add context hints
            let hint = match field {
                SettingsField::WorkDuration | SettingsField::RestDuration | SettingsField::LongBreakDuration => 
                    "Press Enter to edit duration (supports decimals: 0.5 = 30s, 0.1 = 6s)",
                SettingsField::SessionsBeforeLongBreak => 
                    "Press Enter to edit count (1-10 sessions)",
                SettingsField::Theme => 
                    "Use ←→ arrows or h/l to cycle themes",
                SettingsField::SoundEnabled | SettingsField::AutoStartNext => 
                    "Press Space to toggle",
                SettingsField::ExtendedBreakReminder =>
                    "Press Enter to edit (0.5-8.0 hours, 0 to disable)",
            };
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(hint, Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC)),
            ]));
        } else {
            // Show unselected
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(*label, Style::default().fg(Color::Gray)),
            ]));
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(value.as_str(), Style::default().fg(Color::DarkGray)),
            ]));
        }
    }
    
    lines.push(Line::from(""));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  💾 All changes are saved automatically!", Style::default().fg(Color::Green).add_modifier(Modifier::ITALIC))));
    
    let widget = Paragraph::new(lines)
        .alignment(Alignment::Left)
        .block(Block::default()
            .title(" Settings (Press d to open) ")
            .title_alignment(Alignment::Center)
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(app.theme.border_color)));
    
    f.render_widget(widget, area);
}

fn render_notes_view(f: &mut Frame, app: &AppState) {
    let area = centered_rect(80, 85, f.size());
    
    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled("📝 BREAK NOTES", Style::default().fg(app.theme.accent_color).add_modifier(Modifier::BOLD))),
        Line::from(""),
    ];
    
    // Show different help text based on mode
    match app.notes_mode {
        NotesMode::Viewing => {
            lines.push(Line::from(Span::styled(
                "  a/n: Add  •  e: Edit  •  d: Delete  •  ↑↓/jk: Navigate  •  t/Esc: Close", 
                Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC)
            )));
        }
        NotesMode::Adding => {
            lines.push(Line::from(Span::styled(
                "  Type your note and press Enter to save  •  Esc to cancel", 
                Style::default().fg(Color::Yellow).add_modifier(Modifier::ITALIC)
            )));
        }
        NotesMode::Editing => {
            lines.push(Line::from(Span::styled(
                "  Edit your note and press Enter to save  •  Esc to cancel", 
                Style::default().fg(Color::Yellow).add_modifier(Modifier::ITALIC)
            )));
        }
        NotesMode::ConfirmingDelete => {
            lines.push(Line::from(Span::styled(
                "  Press Y to confirm deletion  •  N or Esc to cancel", 
                Style::default().fg(Color::Red).add_modifier(Modifier::ITALIC).add_modifier(Modifier::BOLD)
            )));
        }
    }
    
    lines.push(Line::from(""));
    
    // Show input area for adding/editing
    if app.notes_mode == NotesMode::Adding || app.notes_mode == NotesMode::Editing {
        let title = if app.notes_mode == NotesMode::Adding { 
            "✏️  NEW NOTE" 
        } else { 
            "✏️  EDITING NOTE" 
        };
        
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(format!("  {}", title), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        ]));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(&app.notes_input, Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::styled("█", Style::default().fg(Color::Green)),
        ]));
        lines.push(Line::from(""));
        lines.push(Line::from("  ─────────────────────────────────────────────────────────────────────────"));
        lines.push(Line::from(""));
    }
    
    // Show delete confirmation
    if app.notes_mode == NotesMode::ConfirmingDelete {
        if let Some(idx) = app.selected_note_index {
            if idx < app.stats.notes.len() {
                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::styled("  ⚠️  DELETE NOTE?", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                ]));
                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(&app.stats.notes[idx].content, Style::default().fg(Color::White)),
                ]));
                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::styled("  This action cannot be undone! Press Y to confirm, N to cancel.", 
                        Style::default().fg(Color::Red).add_modifier(Modifier::ITALIC)),
                ]));
                lines.push(Line::from(""));
                lines.push(Line::from("  ─────────────────────────────────────────────────────────────────────────"));
                lines.push(Line::from(""));
            }
        }
    }
    
    // Display existing notes
    if app.stats.notes.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("  No notes yet! ", Style::default().fg(Color::DarkGray)),
            Span::styled("Press 'a' or 'n' to add your first note.", Style::default().fg(Color::Gray)),
        ]));
        lines.push(Line::from(""));
    } else {
        let total_notes = app.stats.notes.len();
        
        lines.push(Line::from(vec![
            Span::styled(format!("  {} NOTES", total_notes), Style::default().fg(app.theme.accent_color).add_modifier(Modifier::BOLD)),
        ]));
        lines.push(Line::from(""));
        
        // Display all notes (newest first) with selection indicator
        for (idx, note) in app.stats.notes.iter().enumerate().rev() {
            let is_selected = app.selected_note_index == Some(idx);
            
            // Format datetime
            let datetime = note.timestamp.split('T')
                .next()
                .and_then(|d| {
                    let time = note.timestamp.split('T').nth(1)?.split('.').next()?;
                    Some(format!("{} {}", d, time))
                })
                .unwrap_or_else(|| "Unknown".to_string());
            
            let icon = match note.phase.as_str() {
                "work" => "🎯",
                "short_break" => "☕",
                "long_break" => "🌴",
                _ => "📝",
            };
            
            // Show selection indicator
            let prefix = if is_selected { "► " } else { "  " };
            let prefix_color = if is_selected { app.theme.accent_color } else { Color::Reset };
            
            lines.push(Line::from(vec![
                Span::styled(prefix, Style::default().fg(prefix_color).add_modifier(Modifier::BOLD)),
                Span::styled(icon, Style::default()),
                Span::raw("  "),
                Span::styled(
                    datetime.clone(), 
                    if is_selected { 
                        Style::default().fg(Color::White) 
                    } else { 
                        Style::default().fg(Color::DarkGray) 
                    }
                ),
            ]));
            
            // Wrap long notes
            let max_width = 70;
            let content_prefix = if is_selected { "     " } else { "     " };
            
            if note.content.len() > max_width {
                let mut remaining = note.content.as_str();
                while !remaining.is_empty() {
                    let chunk_end = remaining.char_indices()
                        .nth(max_width)
                        .map(|(i, _)| i)
                        .unwrap_or(remaining.len());
                    let chunk = &remaining[..chunk_end];
                    lines.push(Line::from(vec![
                        Span::raw(content_prefix),
                        Span::styled(
                            chunk.to_string(), 
                            if is_selected {
                                Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
                            } else {
                                Style::default().fg(Color::White)
                            }
                        ),
                    ]));
                    remaining = &remaining[chunk_end..];
                }
            } else {
                lines.push(Line::from(vec![
                    Span::raw(content_prefix),
                    Span::styled(
                        note.content.clone(), 
                        if is_selected {
                            Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::White)
                        }
                    ),
                ]));
            }
            lines.push(Line::from(""));
        }
    }
    
    let widget = Paragraph::new(lines)
        .alignment(Alignment::Left)
        .block(Block::default()
            .title(" Notes (Press t to open) ")
            .title_alignment(Alignment::Center)
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(app.theme.border_color)));
    
    f.render_widget(widget, area);
}

// ============================================================================
// Helper Functions
// ============================================================================

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

fn send_notification(title: &str, body: &str, sound_enabled: bool) {
    let _ = Notification::new()
        .summary(title)
        .body(body)
        .appname("rtimer")
        .icon("alarm-clock")
        .urgency(Urgency::Critical)
        .show();

    if sound_enabled {
        play_sound();
    }
}

fn play_sound() {
    std::thread::spawn(|| {
        let sound_commands = [
            ("paplay", "/usr/share/sounds/freedesktop/stereo/complete.oga"),
            ("aplay", "/usr/share/sounds/sound-icons/guitar-11.wav"),
            ("aplay", "/usr/share/sounds/generic.wav"),
        ];

        for (cmd, sound_file) in sound_commands.iter() {
            if std::path::Path::new(sound_file).exists() {
                let _ = std::process::Command::new(cmd)
                    .arg(sound_file)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
                break;
            }
        }
    });
}

fn parse_duration_arg(s: &str) -> Result<f64, String> {
    let s = s.trim().to_lowercase();
    let mut total_minutes = 0.0;
    let mut current_num = String::new();
    
    for ch in s.chars() {
        if ch.is_ascii_digit() || ch == '.' {
            current_num.push(ch);
        } else if ch == 'h' {
            if let Ok(hours) = current_num.parse::<f64>() {
                total_minutes += hours * 60.0;
                current_num.clear();
            } else {
                return Err("Invalid hour format".to_string());
            }
        } else if ch == 'm' {
            if let Ok(mins) = current_num.parse::<f64>() {
                total_minutes += mins;
                current_num.clear();
            } else {
                return Err("Invalid minute format".to_string());
            }
        } else if ch == 's' {
            if let Ok(secs) = current_num.parse::<f64>() {
                total_minutes += secs / 60.0;
                current_num.clear();
            } else {
                return Err("Invalid second format".to_string());
            }
        }
    }
    
    if total_minutes > 0.0 {
        Ok(total_minutes)
    } else {
        Err("Invalid duration format. Use: 25m, 1h30m, 90m, 1.5h, 0.5m".to_string())
    }
}

// ============================================================================
// Theme System
// ============================================================================

fn get_theme(name: &str) -> Theme {
    match name.to_lowercase().as_str() {
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

fn is_valid_theme(name: &str) -> bool {
    matches!(name, "default" | "nord" | "dracula" | "gruvbox" | "solarized")
}

// ============================================================================
// File I/O
// ============================================================================

fn get_config_path() -> PathBuf {
    let mut path = PathBuf::from(".");
    path.push("rtimer");
    fs::create_dir_all(&path).ok();
    path.push("config.json");
    path
}

fn get_stats_path() -> PathBuf {
    let mut path = PathBuf::from(".");
    path.push("rtimer");
    fs::create_dir_all(&path).ok();
    path.push("stats.json");
    path
}

fn load_config(path: &PathBuf) -> Config {
    if let Ok(contents) = fs::read_to_string(path) {
        serde_json::from_str(&contents).unwrap_or_default()
    } else {
        let config = Config::default();
        let _ = save_config(path, &config);
        config
    }
}

fn save_config(path: &PathBuf, config: &Config) -> io::Result<()> {
    let json = serde_json::to_string_pretty(config)?;
    fs::write(path, json)?;
    Ok(())
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

fn load_timer_state() -> Option<TimerState> {
    let mut path = get_stats_path();
    path.set_file_name("timer_state.json");
    
    if let Ok(contents) = fs::read_to_string(&path) {
        serde_json::from_str(&contents).ok()
    } else {
        None
    }
}

fn save_timer_state(state: &AppState) -> io::Result<()> {
    let mut path = get_stats_path();
    path.set_file_name("timer_state.json");
    
    let timer_state = TimerState {
        time_remaining_secs: state.time_remaining.as_secs(),
        phase: match state.phase {
            Phase::Work => "work",
            Phase::ShortBreak => "short_break",
            Phase::LongBreak => "long_break",
        }.to_string(),
        session_count: state.session_count,
        paused: state.paused,
    };
    
    let json = serde_json::to_string(&timer_state)?;
    fs::write(path, json)?;
    Ok(())
}

fn export_stats_csv(stats: &Statistics, base_path: &PathBuf) -> io::Result<()> {
    let mut path = base_path.clone();
    path.set_file_name("stats_export.csv");
    
    let mut csv = String::from("Date,Total Sessions,Sessions Today,Work Time (hours),Break Time (hours)\n");
    csv.push_str(&format!(
        "{},{},{},{:.2},{:.2}\n",
        stats.last_session_date,
        stats.total_sessions,
        stats.sessions_today,
        stats.total_work_time as f64 / 60.0,
        stats.total_break_time as f64 / 60.0
    ));
    
    csv.push_str("\n\nSession History\n");
    csv.push_str("Timestamp,Phase,Duration (min),Completed\n");
    
    for session in stats.session_history.iter().rev().take(50) {
        csv.push_str(&format!(
            "{},{},{},{}\n",
            session.timestamp,
            session.phase_type,
            session.duration,
            if session.completed { "Yes" } else { "No" }
        ));
    }
    
    csv.push_str("\n\nNotes\n");
    csv.push_str("Timestamp,Phase,Content\n");
    
    for note in stats.notes.iter().rev() {
        // Escape content for CSV (quote if contains comma or quote)
        let content = if note.content.contains(',') || note.content.contains('"') {
            format!("\"{}\"", note.content.replace('"', "\"\""))
        } else {
            note.content.clone()
        };
        
        csv.push_str(&format!(
            "{},{},{}\n",
            note.timestamp,
            note.phase,
            content
        ));
    }
    
    fs::write(path, csv)?;
    Ok(())
}

// ============================================================================
// State Management
// ============================================================================

fn reset_daily_stats_if_needed(stats: &mut Statistics) {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    if stats.last_session_date != today {
        stats.sessions_today = 0;
        stats.last_session_date = today;
        stats.weekly_sessions.rotate_left(1);
        stats.weekly_sessions[6] = 0;
    }
}

fn create_app_state_from_args(settings: MergedSettings, stats: Statistics, theme: Theme, sound_enabled: bool, theme_name: String, auto_start_next: bool, extended_break_reminder_hours: f64) -> AppState {
    let selected_note = if !stats.notes.is_empty() {
        Some(stats.notes.len() - 1)
    } else {
        None
    };
    
    AppState {
        time_remaining: Duration::from_secs_f64(settings.work_duration * 60.0),
        paused: false,
        session_count: 1,
        phase: Phase::Work,
        work_duration: Duration::from_secs_f64(settings.work_duration * 60.0),
        rest_duration: Duration::from_secs_f64(settings.rest_duration * 60.0),
        long_break_duration: Duration::from_secs_f64(settings.long_break_duration * 60.0),
        sessions_before_long_break: settings.sessions_before_long_break,
        stats,
        current_view: View::Timer,
        theme,
        sound_enabled,
        animation_frame: 0,
        minimized: false,
        settings_selected_field: SettingsField::WorkDuration,
        settings_editing: false,
        settings_input: String::new(),
        theme_name,
        notes_mode: NotesMode::Viewing,
        notes_input: String::new(),
        note_scroll_offset: 0,
        selected_note_index: selected_note,
        needs_stats_save: false,
        last_auto_save: Instant::now(),
        auto_start_next,
        extended_break_reminder_hours,
        last_extended_break_check: Instant::now(),
        total_work_time_since_break: Duration::from_secs(0),
    }
}

fn create_app_state_from_saved(saved: TimerState, settings: MergedSettings, stats: Statistics, theme: Theme, sound_enabled: bool, theme_name: String, auto_start_next: bool, extended_break_reminder_hours: f64) -> AppState {
    let phase = match saved.phase.as_str() {
        "short_break" => Phase::ShortBreak,
        "long_break" => Phase::LongBreak,
        _ => Phase::Work,
    };
    
    let selected_note = if !stats.notes.is_empty() {
        Some(stats.notes.len() - 1)
    } else {
        None
    };
    
    AppState {
        time_remaining: Duration::from_secs(saved.time_remaining_secs),
        paused: saved.paused,
        session_count: saved.session_count,
        phase,
        work_duration: Duration::from_secs_f64(settings.work_duration * 60.0),
        rest_duration: Duration::from_secs_f64(settings.rest_duration * 60.0),
        long_break_duration: Duration::from_secs_f64(settings.long_break_duration * 60.0),
        sessions_before_long_break: settings.sessions_before_long_break,
        stats,
        current_view: View::Timer,
        theme,
        sound_enabled,
        animation_frame: 0,
        minimized: false,
        settings_selected_field: SettingsField::WorkDuration,
        settings_editing: false,
        settings_input: String::new(),
        theme_name,
        notes_mode: NotesMode::Viewing,
        notes_input: String::new(),
        note_scroll_offset: 0,
        selected_note_index: selected_note,
        needs_stats_save: false,
        last_auto_save: Instant::now(),
        auto_start_next,
        extended_break_reminder_hours,
        last_extended_break_check: Instant::now(),
        total_work_time_since_break: Duration::from_secs(0),
    }
}
