// Real loopback HTTP speech requests. No external service or real credential.
use super::*;

const SPEAK: i32 = 285;
const KEY: &str = "speech-only-local-fixture";

pub(super) struct SpeechMock {
    pub(super) port: u16,
    requests: Arc<Mutex<Vec<serde_json::Value>>>,
    error: Arc<Mutex<Option<String>>>,
    release_first: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl SpeechMock {
    pub(super) fn start() -> Result<Self, String> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let requests = Arc::new(Mutex::new(Vec::new()));
        let error = Arc::new(Mutex::new(None));
        let release_first = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let state = (
            requests.clone(),
            error.clone(),
            release_first.clone(),
            stop.clone(),
        );
        let worker = thread::spawn(move || {
            let mut handlers = Vec::new();
            while !state.3.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let state = (
                            state.0.clone(),
                            state.1.clone(),
                            state.2.clone(),
                            state.3.clone(),
                        );
                        handlers.push(thread::spawn(move || {
                            if let Err(problem) = serve_speech(stream, &state.0, &state.2, &state.3)
                            {
                                *state.1.lock().unwrap_or_else(|p| p.into_inner()) = Some(problem);
                            }
                        }));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10))
                    }
                    Err(_) => break,
                }
            }
            for handler in handlers {
                let _ = handler.join();
            }
        });
        Ok(Self {
            port,
            requests,
            error,
            release_first,
            stop,
            worker: Some(worker),
        })
    }

    fn count(&self) -> Result<usize, String> {
        if let Some(error) = self
            .error
            .lock()
            .map_err(|_| "Speech error lock failed.")?
            .as_ref()
        {
            return Err(error.clone());
        }
        Ok(self
            .requests
            .lock()
            .map_err(|_| "Speech request lock failed.")?
            .len())
    }
}

impl Drop for SpeechMock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.release_first.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn pcm_wav() -> Vec<u8> {
    let samples = 480_u32;
    let size = samples * 2;
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + size).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1_u16.to_le_bytes()); // mono
    wav.extend_from_slice(&24_000_u32.to_le_bytes());
    wav.extend_from_slice(&48_000_u32.to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&size.to_le_bytes());
    wav.resize(44 + size as usize, 0);
    wav
}

