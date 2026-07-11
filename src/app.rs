//! Main application — egui dark-theme UI + state management.
//!
//! Renders a compact dashboard with session status, countdown timer,
//! settings panel, and scrollable log. Communicates with the background
//! scheduler via channels.

use std::path::PathBuf;

use chrono::{DateTime, Local};
use eframe::egui;

use crate::config::Config;
use crate::scheduler::{Command, Event, Scheduler};

// ── Color constants ──────────────────────────────────────────────────────────

const ACCENT: egui::Color32 = egui::Color32::from_rgb(110, 64, 201);
const SUCCESS: egui::Color32 = egui::Color32::from_rgb(63, 185, 80);
const WARNING: egui::Color32 = egui::Color32::from_rgb(210, 153, 34);
const DANGER: egui::Color32 = egui::Color32::from_rgb(248, 81, 73);
const DIM: egui::Color32 = egui::Color32::from_rgb(139, 148, 158);
const CARD_BG: egui::Color32 = egui::Color32::from_rgb(28, 33, 40);
const BTN_GO: egui::Color32 = egui::Color32::from_rgb(35, 134, 54);

pub struct App {
    // Config
    config: Config,
    config_path: PathBuf,

    // Backend
    scheduler: Scheduler,

    // Live state
    active: bool,
    status: String,
    session_percent: Option<u32>,
    reset_time: Option<DateTime<Local>>,
    timer_target: Option<DateTime<Local>>,
    week_percent: Option<u32>,
    checking: bool,
    last_error: Option<String>,

    // Log
    log_entries: Vec<String>,

    // Editable settings (bound to UI widgets)
    edit_model: String,
    edit_message: String,
    edit_claude_path: String,
    edit_check_interval: String,
    edit_cooldown: String,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // ── Dark theme ──
        let mut visuals = egui::Visuals::dark();
        visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(22, 27, 34);
        visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(13, 17, 23);
        visuals.extreme_bg_color = egui::Color32::from_rgb(13, 17, 23);
        visuals.faint_bg_color = CARD_BG;
        cc.egui_ctx.set_visuals(visuals);

        // ── Config ──
        let config_path = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("config.json");
        let config = Config::load(&config_path);

        // ── Scheduler ──
        let scheduler = Scheduler::new(
            config.claude_path.clone(),
            config.default_model.clone(),
            config.test_message.clone(),
            config.check_interval_minutes,
            config.cooldown_seconds,
        );

        let auto_start = config.auto_start;

        let mut app = Self {
            edit_model: config.default_model.clone(),
            edit_message: config.test_message.clone(),
            edit_claude_path: config.claude_path.clone(),
            edit_check_interval: config.check_interval_minutes.to_string(),
            edit_cooldown: config.cooldown_seconds.to_string(),
            config,
            config_path,
            scheduler,
            active: false,
            status: "Idle".into(),
            session_percent: None,
            reset_time: None,
            timer_target: None,
            week_percent: None,
            checking: false,
            last_error: None,
            log_entries: Vec::new(),
        };

        if auto_start {
            app.start();
        }

        app
    }

    // ── Actions ──────────────────────────────────────────────────────────────

    fn start(&mut self) {
        self.active = true;
        self.status = "Running".into();
        self.last_error = None;
        let _ = self.scheduler.cmd_tx.send(Command::Start);
    }

    fn stop(&mut self) {
        self.active = false;
        self.status = "Stopped".into();
        let _ = self.scheduler.cmd_tx.send(Command::Stop);
    }

    fn check_now(&mut self) {
        let _ = self.scheduler.cmd_tx.send(Command::CheckNow);
    }

    fn save_config(&mut self) {
        self.config.claude_path = self.edit_claude_path.clone();
        self.config.default_model = self.edit_model.clone();
        self.config.test_message = self.edit_message.clone();
        self.config.check_interval_minutes = self.edit_check_interval.parse().unwrap_or(60);
        self.config.cooldown_seconds = self.edit_cooldown.parse().unwrap_or(60);
        self.config.auto_start = self.active;
        self.config.save(&self.config_path);

        let _ = self.scheduler.cmd_tx.send(Command::UpdateConfig {
            claude_path: self.config.claude_path.clone(),
            model: self.config.default_model.clone(),
            message: self.config.test_message.clone(),
            check_interval_minutes: self.config.check_interval_minutes,
            cooldown_seconds: self.config.cooldown_seconds,
        });
    }

    // ── Event processing ─────────────────────────────────────────────────────

    fn process_events(&mut self) {
        while let Ok(event) = self.scheduler.event_rx.try_recv() {
            match event {
                Event::UsageChecked(info) => {
                    self.session_percent = Some(info.session_percent);
                    self.reset_time = info.reset_time;
                    self.week_percent = info.week_percent;
                    self.checking = false;
                    self.status = "Running".into();
                    self.last_error = None;
                }
                Event::TimerSet(target) => {
                    self.timer_target = Some(target);
                }
                Event::MessageSent(response) => {
                    self.log(&format!("✓ Response: {}", response));
                    self.timer_target = None;
                    self.status = "Running".into();
                }
                Event::Error(err) => {
                    self.last_error = Some(err.clone());
                    self.log(&format!("⚠ {}", err));
                    self.checking = false;
                    self.status = "Error".into();
                }
                Event::Log(msg) => {
                    self.log(&msg);
                }
                Event::Checking => {
                    self.checking = true;
                    self.status = "Checking...".into();
                }
            }
        }
    }

    fn log(&mut self, msg: &str) {
        let ts = Local::now().format("%H:%M:%S");
        self.log_entries.push(format!("[{}] {}", ts, msg));
        // Keep last 200 entries
        if self.log_entries.len() > 200 {
            self.log_entries.remove(0);
        }
    }
}

