//! Online speech generation and bounded in-memory WAV playback. The UI only
//! replaces a latest-wins command; network and audio handles stay on this worker.
use crate::{
    config::{parse_speech_endpoint, Endpoint, SpeechSettings},
    speech::{build_request, parse_wave, WaveAudio, MAX_AUDIO_BYTES},
    translation::winhttp::{
        asynchronous::{Request, Slots},
        HttpHandle,
    },
};
use std::{
    ffi::c_void,
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Condvar, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT},
    Media::Audio::*,
    Networking::WinHttp::*,
    System::Threading::{CreateEventW, ResetEvent, WaitForSingleObject},
};

const POLL: Duration = Duration::from_millis(20);
static AUDIO_FAULT: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Outcome {
    Started,
    Finished,
    Stopped,
}

#[derive(Debug)]
pub(crate) struct Event {
    pub token: u64,
    pub result: Result<Outcome, String>,
}

enum Command {
    Start {
        token: u64,
        text: String,
        settings: SpeechSettings,
    },
    Stop {
        token: u64,
    },
}
#[derive(Default)]
struct Pending {
    command: Option<Command>,
    closed: bool,
}
#[derive(Default)]
struct Shared {
    pending: Mutex<Pending>,
    changed: Condvar,
    latest: AtomicU64,
}

impl Shared {
    fn send(&self, make: impl FnOnce(u64) -> Command) -> u64 {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let token = self.latest.load(Ordering::Relaxed).wrapping_add(1);
        self.latest.store(token, Ordering::Release);
        pending.command = Some(make(token));
        self.changed.notify_one();
        token
    }
    fn current(&self, token: u64) -> bool {
        self.latest.load(Ordering::Acquire) == token
    }
    fn close(&self) {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        pending.closed = true;
        pending.command = None;
        self.latest.fetch_add(1, Ordering::AcqRel);
        self.changed.notify_one();
    }
}

pub(crate) struct Speech {
    shared: Arc<Shared>,
}
impl Speech {
    /// No synthesis, network access or audio device is touched at startup.
    pub(crate) fn new(notify: impl Fn(Event) + Send + 'static) -> Result<Self, String> {
        let shared = Arc::new(Shared::default());
        let worker = Arc::clone(&shared);
        thread::Builder::new()
            .name("online-speech".into())
            .spawn(move || run(worker, notify, || Ok(Online::default())))
            .map_err(|_| "无法启动朗读线程，请重新启动应用后再试。".to_string())?;
        Ok(Self { shared })
    }
    pub(crate) fn start(&self, text: String, settings: SpeechSettings) -> u64 {
        self.shared.send(|token| Command::Start {
            token,
            text,
            settings,
        })
    }
    pub(crate) fn stop(&self) -> u64 {
        self.shared.send(|token| Command::Stop { token })
    }
}
impl Drop for Speech {
    fn drop(&mut self) {
        self.shared.close();
    }
}

