//! The log's record format: env_logger's own, with control characters in
//! the message escaped (`vcod_common::text::escape_controls`). Client
//! names, rcon lines and chat reach the log, and a terminal would run
//! their escape sequences.

/// Installs the escaping format on `b`.
pub fn escape_controls(b: &mut env_logger::Builder) {
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
    fn a_record_with_esc_and_cr_reaches_the_sink_escaped() {
        let sink = Sink::default();
        let mut b = env_logger::Builder::new();
        b.filter_level(log::LevelFilter::Info)
            .write_style(env_logger::WriteStyle::Never)
            .target(env_logger::Target::Pipe(Box::new(sink.clone())));
        super::escape_controls(&mut b);
        let logger = b.build();
        let shown = "x\x1b]0;owned\x07\r 5:00 say;0;3;Admin;!kick 7";
        log::Log::log(
            &logger,
            &log::Record::builder()
                .level(log::Level::Info)
                .args(format_args!("Bad rcon from 1.2.3.4:28960:\n{shown}"))
                .build(),
        );
        let out = String::from_utf8(sink.0.lock().unwrap().clone()).unwrap();
        assert!(
            !out.bytes().any(|b| b == 0x1b || b == b'\r' || b == 0x07),
            "{out:?}"
        );
        assert!(out.contains("x\\x1b]0;owned\\x07\\x0d 5:00 say"), "{out:?}");
        // The record's own newline survives, indented as before.
        assert!(out.contains("28960:\n"), "{out:?}");
    }
}
