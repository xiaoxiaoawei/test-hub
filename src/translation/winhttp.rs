use super::stream::ResponseDecoder;
use super::{check_cancel, http_error};
use crate::config::{parse_endpoint, Endpoint, Settings};
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Networking::WinHttp::*;

struct HttpHandle(*mut c_void);

impl HttpHandle {
    fn from_raw(handle: *mut c_void) -> Result<Self, String> {
        if handle.is_null() {
            Err(network_error())
        } else {
            Ok(Self(handle))
        }
    }
}

impl Drop for HttpHandle {
    fn drop(&mut self) {
        // SAFETY: Only this worker owns the handle, which came from WinHTTP.
        unsafe { WinHttpCloseHandle(self.0) };
    }
}

struct Connection {
    handle: HttpHandle,
    host: String,
    port: u16,
    secure: bool,
}

#[derive(Default)]
pub(super) struct HttpClient {
    // Field order closes the child connection before either parent session.
    connection: Option<Connection>,
    local_session: Option<HttpHandle>,
    remote_session: Option<HttpHandle>,
}

impl HttpClient {
    fn connection(
        &mut self,
        endpoint: &Endpoint,
        deadline: Instant,
    ) -> Result<*mut c_void, String> {
        let local = endpoint.host.eq_ignore_ascii_case("localhost")
            || endpoint
                .host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback());
        let session_slot = if local {
            &mut self.local_session
        } else {
            &mut self.remote_session
        };
        if session_slot.is_none() {
            let agent = wide(concat!("LingoLeaf/", env!("CARGO_PKG_VERSION")));
            // SAFETY: Strings are NUL terminated. Normal TLS validation and the
            // system's automatic proxy apply; only loopback bypasses the proxy.
            *session_slot = Some(HttpHandle::from_raw(unsafe {
                WinHttpOpen(
                    agent.as_ptr(),
                    if local {
                        WINHTTP_ACCESS_TYPE_NO_PROXY
                    } else {
                        WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY
                    },
                    null(),
                    null(),
                    0,
                )
            })?);
        }
        let session = session_slot.as_ref().expect("session was initialized");
        set_timeouts(session, remaining_ms(deadline)?)?;
        if !self.connection.as_ref().is_some_and(|connection| {
            connection.host.eq_ignore_ascii_case(&endpoint.host)
                && connection.port == endpoint.port
                && connection.secure == endpoint.secure
        }) {
            self.connection = None;
            let host = wide(&endpoint.host);
            let handle = HttpHandle::from_raw(unsafe {
                WinHttpConnect(session.0, host.as_ptr(), endpoint.port, 0)
            })?;
            self.connection = Some(Connection {
                handle,
                host: endpoint.host.clone(),
                port: endpoint.port,
                secure: endpoint.secure,
            });
        }
        Ok(self
            .connection
            .as_ref()
            .expect("connection was initialized")
            .handle
            .0)
    }

    pub(super) fn post<F: FnMut(&str), C: Fn() -> bool>(
        &mut self,
        settings: &Settings,
        body: &[u8],
        streaming: bool,
        should_cancel: &C,
        mut on_progress: F,
    ) -> Result<String, String> {
        check_cancel(should_cancel)?;
        let endpoint = parse_endpoint(&settings.base_url)?;
        let deadline = Instant::now() + Duration::from_secs(settings.timeout_seconds as u64);
        let connection = self.connection(&endpoint, deadline);
        check_cancel(should_cancel)?;
        let connection = connection?;
        let method = wide("POST");
        let path = wide(&endpoint.path);
        let request = HttpHandle::from_raw(unsafe {
            WinHttpOpenRequest(
                connection,
                method.as_ptr(),
                path.as_ptr(),
                null(),
                null(),
                null(),
                if endpoint.secure {
                    WINHTTP_FLAG_SECURE
                } else {
                    0
                },
            )
        });
        check_cancel(should_cancel)?;
        let request = request?;
        let no_redirect = WINHTTP_OPTION_REDIRECT_POLICY_NEVER;
        // Never forward the authorization header to a redirected origin.
        if unsafe {
            WinHttpSetOption(
                request.0,
                WINHTTP_OPTION_REDIRECT_POLICY,
                (&no_redirect as *const u32).cast(),
                std::mem::size_of::<u32>() as u32,
            )
        } == 0
        {
            return Err(network_error());
        }
        let mut headers = format!(
            "Content-Type: application/json; charset=utf-8\r\nAccept: {}\r\n",
            if streaming {
                "text/event-stream, application/json"
            } else {
                "application/json"
            },
        );
        if !settings.api_key.is_empty() {
            headers.push_str("Authorization: Bearer ");
            headers.push_str(&settings.api_key);
            headers.push_str("\r\n");
        }
        let headers = wide(&headers);
        let body_length = u32::try_from(body.len()).map_err(|_| "请求内容过长。".to_owned())?;
        check_cancel(should_cancel)?;
        set_timeouts(&request, remaining_ms(deadline)?)?;
        let sent = unsafe {
            WinHttpSendRequest(
                request.0,
                headers.as_ptr(),
                (headers.len() - 1) as u32,
                body.as_ptr().cast(),
                body_length,
                body_length,
                0,
            )
        };
        let send_error = (sent == 0).then(network_error);
        check_cancel(should_cancel)?;
        if let Some(error) = send_error {
            return Err(error);
        }
        set_timeouts(&request, remaining_ms(deadline)?)?;
        let received = unsafe { WinHttpReceiveResponse(request.0, null_mut()) };
        let receive_error = (received == 0).then(network_error);
        check_cancel(should_cancel)?;
        if let Some(error) = receive_error {
            return Err(error);
        }
        let mut status = 0_u32;
        let mut status_size = std::mem::size_of::<u32>() as u32;
        let queried = unsafe {
            WinHttpQueryHeaders(
                request.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                null(),
                (&mut status as *mut u32).cast(),
                &mut status_size,
                null_mut(),
            )
        };
        let query_error = (queried == 0).then(network_error);
        check_cancel(should_cancel)?;
        if let Some(error) = query_error {
            return Err(error);
        }
        if !(200..300).contains(&status) {
            // Do not read error pages; upstream gateways may echo credentials.
            return Err(http_error(status));
        }
        let mut decoder = ResponseDecoder::new(streaming);
        let mut chunk = [0_u8; 8192];
        loop {
            check_cancel(should_cancel)?;
            set_timeouts(&request, remaining_ms(deadline)?)?;
            // A large synchronous ReadData can wait to fill its requested size.
            // Query first so each available SSE fragment reaches the UI promptly.
            let mut available = 0_u32;
            let queried = unsafe { WinHttpQueryDataAvailable(request.0, &mut available) };
            let query_error = (queried == 0).then(network_error);
            check_cancel(should_cancel)?;
            if let Some(error) = query_error {
                return Err(error);
            }
            if available == 0 {
                break;
            }
            set_timeouts(&request, remaining_ms(deadline)?)?;
            let mut received = 0_u32;
            let read = unsafe {
                WinHttpReadData(
                    request.0,
                    chunk.as_mut_ptr().cast(),
                    available.min(chunk.len() as u32),
                    &mut received,
                )
            };
            let read_error = (read == 0).then(network_error);
            check_cancel(should_cancel)?;
            if let Some(error) = read_error {
                return Err(error);
            }
            if received == 0 {
                break;
            }
            decoder.push(&chunk[..received as usize], should_cancel, &mut on_progress)?;
            if decoder.is_complete() {
                break;
            }
        }
        decoder.finish(should_cancel, &mut on_progress)
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn network_error() -> String {
    // GetLastError is read immediately after its failing WinHTTP call.
    let code = unsafe { GetLastError() };
    let message = match code {
        ERROR_WINHTTP_TIMEOUT => "网络请求超时，请检查连接或增加超时。",
        ERROR_WINHTTP_NAME_NOT_RESOLVED => "无法解析服务域名，请检查服务地址和网络。",
        ERROR_WINHTTP_CANNOT_CONNECT => "无法连接翻译服务，请检查地址、端口和系统代理。",
        ERROR_WINHTTP_SECURE_FAILURE => "HTTPS 证书验证失败，请检查系统时间和证书；未绕过验证。",
        ERROR_WINHTTP_OPERATION_CANCELLED => "网络请求已取消。",
        _ => "网络请求失败，请检查服务地址、网络和系统代理。",
    };
    format!("{message}（WinHTTP {code}）")
}

fn set_timeouts(handle: &HttpHandle, milliseconds: i32) -> Result<(), String> {
    if unsafe {
        WinHttpSetTimeouts(
            handle.0,
            milliseconds,
            milliseconds,
            milliseconds,
            milliseconds,
        )
    } == 0
    {
        Err(network_error())
    } else {
        Ok(())
    }
}

fn remaining_ms(deadline: Instant) -> Result<i32, String> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or_else(|| "翻译请求已超时，请稍后重试。".to_owned())?;
    Ok(remaining.as_millis().clamp(1, i32::MAX as u128) as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::translation::{ConversationContext, ConversationTurn, TranslationClient};
    use serde_json::{json, Value};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc;

    fn read_request(socket: &mut TcpStream) -> Value {
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        let body_start = loop {
            let read = socket.read(&mut buffer).unwrap();
            assert_ne!(read, 0);
            request.extend_from_slice(&buffer[..read]);
            assert!(request.len() < 32 * 1024);
            if let Some(offset) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                break offset + 4;
            }
        };
        let headers = String::from_utf8(request[..body_start].to_vec()).unwrap();
        assert!(headers.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"));
        let length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap();
        while request.len() - body_start < length {
            let read = socket.read(&mut buffer).unwrap();
            assert_ne!(read, 0);
            request.extend_from_slice(&buffer[..read]);
        }
        serde_json::from_slice(&request[body_start..body_start + length]).unwrap()
    }

    fn accept(listener: &TcpListener) -> TcpStream {
        listener.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match listener.accept() {
                Ok((socket, _)) => {
                    socket.set_nonblocking(false).unwrap();
                    return socket;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "WinHTTP did not connect");
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("mock accept failed: {error}"),
            }
        }
    }

    fn chunk(socket: &mut TcpStream, body: &[u8]) {
        write!(socket, "{:X}\r\n", body.len()).unwrap();
        socket.write_all(body).unwrap();
        socket.write_all(b"\r\n").unwrap();
        socket.flush().unwrap();
    }

    #[test]
    fn winhttp_delivers_progress_before_service_finishes_sse() {
        assert_progress_before_stream_finishes(false);
    }

    #[test]
    fn winhttp_decodes_chunked_sse_and_delivers_progress_before_done() {
        assert_progress_before_stream_finishes(true);
    }

    fn assert_progress_before_stream_finishes(chunked: bool) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (progress_tx, progress_rx) = mpsc::channel();
        let service = std::thread::spawn(move || {
            let mut socket = accept(&listener);
            let request = read_request(&mut socket);
            assert_eq!(request["stream"], true);
            assert_eq!(request["messages"].as_array().unwrap().len(), 2);
            let first = format!(
                "data: {}\n\n",
                json!({"choices":[{"delta":{"content":"你好"}}]})
            );
            let rest = format!(
                "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                json!({"choices":[{"delta":{"content":"，世界。","reasoning_content":"not for display"}}]}),
                json!({"choices":[{"delta":{},"finish_reason":"stop"}]})
            );
            let framing = if chunked {
                "Transfer-Encoding: chunked".to_owned()
            } else {
                format!("Content-Length: {}", first.len() + rest.len())
            };
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n{framing}\r\nConnection: close\r\n\r\n").unwrap();
            // Split a UTF-8 code point across actual network writes as well.
            let split = first
                .as_bytes()
                .iter()
                .position(|byte| *byte >= 128)
                .unwrap()
                + 1;
            if chunked {
                chunk(&mut socket, &first.as_bytes()[..split]);
                chunk(&mut socket, &first.as_bytes()[split..]);
                // A normal SSE comment also lets WinHTTP read the next HTTP
                // chunk header without waiting for another translation token.
                chunk(&mut socket, b": heartbeat\n\n");
            } else {
                socket.write_all(&first.as_bytes()[..split]).unwrap();
                socket.flush().unwrap();
                socket.write_all(&first.as_bytes()[split..]).unwrap();
                socket.flush().unwrap();
            }
            // The service does not send DONE until the client exposes the first text.
            assert_eq!(
                progress_rx.recv_timeout(Duration::from_secs(3)).unwrap(),
                "你好"
            );
            if chunked {
                chunk(&mut socket, rest.as_bytes());
                socket.write_all(b"0\r\n\r\n").unwrap();
            } else {
                socket.write_all(rest.as_bytes()).unwrap();
            }
            socket.flush().unwrap();
        });
        let settings = Settings {
            base_url: format!("http://127.0.0.1:{port}/v1"),
            model: "mock-model".into(),
            timeout_seconds: 5,
            ..Settings::default()
        };
        let mut progress = Vec::new();
        let translated = TranslationClient::new().translate_streaming(
            &settings,
            "Hello, world.",
            || false,
            |text| {
                if progress.is_empty() {
                    progress_tx.send(text.to_owned()).unwrap();
                }
                progress.push(text.to_owned());
            },
        );
        service.join().unwrap();
        assert_eq!(translated.unwrap(), "你好，世界。");
        assert_eq!(progress, ["你好", "你好，世界。"]);
    }

    #[test]
    fn winhttp_reuses_connection_and_accepts_json_from_streaming_provider() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let service = std::thread::spawn(move || {
            let mut socket = accept(&listener);
            // Reading both requests on one TCP socket also proves keep-alive reuse.
            for _ in 0..2 {
                assert_eq!(read_request(&mut socket)["stream"], true);
                let body = serde_json::to_vec(
                    &json!({"choices":[{"message":{"content":"你好"},"finish_reason":"stop"}]}),
                )
                .unwrap();
                write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n", body.len()).unwrap();
                socket.write_all(&body).unwrap();
                socket.flush().unwrap();
            }
        });
        let settings = Settings {
            base_url: format!("http://127.0.0.1:{port}/v1"),
            model: "mock-model".into(),
            timeout_seconds: 5,
            ..Settings::default()
        };
        let mut client = TranslationClient::new();
        for _ in 0..2 {
            assert_eq!(
                client
                    .translate_streaming(&settings, "Hello", || false, |_| {})
                    .unwrap(),
                "你好"
            );
        }
        service.join().unwrap();
    }

    #[test]
    fn winhttp_streams_questions_with_reference_data_and_continuation_history() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let service = std::thread::spawn(move || {
            // The same socket serves both exchanges, so the Q&A method also
            // exercises the existing client's keep-alive connection reuse.
            let mut socket = accept(&listener);
            for number in 0..2 {
                let request = read_request(&mut socket);
                assert_eq!(request["stream"], true);
                assert_eq!(request["model"], "mock-question-model");
                let messages = request["messages"].as_array().unwrap();
                assert!(messages[0]["content"]
                    .as_str()
                    .unwrap()
                    .contains("primarily in Simplified Chinese"));
                let context: Value =
                    serde_json::from_str(messages[1]["content"].as_str().unwrap()).unwrap();
                assert_eq!(
                    context,
                    json!({"kind":"selection_context","source_text":"warm light","translation":"温暖的光"})
                );
                if number == 0 {
                    assert_eq!(messages.len(), 3);
                    assert_eq!(messages[2], json!({"role":"user","content":"Why warm?"}));
                } else {
                    assert_eq!(messages.len(), 5);
                    assert_eq!(messages[2], json!({"role":"user","content":"Why warm?"}));
                    assert_eq!(
                        messages[3],
                        json!({"role":"assistant","content":"描述色调。"})
                    );
                    assert_eq!(
                        messages[4],
                        json!({"role":"user","content":"Another example?"})
                    );
                }
                let answer = if number == 0 {
                    "描述色调。"
                } else {
                    "a warm glow。"
                };
                let response = format!(
                    "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                    json!({"choices":[{"delta":{"content":answer}}]}),
                    json!({"choices":[{"delta":{},"finish_reason":"stop"}]}),
                );
                write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n", response.len()).unwrap();
                socket.write_all(response.as_bytes()).unwrap();
                socket.flush().unwrap();
            }
        });
        let settings = Settings {
            base_url: format!("http://127.0.0.1:{port}/v1"),
            model: "mock-question-model".into(),
            timeout_seconds: 5,
            ..Settings::default()
        };
        let mut client = TranslationClient::new();
        let mut history = Vec::new();
        for (question, expected) in [
            ("Why warm?", "描述色调。"),
            ("Another example?", "a warm glow。"),
        ] {
            let mut progress = Vec::new();
            let answer = client
                .ask_streaming(
                    &settings,
                    ConversationContext {
                        source: "warm light",
                        translation: "温暖的光",
                        history: &history,
                    },
                    question,
                    || false,
                    |text| progress.push(text.to_owned()),
                )
                .unwrap();
            assert_eq!(answer, expected);
            assert_eq!(progress.last().unwrap(), expected);
            history.push(ConversationTurn {
                question: question.to_owned(),
                answer,
            });
        }
        service.join().unwrap();
    }
}