trait Engine {
    fn start(
        &mut self,
        text: &str,
        settings: &SpeechSettings,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<bool, String>;
    fn stop(&mut self) -> Result<(), String>;
    fn done(&self) -> Result<bool, String>;
}

fn run<E: Engine>(
    shared: Arc<Shared>,
    notify: impl Fn(Event),
    mut create: impl FnMut() -> Result<E, String>,
) {
    let mut engine: Option<E> = None;
    let mut active = None;
    loop {
        let command = {
            let mut pending = shared
                .pending
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            while pending.command.is_none() && !pending.closed {
                if active.is_some() {
                    let (next, _) = shared
                        .changed
                        .wait_timeout(pending, POLL)
                        .unwrap_or_else(|error| error.into_inner());
                    pending = next;
                    break;
                }
                pending = shared
                    .changed
                    .wait(pending)
                    .unwrap_or_else(|error| error.into_inner());
            }
            if pending.closed {
                break;
            }
            pending.command.take()
        };
        if let Some(command) = command {
            let token = match &command {
                Command::Start { token, .. } | Command::Stop { token } => *token,
            };
            if !shared.current(token) {
                continue;
            }
            active = None;
            if let Some(current) = &mut engine {
                if let Err(error) = current.stop() {
                    engine = None;
                    if shared.current(token) {
                        notify(Event {
                            token,
                            result: Err(error),
                        });
                    }
                    continue;
                }
            }
            match command {
                Command::Stop { .. } => {
                    if shared.current(token) {
                        notify(Event {
                            token,
                            result: Ok(Outcome::Stopped),
                        });
                    }
                }
                Command::Start { text, settings, .. } => {
                    if engine.is_none() {
                        match create() {
                            Ok(created) => engine = Some(created),
                            Err(error) => {
                                if shared.current(token) {
                                    notify(Event {
                                        token,
                                        result: Err(error),
                                    });
                                }
                                continue;
                            }
                        }
                    }
                    if !shared.current(token) {
                        continue;
                    }
                    let result =
                        engine
                            .as_mut()
                            .expect("engine exists")
                            .start(&text, &settings, &|| !shared.current(token));
                    if !shared.current(token) {
                        if let Some(current) = &mut engine {
                            let _ = current.stop();
                        }
                        continue;
                    }
                    match result {
                        Ok(true) => {
                            active = Some(token);
                            notify(Event {
                                token,
                                result: Ok(Outcome::Started),
                            });
                        }
                        Ok(false) => {}
                        Err(error) => notify(Event {
                            token,
                            result: Err(error),
                        }),
                    }
                }
            }
            continue;
        }
        if let (Some(token), Some(current)) = (active, &engine) {
            match current.done() {
                Ok(false) => {}
                result => {
                    active = None;
                    let cleanup = engine.as_mut().expect("active engine").stop();
                    if result.is_err() || cleanup.is_err() {
                        engine = None;
                    }
                    if shared.current(token) {
                        notify(Event {
                            token,
                            result: result.and(cleanup).map(|()| Outcome::Finished),
                        });
                    }
                }
            }
        }
    }
    if let Some(current) = &mut engine {
        let _ = current.stop();
    }
}

#[derive(Default)]
struct Online {
    client: AudioClient,
    playback: Option<Playback>,
}
impl Engine for Online {
    fn start(
        &mut self,
        text: &str,
        settings: &SpeechSettings,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<bool, String> {
        if AUDIO_FAULT.load(Ordering::Acquire) {
            return Err(audio_fault());
        }
        let audio = self.client.synthesize(settings, text, cancelled)?;
        if cancelled() {
            return Ok(false);
        }
        self.playback = Some(Playback::start(audio)?);
        if cancelled() {
            self.stop()?;
            return Ok(false);
        }
        Ok(true)
    }
    fn stop(&mut self) -> Result<(), String> {
        if let Some(mut playback) = self.playback.take() {
            playback.close()
        } else {
            Ok(())
        }
    }
    fn done(&self) -> Result<bool, String> {
        self.playback.as_ref().map_or(Ok(true), Playback::done)
    }
}

struct Connection {
    handle: HttpHandle,
    host: String,
    port: u16,
    secure: bool,
}
#[derive(Default)]
struct AudioClient {
    connection: Option<Connection>,
    local_session: Option<HttpHandle>,
    remote_session: Option<HttpHandle>,
    slots: Arc<Slots>,
}
impl AudioClient {
    fn connection(&mut self, endpoint: &Endpoint) -> Result<*mut c_void, String> {
        let local = endpoint.host.eq_ignore_ascii_case("localhost")
            || endpoint
                .host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback());
        let slot = if local {
            &mut self.local_session
        } else {
            &mut self.remote_session
        };
        if slot.is_none() {
            *slot = Some(
                HttpHandle::from_raw(unsafe {
                    WinHttpOpen(
                        wide(concat!("LingoLeaf/", env!("CARGO_PKG_VERSION"))).as_ptr(),
                        if local {
                            WINHTTP_ACCESS_TYPE_NO_PROXY
                        } else {
                            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY
                        },
                        null(),
                        null(),
                        WINHTTP_FLAG_ASYNC,
                    )
                })
                .map_err(speech_error)?,
            );
        }
        let session = slot.as_ref().expect("session initialized");
        if !self.connection.as_ref().is_some_and(|connection| {
            connection.host.eq_ignore_ascii_case(&endpoint.host)
                && connection.port == endpoint.port
                && connection.secure == endpoint.secure
        }) {
            self.connection = None;
            let handle = HttpHandle::from_raw(unsafe {
                WinHttpConnect(session.0, wide(&endpoint.host).as_ptr(), endpoint.port, 0)
            })
            .map_err(speech_error)?;
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
            .expect("connection initialized")
            .handle
            .0)
    }