// ── Drop: clean shutdown ─────────────────────────────────────────────────────

impl Drop for App {
    fn drop(&mut self) {
        let _ = self.scheduler.cmd_tx.send(Command::Quit);
    }
}

// ── UI Rendering ─────────────────────────────────────────────────────────────

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.process_events();

        // Repaint every second for timer countdown
        ctx.request_repaint_after(std::time::Duration::from_secs(1));

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;

                self.render_header(ui);
                ui.separator();
                self.render_status_card(ui);
                self.render_error(ui);
                self.render_controls(ui);
                ui.separator();
                self.render_settings(ui);
                ui.add_space(4.0);
                self.render_log(ui);
            });
        });
    }
}

impl App {
    fn render_header(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading(egui::RichText::new("⏰ Claude Timer Reset").size(20.0));
            ui.label(egui::RichText::new("v1.0 · Rust").size(11.0).color(DIM));
        });
    }

    fn render_status_card(&self, ui: &mut egui::Ui) {
        egui::Frame::group(ui.style())
            .fill(CARD_BG)
            .rounding(8.0)
            .inner_margin(14.0)
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new("📊 Session Status")
                        .size(14.0)
                        .color(ACCENT)
                        .strong(),
                );
                ui.add_space(6.0);

                // Status line
                let (icon, color) = if self.checking {
                    ("🔄", WARNING)
                } else if self.active {
                    ("🟢", SUCCESS)
                } else {
                    ("⚫", DIM)
                };
                ui.label(
                    egui::RichText::new(format!("{}  {}", icon, self.status)).color(color),
                );

                // Usage progress bar
                if let Some(pct) = self.session_percent {
                    ui.add_space(6.0);
                    let bar_color = if pct > 80 {
                        DANGER
                    } else if pct > 50 {
                        WARNING
                    } else {
                        SUCCESS
                    };
                    ui.label(
                        egui::RichText::new(format!("Usage: {}%", pct)).color(bar_color),
                    );
                    ui.add(egui::ProgressBar::new(pct as f32 / 100.0).fill(bar_color));
                }

                // Reset time countdown
                if let Some(reset) = self.reset_time {
                    let remaining = reset - Local::now();
                    let secs = remaining.num_seconds().max(0);
                    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);

                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(format!(
                        "Reset: {}  ({:02}:{:02}:{:02} remaining)",
                        reset.format("%H:%M"),
                        h,
                        m,
                        s
                    )).color(DIM));
                }

                // Timer target & big countdown
                if let Some(target) = self.timer_target {
                    let remaining = target - Local::now();
                    let secs = remaining.num_seconds().max(0);
                    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);

                    let timer_color = if secs < 300 {
                        DANGER
                    } else if secs < 1800 {
                        WARNING
                    } else {
                        SUCCESS
                    };

                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new("⏱ Trigger Message")
                            .size(13.0)
                            .color(ACCENT),
                    );
                    ui.label(
                        egui::RichText::new(format!("{:02}:{:02}:{:02}", h, m, s))
                            .size(36.0)
                            .strong()
                            .color(timer_color),
                    );
                    ui.label(
                        egui::RichText::new(format!("Target: {}", target.format("%H:%M:%S")))
                            .size(11.0)
                            .color(DIM),
                    );
                }

                // Week stats
                if let Some(wpct) = self.week_percent {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(format!("Weekly: {}%", wpct))
                            .size(11.0)
                            .color(DIM),
                    );
                }
            });
    }

    fn render_error(&self, ui: &mut egui::Ui) {
        if let Some(ref err) = self.last_error {
            egui::Frame::group(ui.style())
                .fill(egui::Color32::from_rgb(40, 20, 20))
                .rounding(6.0)
                .inner_margin(8.0)
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(format!("⚠ {}", err))
                            .color(DANGER)
                            .size(11.0),
                    );
                });
        }
    }

    fn render_controls(&mut self, ui: &mut egui::Ui) {
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            if !self.active {
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new("▶  Start").color(egui::Color32::WHITE),
                        )
                        .fill(BTN_GO)
                        .min_size(egui::vec2(110.0, 34.0)),
                    )
                    .clicked()
                {
                    self.start();
                    self.save_config();
                }
            } else if ui
                .add(
                    egui::Button::new(
                        egui::RichText::new("⏹  Stop").color(egui::Color32::WHITE),
                    )
                    .fill(DANGER)
                    .min_size(egui::vec2(110.0, 34.0)),
                )
                .clicked()
            {
                self.stop();
                self.save_config();
            }

            if ui
                .add(egui::Button::new("🔄 Check Now").min_size(egui::vec2(130.0, 34.0)))
                .clicked()
            {
                self.check_now();
            }
        });
    }

    fn render_settings(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new(egui::RichText::new("⚙ Settings").size(14.0).color(ACCENT).strong())
            .default_open(false)
            .show(ui, |ui| {
                egui::Frame::group(ui.style())
                    .fill(CARD_BG)
                    .rounding(8.0)
                    .inner_margin(14.0)
                    .show(ui, |ui| {
                        egui::Grid::new("settings")
                            .num_columns(2)
                            .spacing([8.0, 6.0])
                            .show(ui, |ui| {
                                ui.label("Model:");
                                egui::ComboBox::from_id_salt("model")
                                    .selected_text(&self.edit_model)
                                    .width(120.0)
                                    .show_ui(ui, |ui| {
                                        for m in ["haiku", "sonnet", "opus"] {
                                            ui.selectable_value(
                                                &mut self.edit_model,
                                                m.to_string(),
                                                m,
                                            );
                                        }
                                    });
                                ui.end_row();

                                ui.label("Message:");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.edit_message)
                                        .desired_width(240.0),
                                );
                                ui.end_row();

                                ui.label("Claude path:");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.edit_claude_path)
                                        .desired_width(240.0)
                                        .font(egui::FontId::monospace(11.0)),
                                );
                                ui.end_row();

                                ui.label("Check interval:");
                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.edit_check_interval)
                                            .desired_width(50.0),
                                    );
                                    ui.label(egui::RichText::new("minutes").size(11.0).color(DIM));
                                });
                                ui.end_row();

                                ui.label("Wait after reset:");
                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.edit_cooldown)
                                            .desired_width(50.0),
                                    );
                                    ui.label(egui::RichText::new("seconds").size(11.0).color(DIM));
                                });
                                ui.end_row();
                            });

                        ui.add_space(6.0);
                        if ui.button("💾 Save Settings").clicked() {
                            self.save_config();
                            self.log("✓ Settings saved");
                        }
                    });
            });
    }

    fn render_log(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new(egui::RichText::new("📋 Log").size(14.0).color(ACCENT).strong())
            .default_open(false)
            .show(ui, |ui| {
                egui::Frame::group(ui.style())
                    .fill(CARD_BG)
                    .rounding(8.0)
                    .inner_margin(14.0)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("Events")
                                    .size(12.0)
                                    .color(DIM),
                            );
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button("Clear").clicked() {
                                    self.log_entries.clear();
                                }
                            });
                        });
                        ui.add_space(4.0);

                        egui::ScrollArea::vertical()
                            .max_height(160.0)
                            .stick_to_bottom(true)
                            .show(ui, |ui| {
                                if self.log_entries.is_empty() {
                                    ui.label(
                                        egui::RichText::new("No logs yet…")
                                            .size(11.0)
                                            .color(DIM)
                                            .italics(),
                                    );
                                }
                                for entry in &self.log_entries {
                                    let color = if entry.contains('✓') || entry.contains('▶') {
                                        SUCCESS
                                    } else if entry.contains('⚠') || entry.contains('↻') || entry.contains('🔄') {
                                        WARNING
                                    } else if entry.contains('✗') {
                                        DANGER
                                    } else {
                                        DIM
                                    };
                                    ui.label(
                                        egui::RichText::new(entry)
                                            .size(11.0)
                                            .color(color)
                                            .font(egui::FontId::monospace(11.0)),
                                    );
                                }
                            });
                    });
            });
    }
}
