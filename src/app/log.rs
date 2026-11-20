//! Dateilog `<dir>/airplay.log`; bei > 5 MB Rotation nach `airplay.1.log` … `airplay.4.log`.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::fmt::{Arguments, Write as _};
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use windows::Win32::System::SystemInformation::GetLocalTime;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

const MAX_BYTES: u64 = 5 * 1024 * 1024;
/// airplay.log + airplay.1.log … airplay.4.log
const MAX_FILES: u32 = 5;

struct State {
    file: Option<File>,
    dir: PathBuf,
    size: u64,
}

static STATE: Mutex<State> = Mutex::new(State {
    file: None,
    dir: PathBuf::new(),
    size: 0,
});

fn lock() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

impl State {
    fn file_for(&self, index: u32) -> PathBuf {
        if index == 0 {
            self.dir.join("airplay.log")
        } else {
            self.dir.join(format!("airplay.{index}.log"))
        }
    }

    fn open_current(&mut self) {
        let path = self.file_for(0);
        self.file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&path)
            .ok();
        self.size = if self.file.is_some() {
            std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
        } else {
            0
        };
    }

    fn rotate(&mut self) {
        self.file = None;
        let _ = std::fs::remove_file(self.file_for(MAX_FILES - 1));
        for i in (0..=MAX_FILES - 2).rev() {
            let from = self.file_for(i);
            if from.exists() {
                let _ = std::fs::rename(&from, self.file_for(i + 1));
            }
        }
        self.open_current();
    }
}

fn level_name(l: LogLevel) -> &'static str {
    match l {
        LogLevel::Debug => "DEBUG",
        LogLevel::Info => "INFO",
        LogLevel::Warn => "WARN",
        LogLevel::Error => "ERROR",
    }
}

fn write_line(level: LogLevel, text: &str) {
    // SAFETY: GetLocalTime hat keine Vorbedingungen.
    let t = unsafe { GetLocalTime() };
    let mut line = String::with_capacity(text.len() + 40);
    let _ = write!(
        line,
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03} [{}] {}",
        t.wYear,
        t.wMonth,
        t.wDay,
        t.wHour,
        t.wMinute,
        t.wSecond,
        t.wMilliseconds,
        level_name(level),
        text
    );
    line.push('\n');
    let mut st = lock();
    if st.file.is_none() {
        return;
    }
    let len = line.len() as u64;
    if st.size + len > MAX_BYTES && st.size > 0 {
        st.rotate();
    }
    let Some(f) = st.file.as_mut() else { return };
    let _ = f.write_all(line.as_bytes());
    let _ = f.flush();
    st.size += len;
}

pub fn log_init(dir: &Path) {
    let mut st = lock();
    st.file = None;
    st.dir = dir.to_path_buf();
    let _ = std::fs::create_dir_all(dir);
    st.open_current();
}

pub fn log_shutdown() {
    lock().file = None;
}

#[doc(hidden)]
pub fn write_fmt(level: LogLevel, args: Arguments<'_>) {
    write_line(level, &args.to_string());
}

/// `log_msg!(LogLevel::Info, "hallo {}", 42)`
#[macro_export]
macro_rules! log_msg {
    ($level:expr, $($arg:tt)+) => {
        $crate::app::log::write_fmt($level, format_args!($($arg)+))
    };
}

/// Passt zu `raop_log_callback_t`: Level sind syslog-artig (LOGGER_ERR=3 … LOGGER_DEBUG=7).
/// Panikt nie.
///
/// # Safety
/// `msg` ist null oder zeigt auf eine nullterminierte C-Zeichenkette.
pub unsafe extern "C" fn log_raop_bridge(_cls: *mut c_void, level: c_int, msg: *const c_char) {
    let _ = std::panic::catch_unwind(|| {
        let l = match level {
            i32::MIN..=3 => LogLevel::Error,
            4 => LogLevel::Warn,
            5 | 6 => LogLevel::Info,
            _ => LogLevel::Debug,
        };
        let body = if msg.is_null() {
            String::new()
        } else {
            // SAFETY: UxPlay übergibt eine nullterminierte C-Zeichenkette.
            unsafe { CStr::from_ptr(msg) }
                .to_string_lossy()
                .into_owned()
        };
        let text = format!("[raop] {body}");
        write_line(l, text.trim_end_matches(['\n', '\r']));
    });
}