    fn synthesize(
        &mut self,
        settings: &SpeechSettings,
        text: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<WaveAudio, String> {
        let body = build_request(settings, text)?;
        let raw = self.post(settings, body, cancelled)?;
        parse_wave(raw)
    }

    fn post(
        &mut self,
        settings: &SpeechSettings,
        body: Vec<u8>,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<u8>, String> {
        self.download(settings, body, cancelled, |_| {})
    }

    fn download(
        &mut self,
        settings: &SpeechSettings,
        body: Vec<u8>,
        cancelled: &dyn Fn() -> bool,
        mut received: impl FnMut(usize),
    ) -> Result<Vec<u8>, String> {
        cancel(cancelled)?;
        settings.validate()?;
        let endpoint = parse_speech_endpoint(&settings.base_url)?;
        let deadline = Instant::now() + Duration::from_secs(u64::from(settings.timeout_seconds));
        let connection = self.connection(&endpoint)?;
        cancel(cancelled)?;
        let handle = HttpHandle::from_raw(unsafe {
            WinHttpOpenRequest(
                connection,
                wide("POST").as_ptr(),
                wide(&endpoint.path).as_ptr(),
                null(),
                null(),
                null(),
                if endpoint.secure {
                    WINHTTP_FLAG_SECURE
                } else {
                    0
                },
            )
        })
        .map_err(speech_error)?;
        let redirect_policy = WINHTTP_OPTION_REDIRECT_POLICY_NEVER;
        if unsafe {
            WinHttpSetOption(
                handle.0,
                WINHTTP_OPTION_REDIRECT_POLICY,
                (&redirect_policy as *const u32).cast(),
                size_of::<u32>() as u32,
            )
        } == 0
        {
            return Err(network_error());
        }
        let mut headers = "Content-Type: application/json; charset=utf-8\r\nAccept: audio/wav, audio/x-wav, application/octet-stream\r\n".to_owned();
        if !settings.api_key.is_empty() {
            headers.push_str("Authorization: Bearer ");
            headers.push_str(&settings.api_key);
            headers.push_str("\r\n");
        }
        let request =
            Request::new(handle, wide(&headers), body, self.slots.clone()).map_err(speech_error)?;
        request.send(deadline, &cancelled).map_err(speech_error)?;
        request
            .receive(deadline, &cancelled)
            .map_err(speech_error)?;
        let mut status = 0u32;
        let mut length = size_of::<u32>() as u32;
        if unsafe {
            WinHttpQueryHeaders(
                request.raw(),
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                null(),
                (&mut status as *mut u32).cast(),
                &mut length,
                null_mut(),
            )
        } == 0
        {
            return Err(network_error());
        }
        cancel(cancelled)?;
        if !(200..300).contains(&status) {
            return Err(http_error(status));
        }
        if let Some(content_type) = query_header(request.raw(), WINHTTP_QUERY_CONTENT_TYPE)? {
            let mime = content_type
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            if !matches!(
                mime.as_str(),
                "audio/wav"
                    | "audio/x-wav"
                    | "audio/wave"
                    | "audio/vnd.wave"
                    | "application/octet-stream"
            ) {
                return Err("朗读接口未返回 WAV 音频，请检查接口设置。".into());
            }
        }
        let declared = query_header(request.raw(), WINHTTP_QUERY_CONTENT_LENGTH)?
            .map(|value| {
                value
                    .parse::<usize>()
                    .map_err(|_| "语音响应长度无效。".to_owned())
            })
            .transpose()?;
        if declared.is_some_and(|length| length == 0 || length > MAX_AUDIO_BYTES) {
            return Err("语音文件过大或为空，请选择较短内容。".into());
        }
        let mut bytes = Vec::with_capacity(declared.unwrap_or(8192).min(64 * 1024));
        loop {
            let available = request
                .available(deadline, &cancelled)
                .map_err(speech_error)?;
            if available == 0 {
                break;
            }
            let read = request
                .read(available, deadline, &cancelled, |chunk| {
                    if bytes.len().saturating_add(chunk.len()) > MAX_AUDIO_BYTES {
                        return Err("语音文件过大，请选择较短内容。".into());
                    }
                    bytes.extend_from_slice(chunk);
                    received(bytes.len());
                    Ok(())
                })
                .map_err(speech_error)?;
            if read == 0 {
                break;
            }
        }
        cancel(cancelled)?;
        if declared.is_some_and(|length| length != bytes.len()) {
            return Err("语音下载不完整，请重试。".into());
        }
        Ok(bytes)
    }
}

fn query_header(request: *mut c_void, name: u32) -> Result<Option<String>, String> {
    let mut size = 0u32;
    let result =
        unsafe { WinHttpQueryHeaders(request, name, null(), null_mut(), &mut size, null_mut()) };
    if result == 0 {
        let error = unsafe { GetLastError() };
        if error == ERROR_WINHTTP_HEADER_NOT_FOUND {
            return Ok(None);
        }
        if error != 122 {
            return Err(network_error_code(error));
        }
    }
    if size == 0 || size > 2048 || size & 1 != 0 {
        return Err("语音响应头无效。".into());
    }
    let mut buffer = vec![0u16; size as usize / 2 + 1];
    if unsafe {
        WinHttpQueryHeaders(
            request,
            name,
            null(),
            buffer.as_mut_ptr().cast(),
            &mut size,
            null_mut(),
        )
    } == 0
    {
        return Err(network_error());
    }
    let length = buffer
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(buffer.len());
    Ok(Some(String::from_utf16_lossy(&buffer[..length])))
}
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
fn cancel(cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    if cancelled() {
        Err("朗读已取消。".into())
    } else {
        Ok(())
    }
}
fn speech_error(error: String) -> String {
    error.replace("翻译", "朗读")
}
fn network_error() -> String {
    network_error_code(unsafe { GetLastError() })
}
fn network_error_code(code: u32) -> String {
    match code {
        ERROR_WINHTTP_TIMEOUT => "语音请求超时，请检查接口或增加超时。".into(),
        ERROR_WINHTTP_NAME_NOT_RESOLVED => "无法解析语音接口域名，请检查地址。".into(),
        ERROR_WINHTTP_SECURE_FAILURE => "语音接口证书验证失败，请检查系统时间与证书。".into(),
        ERROR_WINHTTP_CANNOT_CONNECT | ERROR_WINHTTP_CONNECTION_ERROR => {
            "无法连接语音接口，请检查网络与地址。".into()
        }
        _ => format!("语音网络请求失败（错误 {code}），请稍后重试。"),
    }
}
fn http_error(status: u32) -> String {
    match status {
        301..=399 => "语音接口发生重定向，请填写最终服务地址。".into(),
        401 | 403 => "语音接口鉴权失败，请检查独立语音密钥。".into(),
        404 => "语音接口地址不存在，请检查服务地址。".into(),
        429 => "语音接口繁忙或额度不足，请稍后重试。".into(),
        400 => "语音接口拒绝请求，请检查模型、音色与文本长度。".into(),
        _ => format!("语音服务返回 HTTP {status}，请稍后重试。"),
    }
}

/// A queued wave header and PCM allocation are owned until Reset/Unprepare/Close
/// proves the output driver no longer holds them. A broken driver is latched and
/// its bounded allocation retained, rather than freeing memory still in use.
struct Playback {
    handle: HWAVEOUT,
    completed: HANDLE,
    header: *mut WAVEHDR,
    audio: Option<WaveAudio>,
    started: Instant,
    duration: Duration,
}
impl Playback {
    fn start(audio: WaveAudio) -> Result<Self, String> {
        if AUDIO_FAULT.load(Ordering::Acquire) {
            return Err(audio_fault());
        }
        let format = WAVEFORMATEX {
            wFormatTag: 1,
            nChannels: audio.channels,
            nSamplesPerSec: audio.sample_rate,
            nAvgBytesPerSec: audio.byte_rate,
            nBlockAlign: audio.block_align,
            wBitsPerSample: audio.bits_per_sample,
            cbSize: 0,
        };
        // CALLBACK_EVENT signals completion without racing the driver's writes
        // to WAVEHDR. This manual-reset event is private to this single buffer.
        let completed = unsafe { CreateEventW(null(), 1, 0, null()) };
        if completed.is_null() {
            return Err(audio_error());
        }
        let mut handle = null_mut();
        if unsafe {
            waveOutOpen(
                &mut handle,
                WAVE_MAPPER,
                &format,
                completed as usize,
                0,
                CALLBACK_EVENT,
            )
        } != 0
        {
            unsafe {
                CloseHandle(completed);
            }
            return Err(audio_error());
        }
        let header = Box::into_raw(Box::new(WAVEHDR {
            lpData: audio.samples().as_ptr().cast_mut(),
            dwBufferLength: audio.samples().len() as u32,
            ..unsafe { zeroed() }
        }));
        let duration = audio.duration();
        let mut playback = Self {
            handle,
            completed,
            header,
            audio: Some(audio),
            started: Instant::now(),
            duration,
        };
        if unsafe { waveOutPrepareHeader(handle, header, size_of::<WAVEHDR>() as u32) } != 0 {
            let _ = playback.close();
            return Err(audio_error());
        }
        // Discard any initial device notification before submitting the only
        // buffer; completion after waveOutWrite must remain signalled.
        if unsafe { ResetEvent(completed) } == 0 {
            let _ = playback.close();
            return Err(audio_error());
        }
        if unsafe { waveOutWrite(handle, header, size_of::<WAVEHDR>() as u32) } != 0 {
            let _ = playback.close();
            return Err(audio_error());
        }
        Ok(playback)
    }
    fn done(&self) -> Result<bool, String> {
        if self.header.is_null() {
            return Ok(true);
        }
        match unsafe { WaitForSingleObject(self.completed, 0) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT if self.started.elapsed() <= self.duration + Duration::from_secs(5) => {
                Ok(false)
            }
            _ => Err(audio_error()),
        }
    }
    fn close(&mut self) -> Result<(), String> {
        if self.handle.is_null() {
            return Ok(());
        }
        let reset = unsafe { waveOutReset(self.handle) };
        let unprepare = unsafe {
            waveOutUnprepareHeader(self.handle, self.header, size_of::<WAVEHDR>() as u32)
        };
        let closed = unsafe { waveOutClose(self.handle) };
        self.handle = null_mut();
        if closed == 0 && (unprepare == 0 || reset == 0) {
            // Close succeeded, so no driver callback can signal this event.
            unsafe {
                CloseHandle(self.completed);
            }
            self.completed = null_mut();
            if !self.header.is_null() {
                unsafe {
                    drop(Box::from_raw(self.header));
                }
                self.header = null_mut();
            }
            self.audio = None;
            Ok(())
        } else {
            AUDIO_FAULT.store(true, Ordering::Release);
            if let Some(audio) = self.audio.take() {
                std::mem::forget(audio);
            }
            self.header = null_mut();
            // Retain the completion event with the bounded driver allocation;
            // closing it while a broken driver may signal it is unsafe.
            self.completed = null_mut();
            Err(audio_fault())
        }
    }
}
impl Drop for Playback {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
fn audio_error() -> String {
    "无法播放语音，请检查声音输出设备后重试。".into()
}

fn audio_fault() -> String {
    "音频设备异常，请重新启动轻译后重试。".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::mpsc,
    };

    fn settings(port: u16) -> SpeechSettings {
        SpeechSettings {
            base_url: format!("http://127.0.0.1:{port}/v1"),
            model: "kokoro-test".into(),
            voice: "zf_xiaobei".into(),
            api_key: "speech-only-test-key".into(),
            timeout_seconds: 5,
        }
    }
    fn wave() -> Vec<u8> {
        let samples = [0, 0, 0, 1, 0, 2, 0, 3];
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&(36 + samples.len() as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&24_000u32.to_le_bytes());
        bytes.extend_from_slice(&48_000u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(samples.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&samples);
        bytes
    }
    fn accept(listener: &TcpListener) -> TcpStream {
        listener.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match listener.accept() {
                Ok((socket, _)) => {
                    socket.set_nonblocking(false).unwrap();
                    return socket;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "speech client did not connect");
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("speech test accept failed: {error}"),
            }
        }
    }
    fn request(socket: &mut TcpStream) -> serde_json::Value {
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 4096];
        let start = loop {
            let count = socket.read(&mut chunk).unwrap();
            assert_ne!(count, 0);
            bytes.extend_from_slice(&chunk[..count]);
            assert!(bytes.len() < 192 * 1024);
            if let Some(index) = bytes.windows(4).position(|s| s == b"\r\n\r\n") {
                break index + 4;
            }
        };
        let headers = String::from_utf8(bytes[..start].to_vec()).unwrap();
        assert!(headers.starts_with("POST /v1/audio/speech HTTP/1.1\r\n"));
        assert!(headers
            .lines()
            .any(|s| s.eq_ignore_ascii_case("Authorization: Bearer speech-only-test-key")));
        assert!(!headers.contains("chat-test-key"));
        let length = headers
            .lines()
            .find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap();
        while bytes.len() - start < length {
            let count = socket.read(&mut chunk).unwrap();
            assert_ne!(count, 0);
            bytes.extend_from_slice(&chunk[..count]);
        }
        let value: serde_json::Value =
            serde_json::from_slice(&bytes[start..start + length]).unwrap();
        assert_eq!(value["model"], "kokoro-test");
        assert_eq!(value["voice"], "zf_xiaobei");
        assert_eq!(value["response_format"], "wav");
        value
    }
    fn respond(socket: &mut TcpStream, audio: &[u8]) {
        write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",audio.len()).unwrap();
        socket.write_all(audio).unwrap();
        socket.flush().unwrap();
    }

    #[test]
    fn online_speech_posts_separate_credentials_and_decodes_real_pcm_response() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let settings = settings(listener.local_addr().unwrap().port());
        let service = thread::spawn(move || {
            let mut socket = accept(&listener);
            assert_eq!(request(&mut socket)["input"], "你好 👩‍💻\nHello");
            respond(&mut socket, &wave());
        });
        let audio = AudioClient::default()
            .synthesize(&settings, "你好 👩‍💻\nHello", &|| false)
            .unwrap();
        assert_eq!(audio.samples(), &wave()[44..]);
        assert_eq!(audio.sample_rate, 24_000);
        service.join().unwrap();
    }

