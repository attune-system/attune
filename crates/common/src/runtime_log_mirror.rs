use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use std::io::{self, Write};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::OnceLock;

const MAX_FRAGMENT_BYTES: usize = 128 * 1024;
const WRITER_QUEUE_CAPACITY: usize = 1024;

static STDOUT_WRITER: OnceLock<SyncSender<Vec<u8>>> = OnceLock::new();
static STDERR_WRITER: OnceLock<SyncSender<Vec<u8>>> = OnceLock::new();

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeLogStream {
    Stdout,
    Stderr,
}

impl RuntimeLogStream {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "source_kind", rename_all = "snake_case")]
pub enum RuntimeLogSource {
    Execution {
        execution_id: i64,
        parent_execution_id: Option<i64>,
        action_ref: String,
        pack_ref: String,
        trace_tag: Option<String>,
        worker_id: i64,
        worker_name: String,
        worker_instance: uuid::Uuid,
    },
    Sensor {
        sensor_id: i64,
        sensor_ref: String,
        pack_ref: String,
        worker_id: i64,
        worker_name: String,
        worker_instance: uuid::Uuid,
        workload_id: i64,
        assignment_generation: i64,
        process_id: Option<u32>,
    },
}

/// Select a live mirror source independently for stdout and stderr.
pub fn select_mirror_sources(
    source: RuntimeLogSource,
    mirror_stdout: bool,
    mirror_stderr: bool,
) -> (Option<RuntimeLogSource>, Option<RuntimeLogSource>) {
    let stdout_source = mirror_stdout.then(|| source.clone());
    let stderr_source = mirror_stderr.then_some(source);
    (stdout_source, stderr_source)
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum RuntimeLogBodyType {
    Json,
    Text,
    Bytes,
}

#[derive(Debug, Serialize)]
struct RuntimeLogRecord<'a> {
    event: &'static str,
    observed_at: DateTime<Utc>,
    #[serde(flatten)]
    source: &'a RuntimeLogSource,
    source_stream: RuntimeLogStream,
    byte_start: u64,
    byte_end: u64,
    fragment_index: u32,
    continued: bool,
    truncated: bool,
    body_type: RuntimeLogBodyType,
    #[serde(skip_serializing_if = "Option::is_none")]
    body_encoding: Option<&'static str>,
    body: Value,
}

#[derive(Debug)]
struct RuntimeLogFragment {
    bytes: Vec<u8>,
    byte_start: u64,
    byte_end: u64,
    fragment_index: u32,
    continued: bool,
    truncated: bool,
}

type RecordSink = Box<
    dyn FnMut(&RuntimeLogSource, RuntimeLogStream, &RuntimeLogFragment) -> io::Result<()> + Send,
>;

/// Converts a byte stream into bounded, line-oriented runtime log records.
pub struct RuntimeLogMirror {
    source: RuntimeLogSource,
    stream: RuntimeLogStream,
    pending: Vec<u8>,
    pending_start: u64,
    stream_offset: u64,
    fragment_index: u32,
    max_bytes: Option<u64>,
    input_bytes: u64,
    truncated: bool,
    sink: RecordSink,
}

impl RuntimeLogMirror {
    pub fn new(source: RuntimeLogSource, stream: RuntimeLogStream, max_bytes: Option<u64>) -> Self {
        Self::with_sink(source, stream, max_bytes, Box::new(write_record))
    }

    fn with_sink(
        source: RuntimeLogSource,
        stream: RuntimeLogStream,
        max_bytes: Option<u64>,
        sink: RecordSink,
    ) -> Self {
        Self {
            source,
            stream,
            pending: Vec::new(),
            pending_start: 0,
            stream_offset: 0,
            fragment_index: 0,
            max_bytes,
            input_bytes: 0,
            truncated: false,
            sink,
        }
    }

