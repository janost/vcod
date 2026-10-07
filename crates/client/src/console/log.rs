//! The console's line sink: every `log` record that passes the env filter and
//! every [`print`] lands here as well as on the terminal, and the console
//! drains it each frame.

use std::sync::Mutex;

/// Lines nobody drained (a headless `--net-probe` never does) stop here.
const PENDING_CAP: usize = 4096;

static PENDING: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn push(line: String) {
    let mut p = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    if p.len() < PENDING_CAP {
        p.push(line);
    }
}

/// `println!` that the console sees too, colour codes and all; the terminal
/// gets the text without them.
pub fn print(text: &str) {
    println!("{}", vcod_common::net::strip_colors(text));
    for line in text.lines() {
        push(line.to_string());
    }
}

pub fn drain() -> Vec<String> {
    std::mem::take(&mut *PENDING.lock().unwrap_or_else(|e| e.into_inner()))
}

struct Tee(env_logger::Logger);

impl log::Log for Tee {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        self.0.enabled(metadata)
    }

    fn log(&self, record: &log::Record) {
        if !self.0.matches(record) {
            return;
        }
        self.0.log(record);
        // Retail prints its warnings and errors in yellow and red.
        let color = match record.level() {
            log::Level::Error => "^1",
            log::Level::Warn => "^3",
            _ => "",
        };
        for line in record.args().to_string().lines() {
            push(format!("{color}{line}"));
        }
    }

    fn flush(&self) {
        self.0.flush();
    }
}

/// env_logger as before (`RUST_LOG`, default `info`), teed into the console.
pub fn init() {
    let logger =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).build();
    let max = logger.filter();
    if log::set_boxed_logger(Box::new(Tee(logger))).is_ok() {
        log::set_max_level(max);
    }
}