    #[test]
    fn speech_redirect_wrong_mime_and_oversized_audio_fail_without_echoing_secrets() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let destination = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let target_port = destination.local_addr().unwrap().port();
        destination.set_nonblocking(true).unwrap();
        let settings = settings(listener.local_addr().unwrap().port());
        let service = thread::spawn(move || {
            for index in 0..3 {
                let mut socket = accept(&listener);
                request(&mut socket);
                match index {
                    0 => write!(socket,"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{target_port}/leak\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap(),
                    1 => { let body=b"secret-echo-must-not-be-rendered"; write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).unwrap(); let _=socket.write_all(body); }
                    _ => write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", MAX_AUDIO_BYTES + 1).unwrap(),
                }
                let _ = socket.flush();
            }
        });
        let mut client = AudioClient::default();
        for _ in 0..3 {
            let error = client
                .synthesize(&settings, "Hello", &|| false)
                .unwrap_err();
            assert!(!error.contains("secret-echo"));
            assert!(!error.contains("speech-only-test-key"));
        }
        service.join().unwrap();
        assert!(
            destination.accept().is_err(),
            "redirect was followed with speech credentials"
        );
    }

    #[test]
    fn speech_cancels_waiting_headers_without_waiting_for_the_old_service() {
        cancelled_download(false);
    }
    #[test]
    fn speech_cancels_stalled_binary_body_then_reuses_client_before_old_body_release() {
        cancelled_download(true);
    }