fn serve_speech(
    mut stream: TcpStream,
    requests: &Mutex<Vec<serde_json::Value>>,
    gate: &AtomicBool,
    stop: &AtomicBool,
) -> Result<(), String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 1024];
    let (body_start, length) = loop {
        let received = stream.read(&mut chunk).map_err(|e| e.to_string())?;
        if received == 0 || bytes.len() + received > 65_536 {
            return Err("Invalid bounded speech HTTP request.".into());
        }
        bytes.extend_from_slice(&chunk[..received]);
        if let Some(end) = bytes.windows(4).position(|p| p == b"\r\n\r\n") {
            let header = std::str::from_utf8(&bytes[..end]).map_err(|e| e.to_string())?;
            if !header.starts_with("POST /v1/audio/speech HTTP/") {
                return Err("Speech used an unexpected method or endpoint.".into());
            }
            let authorization: Vec<_> = header
                .lines()
                .filter_map(|line| line.split_once(':'))
                .filter(|(name, _)| name.eq_ignore_ascii_case("authorization"))
                .map(|(_, value)| value.trim())
                .collect();
            if authorization != [format!("Bearer {KEY}")] {
                return Err(
                    "Speech did not send its independent Bearer credential exactly once.".into(),
                );
            }
            let length = header
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .ok_or("Speech Content-Length is missing.")?
                .1
                .trim()
                .parse::<usize>()
                .map_err(|e| e.to_string())?;
            if length > 60_000 {
                return Err("Oversized speech fixture request.".into());
            }
            break (end + 4, length);
        }
    };
    while bytes.len() < body_start + length {
        let received = stream.read(&mut chunk).map_err(|e| e.to_string())?;
        if received == 0 || bytes.len() + received > 65_536 {
            return Err("Incomplete speech HTTP body.".into());
        }
        bytes.extend_from_slice(&chunk[..received]);
    }
    let body: serde_json::Value = serde_json::from_slice(&bytes[body_start..body_start + length])
        .map_err(|e| e.to_string())?;
    let object = body
        .as_object()
        .ok_or("Speech body is not a JSON object.")?;
    if object.len() != 4
        || body["model"] != "local-pcm-model"
        || body["voice"] != "local-pcm-voice"
        || body["response_format"] != "wav"
        || body["input"]
            .as_str()
            .is_none_or(|input| input.trim().is_empty())
    {
        return Err(
            "Speech did not send the exact model/input/voice/response_format schema.".into(),
        );
    }
    let index = {
        let mut requests = requests.lock().map_err(|_| "Speech request lock failed.")?;
        let index = requests.len();
        requests.push(body);
        index
    };
    let wav = if index == 2 {
        b"not a WAV file".to_vec()
    } else {
        pcm_wav()
    };
    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", wav.len()).map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;
    if index == 0 {
        while !gate.load(Ordering::Acquire) && !stop.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(10));
        }
        // A cancelled client may already have closed this deliberately blocked body.
        let _ = stream.write_all(&wav);
    } else {
        stream.write_all(&wav).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub(super) fn check(
    app: &Application,
    inline: HWND,
    translation: &Mock,
    speech: &SpeechMock,
) -> Result<(), String> {
    let before = translation.request_count()?;
    let draft = text(inline, QUESTION)?;
    let profile = PathBuf::from(
        std::env::var_os("LOCALAPPDATA").ok_or("The private speech profile is missing.")?,
    )
    .join("LingoLeaf/settings.json");
    let baseline = fs::read(&profile).map_err(|e| e.to_string())?;
    press_button(inline, SPEAK)?;
    let modal = app.window("轻译 · 朗读接口")?;
    let settings = app.window(SETTINGS)?;
    wait_for(
        "unconfigured speech opening its settings without a request",
        || Ok(unsafe { IsWindowVisible(modal) } != 0),
    )?;
    if speech.count()? != 0
        || translation.request_count()? != before
        || unsafe { GetWindow(modal, GW_OWNER) } != settings
        || unsafe { IsWindowEnabled(settings) } != 0
    {
        return Err(
            "Unconfigured speech sent text or did not open an owned modal above settings.".into(),
        );
    }
    let owner_bounds = bounds(settings)?;
    let modal_bounds = bounds(modal)?;
    if ((owner_bounds.0 + owner_bounds.2) - (modal_bounds.0 + modal_bounds.2)).abs() > 8
        || ((owner_bounds.1 + owner_bounds.3) - (modal_bounds.1 + modal_bounds.3)).abs() > 8
    {
        return Err("The speech modal is not centered over its settings owner.".into());
    }
    let value = wide("http://127.0.0.1:1/v1");
    send(
        unsafe { GetDlgItem(modal, 304) },
        WM_SETTEXT,
        0,
        value.as_ptr() as isize,
    )?;
    press_button(modal, 309)?;
    wait_for(
        "cancelling speech settings restoring the translation",
        || Ok(unsafe { IsWindowVisible(modal) } == 0 && unsafe { IsWindowVisible(inline) } != 0),
    )?;
    if fs::read(&profile).map_err(|e| e.to_string())? != baseline
        || speech.count()? != 0
        || text(inline, QUESTION)? != draft
    {
        return Err(
            "Cancelling speech settings saved a draft, sent text, or lost the question.".into(),
        );
    }
    press_button(inline, SPEAK)?;
    wait_for("reopening the unconfigured speech modal", || {
        Ok(unsafe { IsWindowVisible(modal) } != 0)
    })?;
    if !text(modal, 304)?.is_empty() {
        return Err("Cancelling speech settings retained its unsaved URL.".into());
    }
    for (id, value) in [
        (304, format!("http://127.0.0.1:{}/v1", speech.port)),
        (305, "local-pcm-model".into()),
        (306, "local-pcm-voice".into()),
        (307, KEY.into()),
    ] {
        let value = wide(&value);
        send(
            unsafe { GetDlgItem(modal, id) },
            WM_SETTEXT,
            0,
            value.as_ptr() as isize,
        )?;
    }
    if send(unsafe { GetDlgItem(modal, 307) }, 0x00D2, 0, 0)? == 0 {
        return Err("The independent speech API key editor is not password-masked.".into());
    }
    press_button(modal, 308)?;
    wait_for(
        "saving speech settings applying immediately and restoring the translation",
        || Ok(unsafe { IsWindowVisible(modal) } == 0 && unsafe { IsWindowVisible(inline) } != 0),
    )?;
    if speech.count()? != 0
        || translation.request_count()? != before
        || text(inline, QUESTION)? != draft
    {
        return Err("Saving speech settings sent text or discarded the question draft.".into());
    }
    let saved = fs::read(&profile).map_err(|e| e.to_string())?;
    if saved.windows(KEY.len()).any(|part| part == KEY.as_bytes()) {
        return Err("The independent speech key was saved in plaintext.".into());
    }
    let restored = config::load()?;
    if restored.speech_api_key != KEY
        || !restored.api_key.is_empty()
        || restored.speech_model != "local-pcm-model"
    {
        return Err("Speech settings did not persist independently of the translation key.".into());
    }
    let ready_label = text(inline, SPEAK)?;
    press_button(inline, SPEAK)?;
    wait_for(
        "the first real speech POST with its body still blocked",
        || Ok(speech.count()? == 1 && text(inline, SPEAK)? != ready_label),
    )?;
    press_button(inline, SPEAK)?;
    wait_for("cancelling a blocked speech download", || {
        Ok(text(inline, SPEAK)? == ready_label)
    })?;
    let started = Instant::now();
    press_button(inline, SPEAK)?;
    wait_for(
        "a new speech request finishing before the cancelled body's gate opens",
        || {
            Ok(speech.count()? == 2
                && text(inline, SPEAK)? == ready_label
                && (text(inline, 254)?.contains("朗读") || text(inline, 254)?.contains("语音")))
        },
    )?;
    if started.elapsed() >= Duration::from_secs(3) || speech.release_first.load(Ordering::Acquire) {
        return Err("New speech waited for the cancelled body.".into());
    }
    let actual_status = text(inline, 254)?;
    if !actual_status.contains("完成")
        && !actual_status.contains("无法播放")
        && !actual_status.contains("设备")
    {
        return Err(format!("Valid PCM WAV did not finish or report an explicit output-device failure: {actual_status:?}"));
    }
    speech.release_first.store(true, Ordering::Release);
    press_button(inline, SPEAK)?;
    wait_for(
        "rejecting malformed audio without attempting local fallback",
        || {
            Ok(speech.count()? == 3
                && text(inline, SPEAK)? == ready_label
                && (text(inline, 254)?.contains("WAV") || text(inline, 254)?.contains("格式")))
        },
    )?;
    if translation.request_count()? != before || text(inline, QUESTION)? != draft {
        return Err("Speech sent a chat POST or modified the unsent question draft.".into());
    }
    let requests = speech
        .requests
        .lock()
        .map_err(|_| "Speech request lock failed.")?;
    if requests.len() != 3
        || requests
            .iter()
            .any(|request| request["input"] != requests[0]["input"])
    {
        return Err("Speech retries changed the selected translation text.".into());
    }
    println!("Online speech passed: exact independent Bearer/schema, PCM WAV transport, cancellation before blocked body release, malformed-WAV rejection, and no chat POST or draft change. Runtime audio status: {actual_status}");
    Ok(())
}
