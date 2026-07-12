//! Main application — egui dark-theme UI + state management.
//!
//! Renders a compact dashboard with session status, countdown timer,
//! settings panel, and scrollable log. Communicates with the background
//! scheduler via channels.

use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use chrono::{DateTime, Local};
use eframe::egui;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem},
    MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
};

// ── Palette ──────────────────────────────────────────────────────────────────
//
// Ink ground, one clay accent (Anthropic's brick tone), semantic
// mint/amber/red reserved strictly for state.

const INK: egui::Color32 = egui::Color32::from_rgb(14, 16, 20); // app ground
const PANEL: egui::Color32 = egui::Color32::from_rgb(22, 25, 31); // raised card
const EDGE: egui::Color32 = egui::Color32::from_rgb(38, 42, 51); // card hairline
const WELL: egui::Color32 = egui::Color32::from_rgb(9, 11, 14); // inputs, log well
const CLAY: egui::Color32 = egui::Color32::from_rgb(217, 119, 87); // accent
const MINT: egui::Color32 = egui::Color32::from_rgb(92, 190, 140); // ok / running
const AMBER: egui::Color32 = egui::Color32::from_rgb(214, 164, 62); // busy / warning
const RED: egui::Color32 = egui::Color32::from_rgb(226, 91, 71); // error / danger
const FOG: egui::Color32 = egui::Color32::from_rgb(134, 142, 153); // secondary text
const TEXT: egui::Color32 = egui::Color32::from_rgb(224, 227, 232); // primary text

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

    // Native window / tray state
    hwnd: isize,
    #[cfg(not(windows))]
    _tray_icon: Option<TrayIcon>,
    should_close: Arc<AtomicBool>,
}

use crate::config::Config;
use crate::scheduler::{Command, Event, Scheduler};

// ── Native window helpers ────────────────────────────────────────────────────
//
// On Windows the winit loop sleeps while the window is hidden and queued
// `ViewportCommand`s are never processed, so show/hide goes straight through
// the Win32 API. On macOS the tray shares the main event loop, so viewport
// commands work as intended.

