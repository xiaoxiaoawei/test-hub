//! Incremental byte parsing keeps UTF-8 and SSE framing independent of HTTP reads.

use super::{
    check_api_error, check_cancel, check_finish_reason, content_text, finish_translation,
    parse_response, response_too_large, MAX_RESPONSE_BYTES,
};
use serde_json::Value;

pub(super) struct ResponseDecoder {
    mode: Mode,
    body: Vec<u8>,
    received: usize,
}

enum Mode {
    Detect,
    Json,
    Sse(SseDecoder),
}

impl ResponseDecoder {
    pub(super) fn new(streaming: bool) -> Self {
        Self {
            mode: if streaming { Mode::Detect } else { Mode::Json },
            body: Vec::new(),
            received: 0,
        }
    }

    pub(super) fn push<F: FnMut(&str), C: Fn() -> bool>(
        &mut self,
        bytes: &[u8],
        should_cancel: &C,
        on_progress: &mut F,
    ) -> Result<(), String> {
        check_cancel(should_cancel)?;
        self.received = self.received.saturating_add(bytes.len());
        if self.received > MAX_RESPONSE_BYTES {
            return Err(response_too_large());
        }
        match &mut self.mode {
            Mode::Json => self.body.extend_from_slice(bytes),
            Mode::Sse(sse) => sse.push(bytes, should_cancel, on_progress)?,
            Mode::Detect => {
                self.body.extend_from_slice(bytes);
                if let Some(first) = self.body.iter().find(|byte| !byte.is_ascii_whitespace()) {
                    if matches!(first, b'{' | b'[') {
                        // Some compatible services ignore stream:true and return JSON.
                        self.mode = Mode::Json;
                    } else {
                        let mut sse = SseDecoder::default();
                        sse.push(&self.body, should_cancel, on_progress)?;
                        self.body.clear();
                        self.mode = Mode::Sse(sse);
                    }
                }
            }
        }
        check_cancel(should_cancel)
    }

    pub(super) fn is_complete(&self) -> bool {
        matches!(&self.mode, Mode::Sse(sse) if sse.done)
    }

    pub(super) fn finish<F: FnMut(&str), C: Fn() -> bool>(
        mut self,
        should_cancel: &C,
        on_progress: &mut F,
    ) -> Result<String, String> {
        check_cancel(should_cancel)?;
        match &mut self.mode {
            Mode::Sse(sse) => sse.finish(should_cancel, on_progress),
            Mode::Json | Mode::Detect => {
                let translated = parse_response(200, &self.body)?;
                check_cancel(should_cancel)?;
                on_progress(&translated);
                check_cancel(should_cancel)?;
                Ok(translated)
            }
        }
    }
}

#[derive(Default)]
struct SseDecoder {
    line: Vec<u8>,
    event_data: Vec<u8>,
    event_error: bool,
    skip_lf: bool,
    saw_line: bool,
    translated: String,
    finished: bool,
    done: bool,
}

impl SseDecoder {
    fn push<F: FnMut(&str), C: Fn() -> bool>(
        &mut self,
        bytes: &[u8],
        should_cancel: &C,
        on_progress: &mut F,
    ) -> Result<(), String> {
        for &byte in bytes {
            if self.done {
                break;
            }
            if self.skip_lf {
                self.skip_lf = false;
                if byte == b'\n' {
                    continue;
                }
            }
            match byte {
                b'\r' | b'\n' => {
                    check_cancel(should_cancel)?;
                    self.process_line(should_cancel, on_progress)?;
                    self.skip_lf = byte == b'\r';
                }
                _ => self.line.push(byte),
            }
        }
        Ok(())
    }

