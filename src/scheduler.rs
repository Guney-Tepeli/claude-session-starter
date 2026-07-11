//! Background scheduler — periodically checks /usage, sets timer, auto-starts sessions.
//!
//! Runs in a dedicated thread, communicates with the UI via `mpsc` channels.

use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};

use crate::claude_runner::ClaudeRunner;
use crate::usage_parser::{self, UsageInfo};

// ── Events (scheduler → UI) ─────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Event {
    /// /usage parsed successfully
    UsageChecked(UsageInfo),
    /// Timer target set for this time
    TimerSet(DateTime<Local>),
    /// Claude responded to our test message
    MessageSent(String),
    /// Something went wrong
    Error(String),
    /// General log line
    Log(String),
    /// Currently running /usage check
    Checking,
}

// ── Commands (UI → scheduler) ────────────────────────────────────────────────

pub enum Command {
    Start,
    Stop,
    CheckNow,
    UpdateConfig {
        claude_path: String,
        model: String,
        message: String,
        check_interval_minutes: u32,
        cooldown_seconds: u32,
    },
    Quit,
}

// ── Public handle ────────────────────────────────────────────────────────────

pub struct Scheduler {
    pub cmd_tx: mpsc::Sender<Command>,
    pub event_rx: mpsc::Receiver<Event>,
}

impl Scheduler {
    /// Spawn the background scheduler thread.
    pub fn new(
        claude_path: String,
        model: String,
        message: String,
        check_interval_minutes: u32,
        cooldown_seconds: u32,
    ) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();

        thread::spawn(move || {
            scheduler_loop(
                cmd_rx,
                event_tx,
                claude_path,
                model,
                message,
                check_interval_minutes,
                cooldown_seconds,
            );
        });

        Self { cmd_tx, event_rx }
    }
}

// ── Background loop ──────────────────────────────────────────────────────────

fn scheduler_loop(
    cmd_rx: mpsc::Receiver<Command>,
    tx: mpsc::Sender<Event>,
    mut claude_path: String,
    mut model: String,
    mut message: String,
    mut check_interval_minutes: u32,
    mut cooldown_seconds: u32,
) {
    let mut active = false;
    let mut force_check = false;
    let mut next_check = Instant::now();
    let mut timer_target: Option<DateTime<Local>> = None;

    loop {
        // ── Drain pending commands ──
        loop {
            match cmd_rx.try_recv() {
                Ok(Command::Start) => {
                    active = true;
                    next_check = Instant::now(); // check immediately
                    let _ = tx.send(Event::Log("▶ Autonomous mode started".into()));
                }
                Ok(Command::Stop) => {
                    active = false;
                    timer_target = None;
                    let _ = tx.send(Event::Log("⏹ Stopped".into()));
                }
                Ok(Command::CheckNow) => {
                    force_check = true;
                }
                Ok(Command::UpdateConfig { claude_path: cp, model: m, message: msg,
                    check_interval_minutes: ci, cooldown_seconds: cs }) => {
                    claude_path = cp;
                    model = m;
                    message = msg;
                    check_interval_minutes = ci;
                    cooldown_seconds = cs;
                    let _ = tx.send(Event::Log("✓ Configuration updated".into()));
                }
                Ok(Command::Quit) => return,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return,
            }
        }

        {
            let now_instant = Instant::now();

            // ── Periodic /usage check (Check Now works even when stopped) ──
            if force_check || (active && now_instant >= next_check) {
                force_check = false;
                let _ = tx.send(Event::Checking);
                let _ = tx.send(Event::Log("📊 Checking /usage...".into()));

                let runner = ClaudeRunner::new(&claude_path);
                match runner.check_usage() {
                    Ok(output) => {
                        if let Some(info) = usage_parser::parse_usage(&output) {
                            if let Some(reset) = info.reset_time {
                                let target =
                                    reset + chrono::Duration::seconds(cooldown_seconds as i64);
                                timer_target = Some(target);

                                let _ = tx.send(Event::UsageChecked(info));
                                let _ = tx.send(Event::TimerSet(target));
                                let _ = tx.send(Event::Log(format!(
                                    "⏰ Reset: {} → Timer: {}",
                                    reset.format("%H:%M"),
                                    target.format("%H:%M:%S")
                                )));
                            } else {
                                let _ = tx.send(Event::UsageChecked(info));
                                let _ = tx.send(Event::Log(
                                    "⚠ Could not parse reset time".into(),
                                ));
                            }
                        } else {
                            let _ = tx.send(Event::Error("Could not parse usage output".into()));
                            // Log first 200 chars of raw output for debugging
                            let preview: String = output.chars().take(200).collect();
                            let _ = tx.send(Event::Log(format!("Raw: {}", preview)));
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Event::Error(e));
                    }
                }

                next_check =
                    now_instant + Duration::from_secs(check_interval_minutes as u64 * 60);
            }

            // ── Timer check (only fires in autonomous mode) ──
            if let Some(target) = timer_target.filter(|_| active) {
                if Local::now() >= target {
                    timer_target = None;
                    let _ = tx.send(Event::Log(
                        "🤖 Session reset! Sending message...".into(),
                    ));

                    let runner = ClaudeRunner::new(&claude_path);
                    match runner.send_message(&model, &message) {
                        Ok(response) => {
                            let preview: String = response.chars().take(150).collect();
                            let _ = tx.send(Event::MessageSent(preview));
                        }
                        Err(e) => {
                            let _ = tx.send(Event::Error(e));
                        }
                    }

                    // Re-check /usage shortly after sending message
                    next_check = Instant::now() + Duration::from_secs(60);
                }
            }
        }

        // Sleep 1 second between iterations to stay light on CPU
        thread::sleep(Duration::from_secs(1));
    }
}
