use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{Mutex, OnceLock},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

static LOGGER: OnceLock<Mutex<File>> = OnceLock::new();
static STARTED: OnceLock<Instant> = OnceLock::new();

pub fn path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("cassette")
        .join("cassette.log")
}

pub fn init() {
    let _ = STARTED.set(Instant::now());
    let log_path = path();
    if let Some(parent) = log_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(file) = OpenOptions::new().create(true).append(true).open(&log_path)
        && LOGGER.set(Mutex::new(file)).is_ok()
    {
        let unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        line("INFO", "session started");
        line(
            "INFO",
            &format!("unix_time={unix} log={}", log_path.display()),
        );
    }
}

pub fn line(level: &str, message: &str) {
    let Some(logger) = LOGGER.get() else {
        return;
    };
    let elapsed = STARTED
        .get()
        .map(|started| started.elapsed().as_secs_f64())
        .unwrap_or(0.0);
    let thread = std::thread::current()
        .name()
        .unwrap_or("unnamed")
        .to_string();
    if let Ok(mut file) = logger.lock() {
        let _ = writeln!(file, "[{elapsed:>9.3}s] [{level:<5}] [{thread}] {message}");
        let _ = file.flush();
    }
}

#[macro_export]
macro_rules! cassette_log {
    ($level:expr, $($argument:tt)*) => {
        $crate::log::line($level, &format!($($argument)*))
    };
}
