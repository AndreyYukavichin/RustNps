use std::fmt;
use std::sync::atomic::{AtomicU8, Ordering};

static LOG_LEVEL: AtomicU8 = AtomicU8::new(Level::Info as u8);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error = 3,
    Warn = 4,
    Notice = 5,
    Info = 6,
    Debug = 7,
    Trace = 8,
}

impl Level {
    fn label(self) -> &'static str {
        match self {
            Self::Error => "E",
            Self::Warn => "W",
            Self::Notice => "N",
            Self::Info => "I",
            Self::Debug => "D",
            Self::Trace => "T",
        }
    }
}

pub fn init_from_text(level: &str) {
    LOG_LEVEL.store(parse_level(level), Ordering::Relaxed);
}

pub fn init_console_from_text(level: &str) {
    LOG_LEVEL.store(parse_console_level(level), Ordering::Relaxed);
}

pub fn parse_level(level: &str) -> u8 {
    let value = level.trim().to_ascii_lowercase();
    match value.as_str() {
        "error" | "err" => Level::Error as u8,
        "warn" | "warning" => Level::Warn as u8,
        "notice" => Level::Notice as u8,
        "info" => Level::Info as u8,
        "debug" => Level::Debug as u8,
        "trace" => Level::Trace as u8,
        _ => value.parse::<u8>().unwrap_or(Level::Info as u8),
    }
}

pub fn parse_console_level(level: &str) -> u8 {
    let value = level.trim().to_ascii_lowercase();
    match value.as_str() {
        "debug" => Level::Debug as u8,
        "info" | "" => Level::Info as u8,
        _ => {
            let parsed = parse_level(&value);
            if parsed >= Level::Debug as u8 {
                Level::Debug as u8
            } else {
                Level::Info as u8
            }
        }
    }
}

pub fn enabled(level: Level) -> bool {
    (level as u8) <= LOG_LEVEL.load(Ordering::Relaxed)
}

pub fn log(level: Level, target: &str, args: fmt::Arguments<'_>) {
    if !enabled(level) {
        return;
    }
    let now = chrono::Local::now().format("%Y/%m/%d %H:%M:%S");
    eprintln!("{now} [{}] [{target}] {args}", level.label());
}

#[macro_export]
macro_rules! log_error {
    ($target:expr, $($arg:tt)*) => {
        $crate::logging::log($crate::logging::Level::Error, $target, format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_warn {
    ($target:expr, $($arg:tt)*) => {
        $crate::logging::log($crate::logging::Level::Warn, $target, format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_notice {
    ($target:expr, $($arg:tt)*) => {
        $crate::logging::log($crate::logging::Level::Notice, $target, format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_info {
    ($target:expr, $($arg:tt)*) => {
        $crate::logging::log($crate::logging::Level::Info, $target, format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_debug {
    ($target:expr, $($arg:tt)*) => {
        $crate::logging::log($crate::logging::Level::Debug, $target, format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_trace {
    ($target:expr, $($arg:tt)*) => {
        $crate::logging::log($crate::logging::Level::Trace, $target, format_args!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use super::{parse_console_level, Level};

    #[test]
    fn console_log_level_accepts_info_and_debug() {
        assert_eq!(parse_console_level("info"), Level::Info as u8);
        assert_eq!(parse_console_level("debug"), Level::Debug as u8);
    }

    #[test]
    fn console_log_level_preserves_legacy_numeric_values() {
        assert_eq!(parse_console_level("6"), Level::Info as u8);
        assert_eq!(parse_console_level("7"), Level::Debug as u8);
        assert_eq!(parse_console_level("8"), Level::Debug as u8);
    }

    #[test]
    fn console_log_level_defaults_to_info_for_unknown_values() {
        assert_eq!(parse_console_level("bogus"), Level::Info as u8);
    }
}