    fn process_line<F: FnMut(&str), C: Fn() -> bool>(
        &mut self,
        should_cancel: &C,
        on_progress: &mut F,
    ) -> Result<(), String> {
        let line = std::mem::take(&mut self.line);
        let line = std::str::from_utf8(&line).map_err(|_| invalid_stream())?;
        let line = if self.saw_line {
            line
        } else {
            self.saw_line = true;
            line.strip_prefix('\u{feff}').unwrap_or(line)
        };
        if line.is_empty() {
            return self.dispatch(should_cancel, on_progress);
        }
        if line.starts_with(':') {
            return Ok(());
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "data" => {
                if !self.event_data.is_empty() {
                    self.event_data.push(b'\n');
                }
                self.event_data.extend_from_slice(value.as_bytes());
            }
            "event" => self.event_error = value == "error",
            _ => {}
        }
        Ok(())
    }

    fn dispatch<F: FnMut(&str), C: Fn() -> bool>(
        &mut self,
        should_cancel: &C,
        on_progress: &mut F,
    ) -> Result<(), String> {
        check_cancel(should_cancel)?;
        let error = std::mem::take(&mut self.event_error);
        if error {
            return Err("翻译服务在流式响应中返回错误，请检查服务配置和额度。".into());
        }
        if self.event_data.is_empty() {
            return Ok(());
        }
        let data = std::mem::take(&mut self.event_data);
        let data = std::str::from_utf8(&data)
            .map_err(|_| invalid_stream())?
            .trim();
        if data == "[DONE]" {
            self.done = true;
            return Ok(());
        }
        let event: Value = serde_json::from_str(data).map_err(|_| invalid_stream())?;
        check_api_error(&event)?;
        let Some(choice) = event.pointer("/choices/0") else {
            // Usage-only chunks (choices:[]) contain no translation.
            if event
                .get("choices")
                .and_then(Value::as_array)
                .is_some_and(Vec::is_empty)
            {
                return Ok(());
            }
            return Err(invalid_stream());
        };
        self.finished |= check_finish_reason(choice.get("finish_reason"))?;
        if let Some(content) = choice.pointer("/delta/content") {
            // DeepSeek reasoning_content is deliberately never read or displayed.
            let delta = content_text(content);
            if !delta.is_empty() {
                if self.translated.len().saturating_add(delta.len()) > MAX_RESPONSE_BYTES {
                    return Err(response_too_large());
                }
                self.translated.push_str(&delta);
                check_cancel(should_cancel)?;
                on_progress(&self.translated);
                check_cancel(should_cancel)?;
            }
        }
        Ok(())
    }

    fn finish<F: FnMut(&str), C: Fn() -> bool>(
        &mut self,
        should_cancel: &C,
        on_progress: &mut F,
    ) -> Result<String, String> {
        check_cancel(should_cancel)?;
        if !self.done {
            if !self.line.is_empty() {
                self.process_line(should_cancel, on_progress)?;
            }
            self.dispatch(should_cancel, on_progress)?;
        }
        if !self.done && !self.finished {
            return Err("流式译文在完成前断开，请重试。".into());
        }
        check_cancel(should_cancel)?;
        finish_translation(&self.translated)
    }
}

