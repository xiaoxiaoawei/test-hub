//! Selection-scoped questions use their own worker so a slow answer cannot
//! hold up the translation worker or the next selection.

use super::*;
use translation::{ConversationContext, ConversationTurn};

struct Job {
    generation: u64,
    anchor: u64,
    epoch: u64,
    settings: config::Settings,
    source: String,
    translated: String,
    history: Vec<ConversationTurn>,
    question: String,
}

pub(super) enum Event {
    Progress {
        generation: u64,
        anchor: u64,
        epoch: u64,
        text: String,
    },
    Completed {
        generation: u64,
        anchor: u64,
        epoch: u64,
        result: Result<String, String>,
    },
}

pub(super) struct State {
    worker: mailbox::Sender<Job>,
    latest: Arc<AtomicU64>,
    counter: u64,
    history: Vec<ConversationTurn>,
    pending: Option<String>,
    partial: String,
    error: Option<String>,
    busy: bool,
}

impl State {
    pub(super) fn new(main: HWND, sender: mpsc::Sender<super::Event>) -> Result<Self, String> {
        let (worker, receiver) = mailbox::channel::<Job>();
        let latest = Arc::new(AtomicU64::new(0));
        let worker_latest = Arc::clone(&latest);
        let destination = main as isize;
        thread::Builder::new()
            .name("conversation".into())
            .spawn(move || {
                let mut client = translation::TranslationClient::new();
                while let Some(job) = receiver.recv() {
                    let cancelled = || worker_latest.load(Ordering::Acquire) != job.generation;
                    if cancelled() {
                        continue;
                    }
                    let mut last_progress = Instant::now()
                        .checked_sub(Duration::from_millis(40))
                        .unwrap_or_else(Instant::now);
                    let result = client.ask_streaming(
                        &job.settings,
                        ConversationContext {
                            source: &job.source,
                            translation: &job.translated,
                            history: &job.history,
                        },
                        &job.question,
                        cancelled,
                        |text| {
                            if !cancelled() && last_progress.elapsed() >= Duration::from_millis(40)
                            {
                                let _ = sender.send(super::Event::Conversation(Event::Progress {
                                    generation: job.generation,
                                    anchor: job.anchor,
                                    epoch: job.epoch,
                                    text: text.to_owned(),
                                }));
                                unsafe {
                                    PostMessageW(destination as HWND, WM_WORK, 0, 0);
                                }
                                last_progress = Instant::now();
                            }
                        },
                    );
                    if cancelled() {
                        continue;
                    }
                    if sender
                        .send(super::Event::Conversation(Event::Completed {
                            generation: job.generation,
                            anchor: job.anchor,
                            epoch: job.epoch,
                            result,
                        }))
                        .is_err()
                    {
                        break;
                    }
                    unsafe {
                        PostMessageW(destination as HWND, WM_WORK, 0, 0);
                    }
                }
            })
            .map_err(|error| format!("无法启动提问线程：{error}"))?;
        Ok(Self {
            worker,
            latest,
            counter: 0,
            history: Vec::new(),
            pending: None,
            partial: String::new(),
            error: None,
            busy: false,
        })
    }

    fn advance(&mut self) -> u64 {
        self.counter = self.counter.wrapping_add(1);
        self.latest.store(self.counter, Ordering::Release);
        self.counter
    }
}

impl Drop for State {
    fn drop(&mut self) {
        // A synchronous WinHTTP read may still be returning after the UI closes.
        self.latest.fetch_add(1, Ordering::Release);
    }
}

pub(super) unsafe fn reset(app: &mut App) {
    let state = &mut app.conversation;
    state.advance();
    state.history.clear();
    state.pending = None;
    state.partial.clear();
    state.error = None;
    state.busy = false;
    set_text(app.inline_ui.question, "");
    set_text(app.result_ui.question, "");
    refresh(app);
}

fn ready(app: &App) -> bool {
    app.translation_ready
        && !app.streaming
        && !app.display_pending
        && app.active_epoch == app.settings_epoch
        && !app.conversation.busy
}

pub(super) unsafe fn refresh(app: &mut App) {
    let enabled = ready(app) as i32;
    EnableWindow(app.inline_ui.send, enabled);
    EnableWindow(app.result_ui.send, enabled);
}