    fn cancelled_download(stalled_body: bool) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let settings = settings(listener.local_addr().unwrap().port());
        let (old_ready_tx, old_ready) = mpsc::channel();
        let (old_release_tx, old_release) = mpsc::channel();
        let service = thread::spawn(move || {
            let mut old = accept(&listener);
            request(&mut old);
            if stalled_body {
                write!(old,"HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",wave().len()).unwrap();
                old.write_all(&wave()[..46]).unwrap();
                old.flush().unwrap();
            }
            old_ready_tx.send(()).unwrap();
            let old_service = thread::spawn(move || {
                old_release.recv_timeout(Duration::from_secs(8)).unwrap();
                drop(old);
            });
            let mut next = accept(&listener);
            request(&mut next);
            respond(&mut next, &wave());
            old_service.join().unwrap();
        });
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let (body_read_tx, body_read) = mpsc::channel();
        let (first_tx, first) = mpsc::channel();
        let (next_tx, next) = mpsc::channel();
        let (continue_tx, continue_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let mut client = AudioClient::default();
            let body = build_request(&settings, "old").unwrap();
            let result = client.download(
                &settings,
                body,
                &|| worker_cancelled.load(Ordering::Acquire),
                |count| {
                    if count >= 46 {
                        let _ = body_read_tx.send(());
                    }
                },
            );
            first_tx.send(result).unwrap();
            continue_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            next_tx
                .send(client.synthesize(&settings, "new", &|| false))
                .unwrap();
        });
        old_ready.recv_timeout(Duration::from_secs(3)).unwrap();
        if stalled_body {
            body_read.recv_timeout(Duration::from_secs(3)).unwrap();
        }
        let cancel_started = Instant::now();
        cancelled.store(true, Ordering::Release);
        assert!(first.recv_timeout(Duration::from_secs(2)).unwrap().is_err());
        assert!(cancel_started.elapsed() < Duration::from_secs(2));
        cancelled.store(false, Ordering::Release);
        continue_tx.send(()).unwrap();
        let fresh = next.recv_timeout(Duration::from_secs(3)).unwrap().unwrap();
        assert_eq!(fresh.samples(), &wave()[44..]);
        // The first service still owns its blocked body/socket at this point.
        old_release_tx.send(()).unwrap();
        worker.join().unwrap();
        service.join().unwrap();
    }

    struct FakeEngine {
        running: Arc<AtomicBool>,
        starts: mpsc::Sender<String>,
    }
    impl Engine for FakeEngine {
        fn start(
            &mut self,
            text: &str,
            _settings: &SpeechSettings,
            cancelled: &dyn Fn() -> bool,
        ) -> Result<bool, String> {
            if cancelled() {
                return Ok(false);
            }
            self.running.store(true, Ordering::Release);
            self.starts.send(text.to_owned()).unwrap();
            Ok(true)
        }
        fn stop(&mut self) -> Result<(), String> {
            self.running.store(false, Ordering::Release);
            Ok(())
        }
        fn done(&self) -> Result<bool, String> {
            Ok(!self.running.load(Ordering::Acquire))
        }
    }
    #[test]
    fn queued_speech_is_latest_wins_and_stop_never_waits_for_playback_completion() {
        let shared = Arc::new(Shared::default());
        let speech = Speech {
            shared: Arc::clone(&shared),
        };
        let discarded = speech.start("old".into(), SpeechSettings::default());
        let latest = speech.start("new".into(), SpeechSettings::default());
        assert_ne!(discarded, latest);
        let (events_tx, events) = mpsc::channel();
        let (starts_tx, starts) = mpsc::channel();
        let running = Arc::new(AtomicBool::new(false));
        let worker_running = Arc::clone(&running);
        let worker_shared = Arc::clone(&shared);
        let worker = thread::spawn(move || {
            run(
                worker_shared,
                |event| {
                    events_tx.send(event).unwrap();
                },
                || {
                    Ok(FakeEngine {
                        running: Arc::clone(&worker_running),
                        starts: starts_tx.clone(),
                    })
                },
            )
        });
        assert_eq!(starts.recv_timeout(Duration::from_secs(2)).unwrap(), "new");
        let event = events.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(event.token, latest);
        assert_eq!(event.result, Ok(Outcome::Started));
        let replacement = speech.start("replace active".into(), SpeechSettings::default());
        assert_eq!(
            starts.recv_timeout(Duration::from_secs(2)).unwrap(),
            "replace active"
        );
        assert_eq!(
            events.recv_timeout(Duration::from_secs(2)).unwrap().token,
            replacement
        );
        let stop = speech.stop();
        let event = events.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(event.token, stop);
        assert_eq!(event.result, Ok(Outcome::Stopped));
        assert!(!running.load(Ordering::Acquire));
        assert!(events.try_recv().is_err());
        drop(speech);
        worker.join().unwrap();
    }
    #[test]
    fn unavailable_output_reports_an_event_and_still_allows_stop_and_shutdown() {
        let shared = Arc::new(Shared::default());
        let speech = Speech {
            shared: Arc::clone(&shared),
        };
        let (events_tx, events) = mpsc::channel();
        let worker = thread::spawn(move || {
            run::<FakeEngine>(
                shared,
                |event| {
                    events_tx.send(event).unwrap();
                },
                || Err("output unavailable".into()),
            )
        });
        let token = speech.start("test".into(), SpeechSettings::default());
        let event = events.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(event.token, token);
        assert_eq!(event.result, Err("output unavailable".into()));
        let stop = speech.stop();
        assert_eq!(
            events.recv_timeout(Duration::from_secs(2)).unwrap().token,
            stop
        );
        drop(speech);
        worker.join().unwrap();
    }
}
