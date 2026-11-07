//! Local Windows SAPI speech. COM and the audio engine stay on one worker;
//! callers only replace a bounded pending command and never wait for playback.

use crate::config::Language;
use std::{
    ptr::null_mut,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Condvar, Mutex,
    },
    thread,
    time::Duration,
};
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT},
        Media::Speech::{
            ISpObjectToken, ISpObjectTokenCategory, ISpVoice, SpObjectTokenCategory, SpVoice,
            SPCAT_VOICES, SPF_ASYNC, SPF_IS_NOT_XML, SPF_PURGEBEFORESPEAK, SPRS_DONE,
            SPVOICESTATUS,
        },
        System::{
            Com::{
                CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize,
                CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
            },
            Threading::WaitForSingleObject,
        },
    },
};

const POLL: Duration = Duration::from_millis(20);
const MAX_TEXT_UTF16: usize = 256 * 1024;

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
        language: Language,
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
        // Token allocation and command replacement share the lock, so even
        // concurrent stop/start callers cannot replace a newer command with an
        // older token. The atomic also lets COM work observe cancellation.
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
    /// Creating the worker does not require a voice or audio device. Their
    /// absence is reported only when the user requests playback.
    pub(crate) fn new(notify: impl Fn(Event) + Send + 'static) -> Result<Self, String> {
        let shared = Arc::new(Shared::default());
        let worker = Arc::clone(&shared);
        thread::Builder::new()
            .name("speech".into())
            .spawn(move || {
                let apartment = Apartment::new();
                run(worker, notify, || {
                    apartment.as_ref().map_err(Clone::clone)?;
                    Sapi::new()
                });
            })
            .map_err(|_| "无法启动朗读线程，请重新启动应用后再试。".to_string())?;
        Ok(Self { shared })
    }

    /// `text` is visible plain text, after Markdown presentation is removed.
    /// The returned token belongs to all notifications for this request.
    pub(crate) fn start(&self, text: String, language: Language) -> u64 {
        self.shared.send(|token| Command::Start {
            token,
            text,
            language,
        })
    }

    /// Invalidates in-flight notifications immediately. Audio is purged on the
    /// worker without waiting for the current sentence to finish.
    pub(crate) fn stop(&self) -> u64 {
        self.shared.send(|token| Command::Stop { token })
    }
}

impl Drop for Speech {
    fn drop(&mut self) {
        // Do not join a COM/audio engine from the UI thread. The worker purges
        // its own voice, releases its interfaces and balances CoInitializeEx.
        self.shared.close();
    }
}

trait Engine {
    fn start(
        &mut self,
        text: &str,
        language: Language,
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
                    // Release a voice that cannot be purged rather than leave
                    // it behind while creating a second overlapping voice.
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
                Command::Start { text, language, .. } => {
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
                    let result = engine.as_mut().expect("voice was created above").start(
                        &text,
                        language,
                        &|| !shared.current(token),
                    );
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
                    if result.is_err() {
                        if let Some(current) = &mut engine {
                            let _ = current.stop();
                        }
                        engine = None;
                    }
                    if shared.current(token) {
                        notify(Event {
                            token,
                            result: result.map(|_| Outcome::Finished),
                        });
                    }
                }
            }
        }
    }
    if let Some(current) = &mut engine {
        let _ = current.stop();
    }
    // Engine interfaces leave scope before the worker's Apartment guard.
}

struct Apartment;

impl Apartment {
    fn new() -> Result<Self, String> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(|_| "无法初始化 Windows 本机朗读，请稍后再试。".to_string())?;
        Ok(Self)
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

struct Sapi {
    voice: ISpVoice,
    selected: Option<Language>,
    stream: u32,
}

impl Sapi {
    fn new() -> Result<Self, String> {
        let voice = unsafe { CoCreateInstance(&SpVoice, None, CLSCTX_INPROC_SERVER) }
            .map_err(|_| "当前系统无法启动本机朗读，请确认已安装 Windows 语音组件。".to_string())?;
        Ok(Self {
            voice,
            selected: None,
            stream: 0,
        })
    }