pub(super) unsafe fn submit(app: &mut App, owner: HWND) {
    if !ready(app) {
        return;
    }
    let input = if owner == app.inline_window || GetAncestor(owner, GA_ROOT) == app.inline_window {
        app.inline_ui.question
    } else {
        app.result_ui.question
    };
    let question = text(input).trim().to_owned();
    if question.is_empty() {
        translation_status(app, "请输入问题");
        SetFocus(input);
        return;
    }
    if question.chars().count() > translation::MAX_QUESTION_CHARS {
        translation_status(
            app,
            &format!("问题最多 {} 个字符", translation::MAX_QUESTION_CHARS),
        );
        SetFocus(input);
        return;
    }
    if let Err(error) = app.settings.validate() {
        translation_status(app, &error);
        return;
    }
    let state = &mut app.conversation;
    let generation = state.advance();
    let job = Job {
        generation,
        anchor: app.generation,
        epoch: app.settings_epoch,
        settings: app.settings.clone(),
        source: app.active_text.clone(),
        translated: app.translated.clone(),
        history: state.history.clone(),
        question: question.clone(),
    };
    state.pending = Some(question);
    state.partial.clear();
    state.error = None;
    state.busy = true;
    if state.worker.send(mailbox::Lane::Translation, job).is_err() {
        state.busy = false;
        state.error = Some("提问线程不可用，请重新打开轻译。".into());
        translation_status(app, "提问未启动");
    } else {
        set_text(input, "");
        // The send button becomes disabled while this request runs. Keep the
        // editor focused so a mouse submission still permits the next draft.
        SetFocus(input);
        translation_status(app, "正在思考…");
    }
    let original = app.translated.clone();
    super::render_translation(app, &original);
    refresh(app);
}

pub(super) unsafe fn handle_event(app: &mut App, event: Event) {
    let (generation, anchor, epoch) = match &event {
        Event::Progress {
            generation,
            anchor,
            epoch,
            ..
        }
        | Event::Completed {
            generation,
            anchor,
            epoch,
            ..
        } => (*generation, *anchor, *epoch),
    };
    if generation != app.conversation.counter
        || anchor != app.generation
        || epoch != app.settings_epoch
        || epoch != app.active_epoch
        || !app.conversation.busy
    {
        return;
    }
    match event {
        Event::Progress { text, .. } => app.conversation.partial = text,
        Event::Completed { result, .. } => {
            let state = &mut app.conversation;
            state.busy = false;
            state.partial.clear();
            match result {
                Ok(answer) => {
                    if let Some(question) = state.pending.take() {
                        state.history.push(ConversationTurn { question, answer });
                        prune_history(&mut state.history);
                    }
                    state.error = None;
                    translation_status(app, "回答完成");
                }
                Err(error) if translation::is_cancelled_error(&error) => {
                    state.pending = None;
                    state.error = None;
                    translation_status(app, "已取消");
                }
                Err(error) => {
                    state.error = Some(error);
                    translation_status(app, "提问失败，可重试");
                }
            }
        }
    }
    let original = app.translated.clone();
    super::render_translation(app, &original);
    refresh(app);
}

pub(super) fn display_text(app: &App) -> String {
    let state = &app.conversation;
    let mut output = app.translated.clone();
    for turn in &state.history {
        append_exchange(&mut output, &turn.question, &turn.answer);
    }
    if let Some(question) = &state.pending {
        let answer = state.error.as_deref().unwrap_or_else(|| {
            if state.partial.is_empty() {
                "正在思考…"
            } else {
                &state.partial
            }
        });
        append_exchange(&mut output, question, answer);
    }
    output
}

pub(super) fn has_content(state: &State) -> bool {
    !state.history.is_empty() || state.pending.is_some()
}

fn append_exchange(output: &mut String, question: &str, answer: &str) {
    output.push_str("\n\n你：");
    output.push_str(question);
    output.push_str("\n轻译：");
    output.push_str(answer);
}

fn prune_history(history: &mut Vec<ConversationTurn>) {
    let mut characters = history.iter().fold(0usize, |total, turn| {
        total
            .saturating_add(turn.question.chars().count())
            .saturating_add(turn.answer.chars().count())
    });
    while history.len() > 1
        && (history.len() > translation::MAX_CONVERSATION_TURNS
            || characters > translation::MAX_CONVERSATION_CONTEXT_CHARS)
    {
        let removed = history.remove(0);
        characters = characters
            .saturating_sub(removed.question.chars().count())
            .saturating_sub(removed.answer.chars().count());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_retains_recent_turns_with_a_unicode_character_budget() {
        let mut history: Vec<_> = (0..10)
            .map(|index| ConversationTurn {
                question: format!("question-{index}"),
                answer: "答".repeat(2_000),
            })
            .collect();
        prune_history(&mut history);
        assert_eq!(history.len(), translation::MAX_CONVERSATION_TURNS);
        assert_eq!(history[0].question, "question-4");
        history.push(ConversationTurn {
            question: "huge newest answer".into(),
            answer: "答".repeat(translation::MAX_CONVERSATION_CONTEXT_CHARS + 1),
        });
        prune_history(&mut history);
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].question, "huge newest answer");
        history.push(ConversationTurn {
            question: "next question".into(),
            answer: "short answer".into(),
        });
        prune_history(&mut history);
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].question, "next question");
    }
}
