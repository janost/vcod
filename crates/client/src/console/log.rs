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
    crate::frame_digest::str(crate::frame_digest::Part::Con, text);
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

/// env_logger as before (`RUST_LOG`, default `info`), its message's
/// control characters escaped: server text reaches the log.
fn builder() -> env_logger::Builder {
    let mut b = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));
    let fmt = env_logger::fmt::ConfigurableFormat::default();
    b.format(move |buf, r| {
        let text = r.args().to_string();
        let text = vcod_common::text::escape_controls(&text);
        fmt.format(
            buf,
            &log::Record::builder()
                .level(r.level())
                .target(r.target())
                .module_path(r.module_path())
                .file(r.file())
                .line(r.line())
                .args(format_args!("{text}"))
                .build(),
        )
    });
    b
}

/// [`builder`]'s logger, teed into the console.
pub fn init() {
    let logger = builder().build();
    let max = logger.filter();
    if log::set_boxed_logger(Box::new(Tee(logger))).is_ok() {
        log::set_max_level(max);
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);

    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_server_string_with_esc_reaches_the_terminal_escaped() {
        let sink = Sink::default();
        let mut b = super::builder();
        b.filter_level(log::LevelFilter::Info)
            .write_style(env_logger::WriteStyle::Never)
            .target(env_logger::Target::Pipe(Box::new(sink.clone())));
        log::Log::log(
            &b.build(),
            &log::Record::builder()
                .level(log::Level::Error)
                .args(format_args!("redirect to \x1b[2Jhttp://x/\rforged"))
                .build(),
        );
        let term = String::from_utf8(sink.0.lock().unwrap().clone()).unwrap();
        assert!(!term.contains('\x1b') && !term.contains('\r'), "{term:?}");
        assert!(term.contains("\\x1b[2Jhttp://x/\\x0dforged"), "{term:?}");
    }
}