    pub fn push(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self.truncated {
            return Ok(());
        }
        let allowed = self
            .max_bytes
            .map(|limit| {
                usize::try_from(limit.saturating_sub(self.input_bytes)).unwrap_or(usize::MAX)
            })
            .unwrap_or(bytes.len())
            .min(bytes.len());
        self.input_bytes = self.input_bytes.saturating_add(allowed as u64);
        let mut remaining = &bytes[..allowed];
        while !remaining.is_empty() {
            let newline = remaining.iter().position(|byte| *byte == b'\n');
            let take = newline.map_or(remaining.len(), |index| index + 1);
            let mut part = &remaining[..take];
            let has_newline = part.last() == Some(&b'\n');
            if has_newline {
                part = &part[..part.len() - 1];
            }

            self.append_bounded(part)?;

            if has_newline {
                if self.pending.last() == Some(&b'\r') {
                    self.pending.pop();
                }
                self.emit_pending(false)?;
                self.stream_offset = self.stream_offset.saturating_add(1);
                self.pending_start = self.stream_offset;
                self.fragment_index = 0;
            }
            remaining = &remaining[take..];
        }
        if allowed < bytes.len() {
            if !self.pending.is_empty() || self.fragment_index > 0 {
                self.emit_pending(false)?;
            }
            self.emit_truncated()?;
            self.truncated = true;
        }
        Ok(())
    }

    pub fn finish(&mut self) -> io::Result<()> {
        if !self.pending.is_empty() || self.fragment_index > 0 {
            self.emit_pending(false)?;
        }
        Ok(())
    }

    fn append_bounded(&mut self, mut bytes: &[u8]) -> io::Result<()> {
        while !bytes.is_empty() {
            let available = (MAX_FRAGMENT_BYTES + 1).saturating_sub(self.pending.len());
            let take = available.min(bytes.len());
            self.pending.extend_from_slice(&bytes[..take]);
            self.stream_offset = self.stream_offset.saturating_add(take as u64);
            bytes = &bytes[take..];
            if self.pending.len() > MAX_FRAGMENT_BYTES {
                let overflow = self.pending.split_off(MAX_FRAGMENT_BYTES);
                self.emit_pending(true)?;
                self.pending_start = self.stream_offset.saturating_sub(overflow.len() as u64);
                self.pending = overflow;
                self.fragment_index = self.fragment_index.saturating_add(1);
            }
        }
        Ok(())
    }

    fn emit_pending(&mut self, continued: bool) -> io::Result<()> {
        let bytes = std::mem::take(&mut self.pending);
        let fragment = RuntimeLogFragment {
            byte_start: self.pending_start,
            byte_end: self.pending_start.saturating_add(bytes.len() as u64),
            bytes,
            fragment_index: self.fragment_index,
            continued,
            truncated: false,
        };
        (self.sink)(&self.source, self.stream, &fragment)
    }

    fn emit_truncated(&mut self) -> io::Result<()> {
        (self.sink)(
            &self.source,
            self.stream,
            &RuntimeLogFragment {
                bytes: b"Runtime log mirror reached its configured byte limit".to_vec(),
                byte_start: self.stream_offset,
                byte_end: self.stream_offset,
                fragment_index: self.fragment_index,
                continued: false,
                truncated: true,
            },
        )
    }
}

fn record_json(
    source: &RuntimeLogSource,
    stream: RuntimeLogStream,
    fragment: &RuntimeLogFragment,
) -> serde_json::Result<Vec<u8>> {
    let (body_type, body_encoding, body) = match std::str::from_utf8(&fragment.bytes) {
        Ok(text) if fragment.fragment_index == 0 && !fragment.continued => {
            match serde_json::from_str::<Value>(text) {
                Ok(value @ Value::Object(_)) => (RuntimeLogBodyType::Json, None, value),
                _ => (
                    RuntimeLogBodyType::Text,
                    None,
                    Value::String(text.to_string()),
                ),
            }
        }
        Ok(text) => (
            RuntimeLogBodyType::Text,
            None,
            Value::String(text.to_string()),
        ),
        Err(_) => (
            RuntimeLogBodyType::Bytes,
            Some("base64"),
            Value::String(BASE64.encode(&fragment.bytes)),
        ),
    };
    serde_json::to_vec(&RuntimeLogRecord {
        event: "attune.runtime_log",
        observed_at: Utc::now(),
        source,
        source_stream: stream,
        byte_start: fragment.byte_start,
        byte_end: fragment.byte_end,
        fragment_index: fragment.fragment_index,
        continued: fragment.continued,
        truncated: fragment.truncated,
        body_type,
        body_encoding,
        body,
    })
}