fn show_native_window(hwnd: isize, ctx: &egui::Context) {
    #[cfg(windows)]
    {
        if hwnd != 0 {
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
            };
            unsafe {
                ShowWindow(hwnd as _, SW_RESTORE);
                ShowWindow(hwnd as _, SW_SHOW);
                SetForegroundWindow(hwnd as _);
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = hwnd;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }
    ctx.request_repaint();
}

fn hide_native_window(hwnd: isize, ctx: &egui::Context) {
    #[cfg(windows)]
    {
        let _ = ctx;
        if hwnd != 0 {
            use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
            unsafe {
                ShowWindow(hwnd as _, SW_HIDE);
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = hwnd;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
    }
}

fn request_native_close(hwnd: isize, ctx: &egui::Context) {
    #[cfg(windows)]
    {
        if hwnd != 0 {
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                PostMessageW, ShowWindow, SW_SHOW, WM_CLOSE,
            };
            unsafe {
                ShowWindow(hwnd as _, SW_SHOW);
                PostMessageW(hwnd as _, WM_CLOSE, 0, 0);
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = hwnd;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
    ctx.request_repaint();
}

// ── Tray setup ───────────────────────────────────────────────────────────────

/// Build the tray icon + menu and register global event handlers.
/// Must be called on the thread that will pump this tray's events
/// (a dedicated Win32 pump thread on Windows, the main thread on macOS).
fn build_tray(hwnd: isize, ctx: &egui::Context, close_flag: Arc<AtomicBool>) -> Option<TrayIcon> {
    let menu = Menu::new();
    let show_item = MenuItem::new("Open", true, None);
    let exit_item = MenuItem::new("Quit", true, None);
    let _ = menu.append(&show_item);
    let _ = menu.append(&exit_item);
    let show_id = show_item.id().clone();
    let exit_id = exit_item.id().clone();

    let icon = load_tray_icon().unwrap_or_else(create_tray_icon);
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(icon)
        .with_tooltip("Claude Timer Reset")
        .build()
        .ok();

    let ctx_menu = ctx.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        if event.id == show_id {
            show_native_window(hwnd, &ctx_menu);
        } else if event.id == exit_id {
            close_flag.store(true, Ordering::SeqCst);
            request_native_close(hwnd, &ctx_menu);
        }
    }));

    // Left-click (or double-click) on the tray icon restores the window
    let ctx_tray = ctx.clone();
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        let restore = matches!(
            event,
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } | TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            }
        );
        if restore {
            show_native_window(hwnd, &ctx_tray);
        }
    }));

    tray
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // ── Theme ──
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = INK;
        visuals.window_fill = INK;
        visuals.extreme_bg_color = WELL;
        visuals.faint_bg_color = PANEL;
        visuals.override_text_color = Some(TEXT);
        visuals.widgets.noninteractive.bg_fill = PANEL;
        visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, EDGE);
        visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(31, 35, 43);
        visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(40, 45, 55);
        visuals.widgets.active.bg_fill = egui::Color32::from_rgb(48, 54, 66);
        visuals.selection.bg_fill = CLAY.gamma_multiply(0.35);
        visuals.selection.stroke = egui::Stroke::new(1.0_f32, CLAY);
        for w in [
            &mut visuals.widgets.noninteractive,
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
            &mut visuals.widgets.open,
        ] {
            w.rounding = egui::Rounding::same(6.0);
        }
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

        // Native window handle — used to show/hide the window from the tray
        // thread, since egui viewport commands are not processed while hidden.
        let hwnd: isize = match cc.window_handle().map(|h| h.as_raw()) {
            Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
            _ => 0,
        };

        let should_close = Arc::new(AtomicBool::new(false));

        // On Windows the tray icon lives on its own thread with a Win32
        // message pump — tray/menu events are only delivered on the thread
        // that created the icon, and the winit loop sleeps while hidden.
        #[cfg(windows)]
        {
            let ctx = cc.egui_ctx.clone();
            let close_flag = should_close.clone();
            std::thread::spawn(move || {
                let _tray_icon = build_tray(hwnd, &ctx, close_flag);

                use windows_sys::Win32::UI::WindowsAndMessaging::{
                    DispatchMessageW, GetMessageW, TranslateMessage, MSG,
                };
                unsafe {
                    let mut msg: MSG = std::mem::zeroed();
                    while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                        TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
            });
        }

        // On macOS the tray must be created on the main thread; the winit
        // loop pumps its events.
        #[cfg(not(windows))]
        let tray_icon = build_tray(hwnd, &cc.egui_ctx, should_close.clone());

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
            hwnd,
            #[cfg(not(windows))]
            _tray_icon: tray_icon,
            should_close,
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
        self.timer_target = None;
        self.checking = false;
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
                    self.status = if self.active { "Running".into() } else { "Stopped".into() };
                    self.last_error = None;
                }
                Event::TimerSet(target) => {
                    self.timer_target = Some(target);
                }
                Event::MessageSent(response) => {
                    self.log(&format!("✓ Response: {}", response));
                    self.timer_target = None;
                    self.status = if self.active { "Running".into() } else { "Stopped".into() };
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
                    self.status = "Checking".into();
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

    fn status_color(&self) -> egui::Color32 {
        if self.checking {
            AMBER
        } else if self.last_error.is_some() {
            RED
        } else if self.active {
            MINT
        } else {
            FOG
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

fn card() -> egui::Frame {
    egui::Frame::none()
        .fill(PANEL)
        .stroke(egui::Stroke::new(1.0_f32, EDGE))
        .rounding(10.0)
        .inner_margin(14.0)
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.process_events();

        // ── Handle Close Button Request ──
        // X button hides to tray; only the tray "Quit" item really closes.
        if ctx.input(|i| i.viewport().close_requested())
            && !self.should_close.load(Ordering::SeqCst)
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            hide_native_window(self.hwnd, ctx);
        }

        // Repaint every second for timer countdown
        ctx.request_repaint_after(std::time::Duration::from_secs(1));

        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(INK).inner_margin(16.0))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 10.0;

                    self.render_header(ui);
                    self.render_timer_card(ui);
                    self.render_usage_card(ui);
                    self.render_error(ui);
                    self.render_controls(ui);
                    ui.add_space(2.0);
                    self.render_settings(ui);
                    self.render_log(ui);
                });
            });
    }
}