fn invalid_stream() -> String {
    "接口没有返回有效的 Chat Completions 流式数据，请检查服务兼容性。".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn event(text: &str) -> String {
        format!(
            "data: {}\r\n\r\n",
            serde_json::json!({"choices":[{"delta":{"content":text}}]})
        )
    }

    #[test]
    fn arbitrary_byte_splits_preserve_unicode_and_emit_incremental_content() {
        let wire = format!("\u{feff}: keepalive\r\n\r\n{}{}data: {{\"choices\":[{{\"delta\":{{\"reasoning_content\":\"private reasoning\"}},\"finish_reason\":null}}]}}\n\ndata: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"stop\"}}]}}\n\ndata: [DONE]\n\n", event("你好"), event("，世界🌍\n{{name}}"));
        for split in 1..=wire.len() {
            let mut decoder = ResponseDecoder::new(true);
            let mut progress = Vec::new();
            let mut callback = |text: &str| progress.push(text.to_owned());
            for part in wire.as_bytes().chunks(split) {
                decoder.push(part, &|| false, &mut callback).unwrap();
            }
            assert!(decoder.is_complete());
            assert_eq!(
                decoder.finish(&|| false, &mut callback).unwrap(),
                "你好，世界🌍\n{{name}}"
            );
            assert_eq!(progress, ["你好", "你好，世界🌍\n{{name}}"]);
        }
    }

    #[test]
    fn multiline_data_cr_framing_and_finish_without_done_are_supported() {
        let wire = b"event: message\rdata: {\"choices\":\rdata: [{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\r\r";
        let mut decoder = ResponseDecoder::new(true);
        decoder.push(wire, &|| false, &mut |_| {}).unwrap();
        assert_eq!(decoder.finish(&|| false, &mut |_| {}).unwrap(), "ok");
    }

    #[test]
    fn ordinary_json_fallback_and_safe_error_messages() {
        let body = serde_json::to_vec(
            &serde_json::json!({"choices":[{"message":{"content":"译文"},"finish_reason":"stop"}]}),
        )
        .unwrap();
        let mut decoder = ResponseDecoder::new(true);
        let mut progress = Vec::new();
        for bytes in body.chunks(2) {
            decoder
                .push(bytes, &|| false, &mut |_| panic!("JSON is incomplete"))
                .unwrap();
        }
        assert_eq!(
            decoder
                .finish(&|| false, &mut |text| progress.push(text.to_owned()))
                .unwrap(),
            "译文"
        );
        assert_eq!(progress, ["译文"]);
        for wire in [
            "data: {\"error\":{\"message\":\"secret-must-never-echo\"}}\n\n".to_owned(),
            "event: error\ndata: secret-must-never-echo\n\n".to_owned(),
            "data: secret-must-never-echo\n\n".to_owned(),
            format!(
                "{}data: {{\"choices\":[{{\"finish_reason\":\"secret-must-never-echo\"}}]}}\n\n",
                event("partial")
            ),
        ] {
            let mut decoder = ResponseDecoder::new(true);
            let error = decoder
                .push(wire.as_bytes(), &|| false, &mut |_| {})
                .unwrap_err();
            assert!(!error.contains("secret-must-never-echo"));
        }
    }

    #[test]
    fn cancellation_stops_before_parse_and_immediately_after_progress() {
        let mut decoder = ResponseDecoder::new(true);
        assert!(super::super::is_cancelled_error(
            &decoder
                .push(b"invalid", &|| true, &mut |_| panic!())
                .unwrap_err()
        ));
        let cancelled = Cell::new(false);
        let mut progress = Vec::new();
        let wire = format!("{}{}data: [DONE]\n\n", event("first"), event("second"));
        let mut decoder = ResponseDecoder::new(true);
        let error = decoder
            .push(wire.as_bytes(), &|| cancelled.get(), &mut |text| {
                progress.push(text.to_owned());
                cancelled.set(true);
            })
            .unwrap_err();
        assert!(super::super::is_cancelled_error(&error));
        assert_eq!(progress, ["first"]);
        let mut decoder = ResponseDecoder::new(true);
        decoder
            .push(event("first").as_bytes(), &|| false, &mut |_| {})
            .unwrap();
        assert!(super::super::is_cancelled_error(
            &decoder.finish(&|| true, &mut |_| panic!()).unwrap_err()
        ));
    }

    #[test]
    fn truncated_failed_oversized_and_invalid_utf8_streams_fail() {
        for finish_reason in ["length", "content_filter", "tool_calls"] {
            let wire = format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"partial\"}},\"finish_reason\":\"{finish_reason}\"}}]}}\n\n");
            assert!(ResponseDecoder::new(true)
                .push(wire.as_bytes(), &|| false, &mut |_| {})
                .is_err());
        }
        let mut decoder = ResponseDecoder::new(true);
        decoder
            .push(event("partial").as_bytes(), &|| false, &mut |_| {})
            .unwrap();
        assert!(decoder
            .finish(&|| false, &mut |_| {})
            .unwrap_err()
            .contains("断开"));
        assert!(ResponseDecoder::new(true)
            .push(&vec![b' '; MAX_RESPONSE_BYTES + 1], &|| false, &mut |_| {})
            .is_err());
        assert!(ResponseDecoder::new(true)
            .push(b"data: \xff\n\n", &|| false, &mut |_| {})
            .is_err());
    }
}