fn write_record(
    source: &RuntimeLogSource,
    stream: RuntimeLogStream,
    fragment: &RuntimeLogFragment,
) -> io::Result<()> {
    let mut record = record_json(source, stream, fragment).map_err(io::Error::other)?;
    record.push(b'\n');
    match writer_sender(stream).try_send(record) {
        Ok(()) | Err(TrySendError::Full(_)) => Ok(()),
        Err(TrySendError::Disconnected(_)) => Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "runtime log mirror writer stopped",
        )),
    }
}

fn writer_sender(stream: RuntimeLogStream) -> &'static SyncSender<Vec<u8>> {
    match stream {
        RuntimeLogStream::Stdout => STDOUT_WRITER.get_or_init(|| start_writer(stream)),
        RuntimeLogStream::Stderr => STDERR_WRITER.get_or_init(|| start_writer(stream)),
    }
}

fn start_writer(stream: RuntimeLogStream) -> SyncSender<Vec<u8>> {
    let (sender, receiver) = sync_channel::<Vec<u8>>(WRITER_QUEUE_CAPACITY);
    std::thread::Builder::new()
        .name(format!("runtime-log-mirror-{}", stream.as_str()))
        .spawn(move || {
            while let Ok(record) = receiver.recv() {
                let result = match stream {
                    RuntimeLogStream::Stdout => std::io::stdout().lock().write_all(&record),
                    RuntimeLogStream::Stderr => std::io::stderr().lock().write_all(&record),
                };
                if result.is_err() {
                    break;
                }
            }
        })
        .expect("failed to start runtime log mirror writer");
    sender
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn source() -> RuntimeLogSource {
        RuntimeLogSource::Execution {
            execution_id: 42,
            parent_execution_id: Some(41),
            action_ref: "core.echo".to_string(),
            pack_ref: "core".to_string(),
            trace_tag: Some("test.trace".to_string()),
            worker_id: 7,
            worker_name: "worker-python".to_string(),
            worker_instance: uuid::Uuid::nil(),
        }
    }

    fn encoded(bytes: &[u8]) -> Value {
        encoded_for_source(&source(), bytes)
    }

    #[test]
    fn selects_runtime_log_mirror_sources_per_stream() {
        for (stdout, stderr) in [(false, false), (true, false), (false, true), (true, true)] {
            let (stdout_source, stderr_source) = select_mirror_sources(source(), stdout, stderr);
            assert_eq!(stdout_source.is_some(), stdout);
            assert_eq!(stderr_source.is_some(), stderr);
        }
    }

    fn encoded_for_source(source: &RuntimeLogSource, bytes: &[u8]) -> Value {
        let fragment = RuntimeLogFragment {
            bytes: bytes.to_vec(),
            byte_start: 0,
            byte_end: bytes.len() as u64,
            fragment_index: 0,
            continued: false,
            truncated: false,
        };
        serde_json::from_slice(&record_json(source, RuntimeLogStream::Stdout, &fragment).unwrap())
            .unwrap()
    }

    fn recording_mirror(max_bytes: Option<u64>) -> (RuntimeLogMirror, Arc<Mutex<Vec<Value>>>) {
        let records = Arc::new(Mutex::new(Vec::new()));
        let captured = records.clone();
        let mirror = RuntimeLogMirror::with_sink(
            source(),
            RuntimeLogStream::Stdout,
            max_bytes,
            Box::new(move |source, stream, fragment| {
                let value = serde_json::from_slice(&record_json(source, stream, fragment).unwrap())
                    .unwrap();
                captured.lock().unwrap().push(value);
                Ok(())
            }),
        );
        (mirror, records)
    }

    #[test]
    fn preserves_json_object_body() {
        let record = encoded(br#"{"level":"info","count":3}"#);
        assert_eq!(record["body_type"], "json");
        assert_eq!(record["body"]["level"], "info");
        assert_eq!(record["body"]["count"], 3);
        assert_eq!(record["execution_id"], 42);
        assert_eq!(record["worker_id"], 7);
        assert_eq!(record["worker_name"], "worker-python");
        assert_eq!(record["worker_instance"], uuid::Uuid::nil().to_string());
    }

    #[test]
    fn includes_sensor_worker_identity() {
        let instance = uuid::Uuid::new_v4();
        let source = RuntimeLogSource::Sensor {
            sensor_id: 17,
            sensor_ref: "github.pull_requests".to_string(),
            pack_ref: "github".to_string(),
            worker_id: 8,
            worker_name: "rdrx-sensor-workers".to_string(),
            worker_instance: instance,
            workload_id: 63,
            assignment_generation: 4,
            process_id: Some(2147),
        };
        let record = encoded_for_source(&source, b"polling");

        assert_eq!(record["source_kind"], "sensor");
        assert_eq!(record["worker_id"], 8);
        assert_eq!(record["worker_name"], "rdrx-sensor-workers");
        assert_eq!(record["worker_instance"], instance.to_string());
    }

    #[test]
    fn keeps_plain_text_as_a_string() {
        let record = encoded(b"hello world");
        assert_eq!(record["body_type"], "text");
        assert_eq!(record["body"], "hello world");
    }

    #[test]
    fn does_not_promote_child_fields_into_the_envelope() {
        let record = encoded(br#"{"execution_id":999,"source_stream":"stderr"}"#);
        assert_eq!(record["execution_id"], 42);
        assert_eq!(record["source_stream"], "stdout");
        assert_eq!(record["body"]["execution_id"], 999);
    }

    #[test]
    fn base64_encodes_invalid_utf8() {
        let record = encoded(&[0xff, 0xfe]);
        assert_eq!(record["body_type"], "bytes");
        assert_eq!(record["body_encoding"], "base64");
        assert_eq!(record["body"], "//4=");
    }

    #[test]
    fn frames_json_split_across_reads() {
        let (mut mirror, records) = recording_mirror(None);
        mirror.push(br#"{"count":"#).unwrap();
        mirror.push(b"3}\nnext").unwrap();
        mirror.finish().unwrap();

        let records = records.lock().unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["body_type"], "json");
        assert_eq!(records[0]["body"]["count"], 3);
        assert_eq!(records[1]["body"], "next");
        assert_eq!(records[1]["byte_start"], 12);
    }

    #[test]
    fn bounds_large_lines_and_marks_continuation() {
        let (mut mirror, records) = recording_mirror(None);
        mirror.push(&vec![b'x'; MAX_FRAGMENT_BYTES + 1]).unwrap();
        mirror.finish().unwrap();

        let records = records.lock().unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["continued"], true);
        assert_eq!(records[0]["fragment_index"], 0);
        assert_eq!(records[1]["continued"], false);
        assert_eq!(records[1]["fragment_index"], 1);
        assert!(serde_json::to_vec(&records[0]).unwrap().len() < 1024 * 1024);
    }

    #[test]
    fn emits_one_truncation_record_at_the_configured_limit() {
        let (mut mirror, records) = recording_mirror(Some(4));
        mirror.push(b"abcdef").unwrap();
        mirror.push(b"more").unwrap();
        mirror.finish().unwrap();

        let records = records.lock().unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["body"], "abcd");
        assert_eq!(records[1]["truncated"], true);
    }
}
