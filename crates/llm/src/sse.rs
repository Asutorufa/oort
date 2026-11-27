use std::pin::Pin;

use bytes::Bytes;
use futures_util::{Stream, StreamExt};

use crate::Error;

/// A single decoded Server-Sent Event (SSE).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
    pub id: Option<String>,
}

/// Incremental byte-level SSE parser that safely handles arbitrary TCP chunk boundaries,
/// including multi-byte UTF-8 sequences split across chunks, `\r\n` and `\n` line endings,
/// multi-line `data:` fields, and comment lines starting with `:`.
#[derive(Debug, Default)]
pub struct SseParser {
    byte_buf: Vec<u8>,
    event_type: Option<String>,
    data_lines: Vec<String>,
    event_id: Option<String>,
    has_data: bool,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds raw bytes into the parser and returns any complete SSE events decoded so far.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.byte_buf.extend_from_slice(chunk);
        let mut events = Vec::new();

        while let Some(newline_pos) = self.byte_buf.iter().position(|&b| b == b'\n') {
            let mut line_bytes: Vec<u8> = self.byte_buf.drain(..=newline_pos).collect();
            // Pop trailing '\n'
            line_bytes.pop();
            // Pop optional trailing '\r'
            if line_bytes.last() == Some(&b'\r') {
                line_bytes.pop();
            }

            let line = String::from_utf8_lossy(&line_bytes);
            if let Some(ev) = self.process_line(&line) {
                events.push(ev);
            }
        }

        events
    }

    /// Flushes any trailing unterminated line and dispatches a pending event at EOF.
    pub fn finish(&mut self) -> Option<SseEvent> {
        if !self.byte_buf.is_empty() {
            let mut line_bytes = std::mem::take(&mut self.byte_buf);
            if line_bytes.last() == Some(&b'\r') {
                line_bytes.pop();
            }
            let line = String::from_utf8_lossy(&line_bytes);
            if let Some(ev) = self.process_line(&line) {
                return Some(ev);
            }
        }
        self.dispatch()
    }

    fn process_line(&mut self, line: &str) -> Option<SseEvent> {
        if line.is_empty() {
            return self.dispatch();
        }
        // Comment line
        if line.starts_with(':') {
            return None;
        }

        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };

        match field {
            "event" => {
                self.event_type = Some(value.to_string());
            }
            "data" => {
                self.has_data = true;
                self.data_lines.push(value.to_string());
            }
            "id" if !value.contains('\0') => {
                self.event_id = Some(value.to_string());
            }
            _ => {}
        }

        None
    }

    fn dispatch(&mut self) -> Option<SseEvent> {
        if !self.has_data {
            self.event_type = None;
            return None;
        }

        let event = SseEvent {
            event: self.event_type.take(),
            data: self.data_lines.join("\n"),
            id: self.event_id.clone(),
        };
        self.data_lines.clear();
        self.has_data = false;
        Some(event)
    }
}

/// Converts a byte chunk stream into a pinned, `Unpin` stream of decoded [`SseEvent`]s.
pub fn parse_sse_stream<S, E>(
    stream: S,
) -> Pin<Box<dyn Stream<Item = Result<SseEvent, Error>> + Send + 'static>>
where
    S: Stream<Item = Result<Bytes, E>> + Send + Unpin + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    use std::collections::VecDeque;

    Box::pin(futures_util::stream::unfold(
        (stream, SseParser::new(), VecDeque::<SseEvent>::new(), false),
        |(mut stream, mut parser, mut pending, mut finished)| async move {
            loop {
                if let Some(event) = pending.pop_front() {
                    return Some((Ok(event), (stream, parser, pending, finished)));
                }
                if finished {
                    return None;
                }

                match stream.next().await {
                    Some(Ok(chunk)) => {
                        for event in parser.push(&chunk) {
                            pending.push_back(event);
                        }
                    }
                    Some(Err(err)) => {
                        return Some((
                            Err(Error::Stream(err.to_string())),
                            (stream, parser, pending, true),
                        ));
                    }
                    None => {
                        finished = true;
                        if let Some(event) = parser.finish() {
                            pending.push_back(event);
                        }
                    }
                }
            }
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;

    #[test]
    fn test_sse_utf8_split_across_chunks() {
        // "你好世界" in UTF-8 bytes
        let full = "event: delta\ndata: 你好世界\n\n".as_bytes();
        let mut parser = SseParser::new();
        let mut collected = Vec::new();
        // Feed 1 byte at a time to stress-test multi-byte UTF-8 boundaries!
        for byte in full {
            collected.extend(parser.push(std::slice::from_ref(byte)));
        }
        if let Some(ev) = parser.finish() {
            collected.push(ev);
        }

        assert_eq!(collected.len(), 1);
        assert_eq!(collected[0].event.as_deref(), Some("delta"));
        assert_eq!(collected[0].data, "你好世界");
    }

    #[test]
    fn test_sse_multiline_data_and_comments_and_crlf() {
        let payload = concat!(
            ": keep-alive comment\r\n",
            "id: 42\r\n",
            "event: message\r\n",
            "data: line1\r\n",
            "data: line2\r\n",
            "\r\n",
            "data:no_space\n",
            "\n"
        );

        let mut parser = SseParser::new();
        let events = parser.push(payload.as_bytes());
        assert_eq!(events.len(), 2);

        assert_eq!(events[0].event.as_deref(), Some("message"));
        assert_eq!(events[0].id.as_deref(), Some("42"));
        assert_eq!(events[0].data, "line1\nline2");

        assert_eq!(events[1].event, None);
        assert_eq!(events[1].id.as_deref(), Some("42"));
        assert_eq!(events[1].data, "no_space");
    }

    #[tokio::test]
    async fn test_parse_sse_stream_eof_without_trailing_blank_line() {
        let chunks = vec![
            Ok::<_, std::io::Error>(Bytes::from("data: first\n\n")),
            Ok::<_, std::io::Error>(Bytes::from("data: last_without_newline")),
        ];

        let events: Vec<_> = parse_sse_stream(stream::iter(chunks))
            .collect::<Vec<_>>()
            .await;

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].as_ref().unwrap().data, "first");
        assert_eq!(events[1].as_ref().unwrap().data, "last_without_newline");
    }

    #[tokio::test]
    async fn test_parse_sse_stream_propagates_io_error() {
        let chunks = vec![
            Ok(Bytes::from("data: ok\n\n")),
            Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "connection reset",
            )),
        ];

        let results: Vec<_> = parse_sse_stream(stream::iter(chunks)).collect().await;
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].as_ref().unwrap().data, "ok");
        assert!(matches!(results[1], Err(Error::Stream(_))));
    }
}
