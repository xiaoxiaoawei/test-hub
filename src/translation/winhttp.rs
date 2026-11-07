use super::stream::ResponseDecoder;
use super::{check_cancel, http_error};
use crate::config::{parse_endpoint, Endpoint, Settings};
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Networking::WinHttp::*;

#[path = "winhttp/asynchronous.rs"]
mod asynchronous;
use asynchronous::Request;

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
    request_slots: std::sync::Arc<asynchronous::Slots>,
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
                    WINHTTP_FLAG_ASYNC,
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
        body: Vec<u8>,
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
        let request = Request::new(request, wide(&headers), body, self.request_slots.clone())?;
        request.send(deadline, should_cancel)?;
        request.receive(deadline, should_cancel)?;
        let mut status = 0_u32;
        let mut status_size = std::mem::size_of::<u32>() as u32;
        let queried = unsafe {
            WinHttpQueryHeaders(
                request.raw(),
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
        loop {
            let available = request.available(deadline, should_cancel)?;
            if available == 0 {
                break;
            }
            let received = request.read(available, deadline, should_cancel, |bytes| {
                decoder.push(bytes, should_cancel, &mut on_progress)
            })?;
            if received == 0 || decoder.is_complete() {
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
    network_error_code(unsafe { GetLastError() })
}

fn network_error_code(code: u32) -> String {
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
    fn winhttp_cancels_a_request_waiting_for_headers_and_reuses_the_client() {
        assert_blocked_request_is_cancelled(false);
    }

    #[test]
    fn winhttp_cancels_a_stalled_sse_body_and_reuses_the_client() {
        assert_blocked_request_is_cancelled(true);
    }

    fn assert_blocked_request_is_cancelled(stalled_body: bool) {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (progress_tx, progress_rx) = mpsc::channel();
        let (old_tx, old_rx) = mpsc::channel();
        let (next_tx, next_rx) = mpsc::channel();
        let (finish_tx, finish_rx) = mpsc::channel();
        let (slots_tx, slots_rx) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        let service = std::thread::spawn(move || {
            let mut old = accept(&listener);
            assert_eq!(
                read_request(&mut old)["messages"][1]["content"],
                "old passage"
            );
            if stalled_body {
                old.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\n\r\n").unwrap();
                let first = format!(
                    "data: {}\n\n",
                    json!({"choices":[{"delta":{"content":"old partial"}}]})
                );
                chunk(&mut old, first.as_bytes());
                chunk(&mut old, b": heartbeat\n\n");
            }
            started_tx.send(()).unwrap();
            let blocked = std::thread::spawn(move || {
                // Deliberately do not finish the old response or close its TCP
                // socket until both cancellation and the next request returned.
                let _ = release_rx.recv_timeout(Duration::from_secs(8));
                drop(old);
            });
            let mut next = accept(&listener);
            assert_eq!(
                read_request(&mut next)["messages"][1]["content"],
                "new passage"
            );
            let response = serde_json::to_vec(
                &json!({"choices":[{"message":{"content":"new translation"},"finish_reason":"stop"}]}),
            )
            .unwrap();
            write!(next, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", response.len()).unwrap();
            next.write_all(&response).unwrap();
            next.flush().unwrap();
            blocked.join().unwrap();
        });
        let worker = std::thread::spawn(move || {
            let settings = Settings {
                base_url: format!("http://127.0.0.1:{port}/v1"),
                timeout_seconds: 10,
                ..Settings::default()
            };
            let mut client = TranslationClient::new();
            let old = client.translate_streaming(
                &settings,
                "old passage",
                || worker_cancelled.load(Ordering::Acquire),
                |text| {
                    let _ = progress_tx.send(text.to_owned());
                },
            );
            let _ = old_tx.send(old);
            let next = client.translate_streaming(&settings, "new passage", || false, |_| {});
            let _ = next_tx.send(next);
            let _ = finish_rx.recv_timeout(Duration::from_secs(5));
            let deadline = Instant::now() + Duration::from_secs(2);
            while client.http.request_slots.live() != 0 && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(1));
            }
            let _ = slots_tx.send(client.http.request_slots.live());
        });
        started_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        if stalled_body {
            assert_eq!(
                progress_rx.recv_timeout(Duration::from_secs(3)).unwrap(),
                "old partial"
            );
        }
        // Ensure the worker has actually reached the absent headers/body wait.
        std::thread::sleep(Duration::from_millis(80));
        let started = Instant::now();
        cancelled.store(true, Ordering::Release);
        let old = old_rx.recv_timeout(Duration::from_secs(2));
        let cancel_elapsed = started.elapsed();
        let next = next_rx.recv_timeout(Duration::from_secs(2));
        // Opening the gate only here means a passing test cannot depend on the
        // abandoned server becoming responsive or its configured ten-second timeout.
        let _ = release_tx.send(());
        service.join().unwrap();
        let _ = finish_tx.send(());
        worker.join().unwrap();
        assert_eq!(slots_rx.recv_timeout(Duration::from_secs(2)).unwrap(), 0);
        let error = old
            .expect("blocked request did not cancel promptly")
            .unwrap_err();
        assert!(crate::translation::is_cancelled_error(&error), "{error}");
        assert!(
            cancel_elapsed < Duration::from_secs(2),
            "{cancel_elapsed:?}"
        );
        assert_eq!(next.unwrap().unwrap(), "new translation");
        eprintln!("cancelled stalled_body={stalled_body} in {cancel_elapsed:?}; subsequent translation completed while the old server was held");
    }

    #[test]
    fn winhttp_enforces_the_whole_request_deadline_while_server_is_held() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (release_tx, release_rx) = mpsc::channel();
        let service = std::thread::spawn(move || {
            let mut socket = accept(&listener);
            read_request(&mut socket);
            // Never send even the response headers until the client has returned.
            let _ = release_rx.recv_timeout(Duration::from_secs(5));
            drop(socket);
        });
        let settings = Settings {
            base_url: format!("http://127.0.0.1:{port}/v1"),
            timeout_seconds: 1,
            ..Settings::default()
        };
        let started = Instant::now();
        let result =
            TranslationClient::new().translate_streaming(&settings, "a passage", || false, |_| {});
        let elapsed = started.elapsed();
        let _ = release_tx.send(());
        service.join().unwrap();
        assert!(result.unwrap_err().contains("超时"));
        assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
    }

    #[test]
    fn winhttp_async_error_and_http_errors_close_requests_without_echoing_bodies() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let service = std::thread::spawn(move || {
            let mut socket = accept(&listener);
            read_request(&mut socket);
            let body = "fixed-reflected-secret-placeholder";
            write!(socket, "HTTP/1.1 401 Unauthorized\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            socket.flush().unwrap();
        });
        let settings = Settings {
            base_url: format!("http://127.0.0.1:{port}/v1"),
            timeout_seconds: 2,
            ..Settings::default()
        };
        let error = TranslationClient::new()
            .translate_streaming(&settings, "a passage", || false, |_| {})
            .unwrap_err();
        service.join().unwrap();
        assert!(error.contains("401"));
        assert!(!error.contains("fixed-reflected-secret-placeholder"));

        // The listener is gone, so SendRequest completes asynchronously with a
        // connection error. That failure must still close and quiesce callbacks.
        let error = TranslationClient::new()
            .translate_streaming(&settings, "a passage", || false, |_| {})
            .unwrap_err();
        assert!(error.contains("WinHTTP") || error.contains("超时"));
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
