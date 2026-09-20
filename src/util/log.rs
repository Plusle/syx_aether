#![allow(dead_code)]

use std::{cell::RefCell, fmt::Write, sync::OnceLock};

use chrono::{Local, Timelike};
use colored::*;
use tracing::{Event, Subscriber};
use tracing_subscriber::{
    fmt::{self, format::Writer, FmtContext, FormatEvent, FormatFields},
    layer::SubscriberExt,
    registry::LookupSpan,
    util::SubscriberInitExt,
    EnvFilter,
};

thread_local! {
    static THREAD_NAME: RefCell<Option<String>> = const { RefCell::new(None) };
}

pub fn set_thread_name(name: &str) {
    THREAD_NAME.with(|n| {
        *n.borrow_mut() = Some(name.to_string());
    });
}

pub fn clear_thread_name() {
    THREAD_NAME.with(|n| {
        *n.borrow_mut() = None;
    });
}

fn get_thread_name() -> Option<String> {
    THREAD_NAME.with(|n| n.borrow().clone())
}

fn get_thread_tag() -> String {
    if let Some(name) = get_thread_name() {
        format!("[{}]", name)
    } else {
        let id = std::thread::current().id();
        format!("[{:?}]", id)
    }
}

pub use tracing::{debug, error, info, trace, warn};

static LOG_GUARD: OnceLock<tracing_appender::non_blocking::WorkerGuard> = OnceLock::new();

struct ColorMessageVisitor<'a> {
    writer: &'a mut dyn Write,
    prefix: &'a String,
    level: &'a tracing::Level,
    as_file: bool,
}

impl<'a> tracing::field::Visit for ColorMessageVisitor<'a> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let formatted = format!("{} {:?}", self.prefix, value);
            let msg = if !self.as_file {
                match self.level {
                    &tracing::Level::TRACE => formatted.bright_white(),
                    &tracing::Level::DEBUG => formatted.blue(),
                    &tracing::Level::INFO => formatted.green(),
                    &tracing::Level::WARN => formatted.yellow(),
                    &tracing::Level::ERROR => formatted.red(),
                }
                .to_string()
            } else {
                formatted
            };
            let _ = write!(self.writer, "{}", msg);
        } else {
            let _ = write!(self.writer, " {}={:?}", field.name(), value);
        }
    }
}

struct FullFormatter {
    as_file: bool,
}

impl<S, N> FormatEvent<S, N> for FullFormatter
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> std::fmt::Result {
        let date = Local::now();
        let level = match event.metadata().level() {
            &tracing::Level::TRACE => "[TRACE]",
            &tracing::Level::DEBUG => "[DEBUG]",
            &tracing::Level::INFO => "[INFO]",
            &tracing::Level::WARN => "[WARN]",
            &tracing::Level::ERROR => "[ERROR]",
        };
        let thread_tag = get_thread_tag();
        let prefix = format!(
            "[{:02}:{:02}:{:02}]{}{}",
            date.hour(),
            date.minute(),
            date.second(),
            thread_tag,
            level
        );
        let mut visitor = ColorMessageVisitor {
            writer: &mut writer,
            level: &event.metadata().level(),
            as_file: self.as_file,
            prefix: &prefix,
        };
        event.record(&mut visitor);
        writeln!(writer, "")
    }
}

pub fn initialize() {
    let exe_dir = std::env::current_exe()
        .expect("Failed to get current exe path")
        .parent()
        .expect("Failed to get parent directory")
        .to_path_buf();
    let log_dir = exe_dir.join("log");
    std::fs::create_dir_all(&log_dir).expect("Failed to create log directory");

    let now = Local::now();
    let log_filename = format!("aether_{}.log", now.format("%Y-%m-%dT%H-%M-%S"));
    let log_file_path = log_dir.join(log_filename);
    let file_appender = std::fs::File::create(log_file_path).expect("Failed to create log file");

    let (non_blocking_file, guard) = tracing_appender::non_blocking(file_appender);
    LOG_GUARD.set(guard).expect("Logger has been initialized");

    let file_layer = fmt::layer()
        .with_writer(non_blocking_file)
        .with_ansi(false)
        .event_format(FullFormatter { as_file: true });

    let cmd_layer = fmt::layer()
        .with_writer(std::io::stdout)
        .with_ansi(true)
        .event_format(FullFormatter { as_file: false });

    // Filter out winit's internal trace logs on macOS
    #[cfg(target_os = "macos")]
    let filter = EnvFilter::new("trace,winit::platform_impl=off");
    #[cfg(not(target_os = "macos"))]
    let filter = EnvFilter::new("trace");

    tracing_subscriber::registry()
        .with(filter)
        .with(cmd_layer)
        .with(file_layer)
        .init();
    set_thread_name("main");
    info!("Logger initialized");
}

pub fn terminate() {
    info!("Logger terminated");
}

pub struct Logger;
impl Logger {
    pub fn new() -> Self {
        initialize();
        Self {}
    }
}
impl Drop for Logger {
    fn drop(&mut self) {
        terminate();
    }
}