impl App {
    fn render_header(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.label(
                    egui::RichText::new("Claude Timer Reset")
                        .size(16.0)
                        .strong()
                        .color(TEXT),
                );
                ui.label(
                    egui::RichText::new(format!(
                        "v{} · session scheduler",
                        env!("CARGO_PKG_VERSION")
                    ))
                    .size(10.5)
                    .color(FOG),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let color = self.status_color();
                egui::Frame::none()
                    .fill(color.gamma_multiply(0.16))
                    .rounding(999.0)
                    .inner_margin(egui::Margin::symmetric(10.0, 4.0))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        ui.label(egui::RichText::new(&self.status).size(11.0).color(color));
                        let (rect, _) = ui
                            .allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                        ui.painter().circle_filled(rect.center(), 3.5, color);
                    });
            });
        });
    }

    /// Signature element: the next-session countdown, oversized and monospace.
    fn render_timer_card(&self, ui: &mut egui::Ui) {
        card().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;

                if let Some(target) = self.timer_target {
                    let remaining = target - Local::now();
                    let secs = remaining.num_seconds().max(0);
                    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);

                    ui.label(
                        egui::RichText::new("NEXT SESSION IN")
                            .size(10.0)
                            .color(CLAY)
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new(format!("{:02}:{:02}:{:02}", h, m, s))
                            .font(egui::FontId::monospace(44.0))
                            .strong()
                            .color(if secs < 300 { RED } else { TEXT }),
                    );
                    let starts = if target.date_naive() == Local::now().date_naive() {
                        target.format("%H:%M:%S").to_string()
                    } else {
                        target.format("%b %d, %H:%M:%S").to_string()
                    };
                    ui.label(
                        egui::RichText::new(format!(
                            "starts {} · sends \"{}\" on {}",
                            starts,
                            truncate(&self.config.test_message, 24),
                            self.config.default_model
                        ))
                        .size(10.5)
                        .color(FOG),
                    );
                } else {
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new("No session scheduled")
                            .size(14.0)
                            .color(FOG),
                    );
                    let hint = if self.checking {
                        "Checking usage…"
                    } else if self.active {
                        "Waiting for next usage check"
                    } else {
                        "Press Start to track your session"
                    };
                    ui.label(egui::RichText::new(hint).size(10.5).color(FOG));
                    ui.add_space(6.0);
                }
            });
        });
    }

    fn render_usage_card(&self, ui: &mut egui::Ui) {
        card().show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;

            match self.session_percent {
                Some(pct) => {
                    let bar_color = if pct > 80 {
                        RED
                    } else if pct > 50 {
                        AMBER
                    } else {
                        MINT
                    };

                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Session usage").size(12.0).color(TEXT));
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                ui.label(
                                    egui::RichText::new(format!("{}%", pct))
                                        .font(egui::FontId::monospace(12.0))
                                        .strong()
                                        .color(bar_color),
                                );
                            },
                        );
                    });
                    ui.add(
                        egui::ProgressBar::new(pct as f32 / 100.0)
                            .fill(bar_color)
                            .desired_height(6.0),
                    );

                    if let Some(reset) = self.reset_time {
                        let remaining = reset - Local::now();
                        let secs = remaining.num_seconds().max(0);
                        let (h, m) = (secs / 3600, (secs % 3600) / 60);
                        ui.label(
                            egui::RichText::new(format!(
                                "Limit resets at {} — {}h {:02}m left",
                                reset.format("%H:%M"),
                                h,
                                m
                            ))
                            .size(10.5)
                            .color(FOG),
                        );
                    }

                    if let Some(wpct) = self.week_percent {
                        ui.label(
                            egui::RichText::new(format!("Weekly usage {}%", wpct))
                                .size(10.5)
                                .color(FOG),
                        );
                    }
                }
                None => {
                    ui.label(
                        egui::RichText::new("No usage data yet — run a check to fill this in.")
                            .size(11.0)
                            .color(FOG),
                    );
                }
            }
        });
    }

    fn render_error(&self, ui: &mut egui::Ui) {
        if let Some(ref err) = self.last_error {
            egui::Frame::none()
                .fill(RED.gamma_multiply(0.12))
                .stroke(egui::Stroke::new(1.0_f32, RED.gamma_multiply(0.5)))
                .rounding(8.0)
                .inner_margin(10.0)
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(err).color(RED).size(11.0));
                });
        }
    }

    fn render_controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;

            if !self.active {
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new("Start")
                                .size(13.0)
                                .strong()
                                .color(egui::Color32::from_rgb(28, 16, 12)),
                        )
                        .fill(CLAY)
                        .rounding(8.0)
                        .min_size(egui::vec2(120.0, 34.0)),
                    )
                    .clicked()
                {
                    self.start();
                    self.save_config();
                }
            } else if ui
                .add(
                    egui::Button::new(
                        egui::RichText::new("Stop")
                            .size(13.0)
                            .strong()
                            .color(egui::Color32::WHITE),
                    )
                    .fill(RED)
                    .rounding(8.0)
                    .min_size(egui::vec2(120.0, 34.0)),
                )
                .clicked()
            {
                self.stop();
                self.save_config();
            }

            if ui
                .add(
                    egui::Button::new(egui::RichText::new("Check now").size(13.0))
                        .rounding(8.0)
                        .min_size(egui::vec2(110.0, 34.0)),
                )
                .clicked()
            {
                self.check_now();
            }
        });
    }

    fn render_settings(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new(
            egui::RichText::new("Settings").size(12.5).color(FOG).strong(),
        )
        .default_open(false)
        .show(ui, |ui| {
            card().show(ui, |ui| {
                egui::Grid::new("settings")
                    .num_columns(2)
                    .spacing([10.0, 8.0])
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new("Model").size(11.5).color(FOG));
                        egui::ComboBox::from_id_salt("model")
                            .selected_text(&self.edit_model)
                            .width(120.0)
                            .show_ui(ui, |ui| {
                                for m in ["haiku", "sonnet", "opus"] {
                                    ui.selectable_value(&mut self.edit_model, m.to_string(), m);
                                }
                            });
                        ui.end_row();

                        ui.label(egui::RichText::new("Message").size(11.5).color(FOG));
                        ui.add(
                            egui::TextEdit::singleline(&mut self.edit_message)
                                .desired_width(230.0),
                        );
                        ui.end_row();

                        ui.label(egui::RichText::new("Claude path").size(11.5).color(FOG));
                        ui.add(
                            egui::TextEdit::singleline(&mut self.edit_claude_path)
                                .desired_width(230.0)
                                .font(egui::FontId::monospace(11.0)),
                        );
                        ui.end_row();

                        ui.label(egui::RichText::new("Check interval").size(11.5).color(FOG));
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.edit_check_interval)
                                    .desired_width(50.0),
                            );
                            ui.label(egui::RichText::new("minutes").size(10.5).color(FOG));
                        });
                        ui.end_row();

                        ui.label(egui::RichText::new("Wait after reset").size(11.5).color(FOG));
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.edit_cooldown)
                                    .desired_width(50.0),
                            );
                            ui.label(egui::RichText::new("seconds").size(10.5).color(FOG));
                        });
                        ui.end_row();
                    });

                ui.add_space(8.0);
                if ui
                    .add(egui::Button::new(egui::RichText::new("Save settings").size(12.0)))
                    .clicked()
                {
                    self.save_config();
                    self.log("✓ Settings saved");
                }
            });
        });
    }

    fn render_log(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new(
            egui::RichText::new("Log").size(12.5).color(FOG).strong(),
        )
        .default_open(false)
        .show(ui, |ui| {
            egui::Frame::none()
                .fill(WELL)
                .stroke(egui::Stroke::new(1.0_f32, EDGE))
                .rounding(10.0)
                .inner_margin(12.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!("{} events", self.log_entries.len()))
                                .size(10.5)
                                .color(FOG),
                        );
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if ui.small_button("Clear").clicked() {
                                    self.log_entries.clear();
                                }
                            },
                        );
                    });
                    ui.add_space(4.0);

                    egui::ScrollArea::vertical()
                        .max_height(160.0)
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            if self.log_entries.is_empty() {
                                ui.label(
                                    egui::RichText::new("Nothing yet — events land here.")
                                        .size(11.0)
                                        .color(FOG)
                                        .italics(),
                                );
                            }
                            for entry in &self.log_entries {
                                let color = if entry.contains('✓') || entry.contains('▶') {
                                    MINT
                                } else if entry.contains('⚠') {
                                    AMBER
                                } else if entry.contains('✗') {
                                    RED
                                } else {
                                    FOG
                                };
                                ui.label(
                                    egui::RichText::new(entry)
                                        .color(color)
                                        .font(egui::FontId::monospace(11.0)),
                                );
                            }
                        });
                });
        });
    }
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max_chars).collect();
        format!("{}…", cut)
    }
}