    fn select(&mut self, language: Language, cancelled: &dyn Fn() -> bool) -> Result<bool, String> {
        if self.selected == Some(language) {
            return Ok(true);
        }
        if language == Language::Auto {
            return Err("请先设置明确的目标语言，再朗读译文。".into());
        }
        let category: ISpObjectTokenCategory =
            unsafe { CoCreateInstance(&SpObjectTokenCategory, None, CLSCTX_INPROC_SERVER) }
                .map_err(|_| "无法读取 Windows 本机语音库。".to_string())?;
        unsafe { category.SetId(SPCAT_VOICES, false) }
            .map_err(|_| "无法读取 Windows 本机语音库。".to_string())?;
        let tokens = unsafe { category.EnumTokens(PCWSTR::null(), PCWSTR::null()) }
            .map_err(|_| "无法读取 Windows 本机语音库。".to_string())?;
        let mut count = 0;
        unsafe { tokens.GetCount(&mut count) }
            .map_err(|_| "无法读取 Windows 本机语音库。".to_string())?;
        let mut best: Option<(u8, ISpObjectToken)> = None;
        // A bounded scan also handles malformed third-party token catalogues.
        for index in 0..count.min(256) {
            if cancelled() {
                return Ok(false);
            }
            let Ok(token) = (unsafe { tokens.Item(index) }) else {
                continue;
            };
            let Some(languages) = token_languages(&token) else {
                continue;
            };
            let score = language_score(language, &languages);
            if score > best.as_ref().map_or(0, |(score, _)| *score) {
                best = Some((score, token));
            }
        }
        let Some((_, token)) = best else {
            return Err(format!(
                "缺少{}语音，请先在 Windows 语音设置中安装。",
                language.label()
            ));
        };
        if cancelled() {
            return Ok(false);
        }
        unsafe { self.voice.SetVoice(&token) }
            .map_err(|_| "无法启用所选本机语音，请检查 Windows 语音设置。".to_string())?;
        self.selected = Some(language);
        Ok(true)
    }
}

impl Engine for Sapi {
    fn start(
        &mut self,
        text: &str,
        language: Language,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<bool, String> {
        if text.trim().is_empty() {
            return Err("暂无可朗读的译文。".into());
        }
        if text.encode_utf16().take(MAX_TEXT_UTF16 + 1).count() > MAX_TEXT_UTF16 {
            return Err("译文过长，请选择较短内容朗读。".into());
        }
        if !self.select(language, cancelled)? || cancelled() {
            return Ok(false);
        }
        // Embedded NULs must not silently truncate what the user hears. XML is
        // explicitly disabled, so model text cannot inject SAPI commands.
        let text: Vec<u16> = text
            .chars()
            .map(|value| if value == '\0' { ' ' } else { value })
            .flat_map(|value| {
                let mut units = [0u16; 2];
                let count = value.encode_utf16(&mut units).len();
                units.into_iter().take(count)
            })
            .chain(Some(0))
            .collect();
        if cancelled() {
            return Ok(false);
        }
        unsafe {
            self.voice.Speak(
                PCWSTR(text.as_ptr()),
                (SPF_ASYNC.0 | SPF_PURGEBEFORESPEAK.0 | SPF_IS_NOT_XML.0) as u32,
                Some(&mut self.stream),
            )
        }
        .map_err(|_| "无法开始朗读，请检查系统语音和声音输出设备。".to_string())?;
        Ok(true)
    }

    fn stop(&mut self) -> Result<(), String> {
        unsafe {
            self.voice.Speak(
                PCWSTR::null(),
                (SPF_ASYNC.0 | SPF_PURGEBEFORESPEAK.0 | SPF_IS_NOT_XML.0) as u32,
                None,
            )
        }
        .map_err(|_| "无法停止本机朗读，请检查系统语音组件。".to_string())
    }

    fn done(&self) -> Result<bool, String> {
        let mut status = SPVOICESTATUS::default();
        unsafe { self.voice.GetStatus(&mut status, null_mut()) }
            .map_err(|_| "无法获取朗读状态，请检查声音输出设备。".to_string())?;
        if status.ulCurrentStream == self.stream && status.hrLastResult.is_err() {
            return Err("朗读未能完成，请检查系统语音和声音输出设备。".into());
        }
        let event = unsafe { self.voice.SpeakCompleteEvent() };
        if !event.is_invalid() {
            return match unsafe { WaitForSingleObject(event, 0) } {
                WAIT_OBJECT_0 => Ok(true),
                WAIT_TIMEOUT => Ok(false),
                _ => Err("无法获取朗读完成状态，请稍后再试。".into()),
            };
        }
        Ok(status.ulLastStreamQueued == self.stream && status.dwRunningState == SPRS_DONE.0 as u32)
    }
}

fn token_languages(token: &ISpObjectToken) -> Option<String> {
    let attributes = unsafe { token.OpenKey(w!("Attributes")) }.ok()?;
    let raw = unsafe { attributes.GetStringValue(w!("Language")) }.ok()?;
    if raw.is_null() {
        return None;
    }
    let value = unsafe { raw.to_string() }.ok();
    unsafe { CoTaskMemFree(Some(raw.0.cast())) };
    value
}

fn language_score(language: Language, attributes: &str) -> u8 {
    attributes
        .split(';')
        .filter_map(|value| u16::from_str_radix(value.trim(), 16).ok())
        .map(|lcid| match language {
            Language::SimplifiedChinese if matches!(lcid, 0x0804 | 0x1004) => 3,
            Language::TraditionalChinese if lcid == 0x0404 => 3,
            Language::TraditionalChinese if matches!(lcid, 0x0c04 | 0x1404) => 2,
            Language::SimplifiedChinese | Language::TraditionalChinese if lcid & 0x3ff == 4 => 1,
            _ if primary_language(language) == Some(lcid & 0x3ff) => 2,
            _ => 0,
        })
        .max()
        .unwrap_or(0)
}

fn primary_language(language: Language) -> Option<u16> {
    Some(match language {
        Language::Auto => return None,
        Language::English => 0x09,
        Language::SimplifiedChinese | Language::TraditionalChinese => 0x04,
        Language::Japanese => 0x11,
        Language::Korean => 0x12,
        Language::French => 0x0c,
        Language::German => 0x07,
        Language::Spanish => 0x0a,
        Language::Russian => 0x19,
        Language::Portuguese => 0x16,
        Language::Italian => 0x10,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{atomic::AtomicBool, mpsc},
        time::Instant,
    };

    struct FakeEngine {
        running: Arc<AtomicBool>,
        starts: mpsc::Sender<String>,
    }

    impl Engine for FakeEngine {
        fn start(
            &mut self,
            text: &str,
            _language: Language,
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
    fn voice_matching_handles_regional_hex_attributes_and_chinese_preferences() {
        assert_eq!(language_score(Language::English, "809;409"), 2);
        assert_eq!(language_score(Language::Japanese, "411"), 2);
        assert_eq!(language_score(Language::Korean, "412"), 2);
        assert_eq!(language_score(Language::Portuguese, "416;816"), 2);
        assert_eq!(language_score(Language::SimplifiedChinese, "804"), 3);
        assert_eq!(language_score(Language::TraditionalChinese, "404"), 3);
        assert_eq!(language_score(Language::TraditionalChinese, "c04"), 2);
        assert_eq!(language_score(Language::SimplifiedChinese, "404"), 1);
        assert_eq!(language_score(Language::Japanese, "409;bad-value"), 0);
        assert_eq!(language_score(Language::Auto, "409"), 0);
    }

    #[test]
    fn replaced_pending_text_is_never_spoken_and_stop_purges_without_completion() {
        let shared = Arc::new(Shared::default());
        let speech = Speech {
            shared: Arc::clone(&shared),
        };
        let old = speech.start("discarded old text".into(), Language::English);
        let latest = speech.start("new text".into(), Language::English);
        assert_ne!(old, latest);
        let (events_tx, events) = mpsc::channel();
        let (starts_tx, starts) = mpsc::channel();
        let running = Arc::new(AtomicBool::new(false));
        let engine_running = Arc::clone(&running);
        let worker_shared = Arc::clone(&shared);
        let worker = thread::spawn(move || {
            run(
                worker_shared,
                |event| {
                    events_tx.send(event).unwrap();
                },
                || {
                    Ok(FakeEngine {
                        running: Arc::clone(&engine_running),
                        starts: starts_tx.clone(),
                    })
                },
            );
        });
        assert_eq!(
            starts.recv_timeout(Duration::from_secs(2)).unwrap(),
            "new text"
        );
        let started = events.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(started.token, latest);
        assert_eq!(started.result, Ok(Outcome::Started));
        // The currently playing sentence is still active. Replacing it must
        // purge that voice and start the next text without waiting for done().
        let replacement = speech.start("replacement while speaking".into(), Language::English);
        assert_eq!(
            starts.recv_timeout(Duration::from_secs(2)).unwrap(),
            "replacement while speaking"
        );
        let restarted = events.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(restarted.token, replacement);
        assert_eq!(restarted.result, Ok(Outcome::Started));
        let before = Instant::now();
        let stop = speech.stop();
        let stopped = events.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(stopped.token, stop);
        assert_eq!(stopped.result, Ok(Outcome::Stopped));
        assert!(!running.load(Ordering::Acquire));
        assert!(before.elapsed() < Duration::from_secs(2));
        assert!(starts.try_recv().is_err());
        assert!(events.try_recv().is_err());
        drop(speech);
        worker.join().unwrap();
    }

    #[test]
    fn missing_engine_is_an_event_and_does_not_prevent_stop_or_worker_shutdown() {
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
                || Err("no installed local voice".into()),
            );
        });
        let token = speech.start("test".into(), Language::Japanese);
        let failed = events.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(failed.token, token);
        assert_eq!(failed.result, Err("no installed local voice".into()));
        let stop = speech.stop();
        let stopped = events.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(stopped.token, stop);
        assert_eq!(stopped.result, Ok(Outcome::Stopped));
        drop(speech);
        worker.join().unwrap();
    }
}
