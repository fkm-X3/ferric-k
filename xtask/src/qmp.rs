//! Minimal std-only QMP client: connects to QEMU's QMP TCP server,
//! negotiates capabilities, and drives `human-monitor-command` so smoke
//! boots can inject keyboard scancodes via HMP `sendkey`.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

const CONNECT_ATTEMPTS: u32 = 400;
const CONNECT_POLL_MS: u64 = 25;

fn read_line(reader: &mut BufReader<TcpStream>) -> Result<String, String> {
    let mut line = String::new();
    let n = reader
        .read_line(&mut line)
        .map_err(|e| format!("QMP read failed: {e}"))?;
    if n == 0 {
        return Err("QMP stream closed".into());
    }
    Ok(line)
}

fn send(stream: &mut TcpStream, command: &str) -> Result<(), String> {
    writeln!(stream, "{command}").map_err(|e| format!("QMP write failed: {e}"))?;
    stream.flush().map_err(|e| format!("QMP flush failed: {e}"))
}

fn expect_ok(reader: &mut BufReader<TcpStream>, context: &str) -> Result<(), String> {
    let line = read_line(reader)?;
    if line.contains("error") {
        return Err(format!("QMP {context} returned an error: {}", line.trim()));
    }
    Ok(())
}

/// A connected, capability-negotiated QMP session.
pub struct QmpClient {
    reader: BufReader<TcpStream>,
}

impl QmpClient {
    /// Connects to `127.0.0.1:port`, retrying until QEMU opens the socket,
    /// then completes the `qmp_capabilities` handshake.
    pub fn connect(port: u16) -> Result<Self, String> {
        let mut stream = None;
        let mut last_err = None;
        for _ in 0..CONNECT_ATTEMPTS {
            match TcpStream::connect(("127.0.0.1", port)) {
                Ok(s) => {
                    stream = Some(s);
                    break;
                }
                Err(e) => last_err = Some(e),
            }
            std::thread::sleep(Duration::from_millis(CONNECT_POLL_MS));
        }
        let Some(stream) = stream else {
            return Err(format!(
                "QMP never appeared on 127.0.0.1:{port}{}",
                last_err.map(|e| format!(" ({e})")).unwrap_or_default()
            ));
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(|e| format!("QMP read-timeout setup failed: {e}"))?;

        let mut reader = BufReader::new(stream);
        loop {
            let line = read_line(&mut reader)?;
            if line.contains("QMP") {
                break;
            }
        }
        send(reader.get_mut(), r#"{"execute":"qmp_capabilities"}"#)?;
        expect_ok(&mut reader, "capabilities")?;
        Ok(QmpClient { reader })
    }

    /// Routes `command-line` to the human monitor (HMP); errors when QEMU
    /// rejects it.
    pub fn human_monitor_command(&mut self, command_line: &str) -> Result<(), String> {
        let command = format!(
            r#"{{"execute":"human-monitor-command","arguments":{{"command-line":"{command_line}"}}}}"#
        );
        send(self.reader.get_mut(), &command)?;
        expect_ok(&mut self.reader, "human-monitor-command")
    }
}