fn create_tray_icon() -> tray_icon::Icon {
    let width = 32;
    let height = 32;
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    for y in 0..height {
        for x in 0..width {
            let idx = ((y * width + x) * 4) as usize;
            let cx = width as f32 / 2.0;
            let cy = height as f32 / 2.0;
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let dist_sq = dx * dx + dy * dy;
            let radius = width as f32 / 2.0 - 2.0;
            if dist_sq <= radius * radius {
                rgba[idx] = 217; // R — clay accent
                rgba[idx + 1] = 119; // G
                rgba[idx + 2] = 87; // B
                rgba[idx + 3] = 255; // A
            } else {
                rgba[idx + 3] = 0;
            }
        }
    }
    tray_icon::Icon::from_rgba(rgba, width, height).unwrap()
}

fn load_tray_icon() -> Option<tray_icon::Icon> {
    // Try cwd first, then the directory next to the executable (cwd differs
    // when launched from a shortcut or Windows startup).
    let mut path = PathBuf::from("assets/icon.png");
    if !path.exists() {
        path = std::env::current_exe()
            .ok()?
            .parent()?
            .join("assets")
            .join("icon.png");
        if !path.exists() {
            return None;
        }
    }
    let img = image::open(&path).ok()?;
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    tray_icon::Icon::from_rgba(rgba.into_raw(), width, height).ok()
}
