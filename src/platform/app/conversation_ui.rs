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
    anchor: u64,
    epoch: u64,
    history: Vec<ConversationTurn>,
    pending: Option<String>,
    partial: String,
    error: Option<String>,
    stopped: bool,
    busy: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Completion {
    Answered,
    Failed,
    Stopped,
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
            anchor: 0,
            epoch: 0,
            history: Vec::new(),
            pending: None,
            partial: String::new(),
            error: None,
            stopped: false,
            busy: false,
        })
    }

    fn advance(&mut self) -> u64 {
        self.counter = self.counter.wrapping_add(1);
        self.latest.store(self.counter, Ordering::Release);
        self.counter
    }

    fn accepts(&self, generation: u64, anchor: u64, epoch: u64) -> bool {
        self.busy && generation == self.counter && anchor == self.anchor && epoch == self.epoch
    }

    fn stop(&mut self) {
        self.advance();
        self.busy = false;
        self.stopped = true;
        self.error = None;
        // Keep the visible partial answer, but never add it to request history.
    }

    fn complete(&mut self, result: Result<String, String>) -> Completion {
        self.busy = false;
        match result {
            Ok(answer) => {
                if let Some(question) = self.pending.take() {
                    self.history.push(ConversationTurn { question, answer });
                    prune_history(&mut self.history);
                }
                self.partial.clear();
                self.error = None;
                self.stopped = false;
                Completion::Answered
            }
            Err(error) if translation::is_cancelled_error(&error) => {
                self.stopped = true;
                self.error = None;
                Completion::Stopped
            }
            Err(error) => {
                self.stopped = false;
                self.error = Some(error);
                Completion::Failed
            }
        }
    }

    fn pending_answer(&self) -> String {
        if self.stopped {
            if self.partial.is_empty() {
                "已停止回答".into()
            } else {
                format!("{}\n（已停止）", self.partial)
            }
        } else if let Some(error) = &self.error {
            if self.partial.is_empty() {
                error.clone()
            } else {
                format!("{}\n\n{}", self.partial, error)
            }
        } else if self.partial.is_empty() {
            "正在思考…".into()
        } else {
            self.partial.clone()
        }
    }
}

impl Drop for State {
    fn drop(&mut self) {
        // The worker must discard callbacks that return after the UI closes.
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
    state.stopped = false;
    state.busy = false;
    state.anchor = 0;
    state.epoch = 0;
    set_text(app.inline_ui.question, "");
    set_text(app.result_ui.question, "");
    refresh(app);
}

fn context_ready(app: &App) -> bool {
    app.translation_ready
        && !app.streaming
        && !app.display_pending
        && app.active_epoch == app.settings_epoch
}

fn ready(app: &App) -> bool {
    context_ready(app) && !is_busy(&app.conversation)
}

pub(super) fn is_busy(state: &State) -> bool {
    state.busy
}

pub(super) unsafe fn refresh(app: &mut App) {
    let busy = is_busy(&app.conversation);
    let current = context_ready(app)
        && (!busy
            || (app.conversation.anchor == app.generation
                && app.conversation.epoch == app.active_epoch));
    let visual_busy = busy && current;
    let changed = CONVERSATION_BUSY.replace(visual_busy) != visual_busy;
    for button in [app.inline_ui.send, app.result_ui.send] {
        if changed {
            set_text(
                button,
                if visual_busy {
                    "停止回答"
                } else {
                    "发送问题"
                },
            );
            InvalidateRect(button, null(), 0);
        }
        if (IsWindowEnabled(button) != 0) != current {
            EnableWindow(button, current as i32);
        }
    }
}

unsafe fn focus_if_foreground(input: HWND) {
    if GetAncestor(GetForegroundWindow(), GA_ROOT) == GetAncestor(input, GA_ROOT) {
        SetFocus(input);
    }
}

unsafe fn restore_question(app: &App) {
    let Some(question) = &app.conversation.pending else {
        return;
    };
    for input in [app.inline_ui.question, app.result_ui.question] {
        // An asynchronous error or a stop must not erase the next draft.
        if text(input).is_empty() {
            set_text(input, question);
        }
    }
}

pub(super) unsafe fn cancel(app: &mut App) {
    if !is_busy(&app.conversation) {
        return;
    }
    app.conversation.stop();
    restore_question(app);
    translation_status(app, "已停止回答 · 可修改问题后重试");
    let original = app.translated.clone();
    super::render_translation(app, &original);
    refresh(app);
}

pub(super) unsafe fn submit(app: &mut App, owner: HWND) {
    let input = if owner == app.inline_window || GetAncestor(owner, GA_ROOT) == app.inline_window {
        app.inline_ui.question
    } else {
        app.result_ui.question
    };
    if is_busy(&app.conversation) {
        if context_ready(app)
            && app.conversation.anchor == app.generation
            && app.conversation.epoch == app.active_epoch
        {
            cancel(app);
            focus_if_foreground(input);
        }
        return;
    }
    if !ready(app) {
        return;
    }
    let question = text(input).trim().to_owned();
    if question.is_empty() {
        translation_status(app, "请输入问题");
        focus_if_foreground(input);
        return;
    }
    if question.chars().count() > translation::MAX_QUESTION_CHARS {
        translation_status(
            app,
            &format!("问题最多 {} 个字符", translation::MAX_QUESTION_CHARS),
        );
        focus_if_foreground(input);
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
    state.anchor = app.generation;
    state.epoch = app.active_epoch;
    state.partial.clear();
    state.error = None;
    state.stopped = false;
    state.busy = true;
    if state.worker.send(mailbox::Lane::Translation, job).is_err() {
        state.busy = false;
        state.error = Some("提问线程不可用，请重新打开轻译。".into());
        translation_status(app, "提问未启动");
    } else {
        set_text(input, "");
        // A real mouse submission permits the next draft, while a background
        // command must not activate an otherwise inactive translation window.
        focus_if_foreground(input);
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
    if !app.conversation.accepts(generation, anchor, epoch)
        || anchor != app.generation
        || epoch != app.settings_epoch
        || epoch != app.active_epoch
    {
        return;
    }
    match event {
        Event::Progress { text, .. } => app.conversation.partial = text,
        Event::Completed { result, .. } => match app.conversation.complete(result) {
            Completion::Answered => translation_status(app, "回答完成"),
            Completion::Failed => {
                restore_question(app);
                translation_status(app, "提问失败 · 问题已保留，可重试");
            }
            Completion::Stopped => {
                restore_question(app);
                translation_status(app, "已停止回答 · 可修改问题后重试");
            }
        },
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
        append_exchange(&mut output, question, &state.pending_answer());
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

    fn pending_state() -> State {
        let (worker, receiver) = mailbox::channel();
        drop(receiver);
        State {
            worker,
            latest: Arc::new(AtomicU64::new(1)),
            counter: 1,
            anchor: 42,
            epoch: 7,
            history: vec![ConversationTurn {
                question: "previous successful question".into(),
                answer: "previous successful answer".into(),
            }],
            pending: Some("explain this passage".into()),
            partial: "A readable partial answer".into(),
            error: None,
            stopped: false,
            busy: true,
        }
    }

    #[test]
    fn stopping_preserves_readable_text_but_rejects_late_events_and_history() {
        let mut state = pending_state();
        let old_generation = state.counter;
        assert!(state.accepts(old_generation, 42, 7));
        let history = state.history.clone();
        state.stop();
        assert!(!state.accepts(old_generation, 42, 7));
        assert_eq!(state.history, history);
        assert_eq!(state.pending.as_deref(), Some("explain this passage"));
        assert_eq!(
            state.pending_answer(),
            "A readable partial answer\n（已停止）"
        );
        assert_eq!(state.latest.load(Ordering::Acquire), state.counter);
    }

    #[test]
    fn failed_or_cancelled_answers_never_enter_a_subsequent_question_history() {
        let mut state = pending_state();
        let history = state.history.clone();
        assert_eq!(
            state.complete(Err("safe request error".into())),
            Completion::Failed
        );
        assert_eq!(state.history, history);
        assert_eq!(state.pending.as_deref(), Some("explain this passage"));
        assert_eq!(
            state.pending_answer(),
            "A readable partial answer\n\nsafe request error"
        );

        // A retry can include only the earlier successful history; the failed
        // attempt remains display state rather than a ConversationTurn.
        state.busy = true;
        state.partial.clear();
        state.error = None;
        state.stopped = false;
        state.advance();
        assert_eq!(state.history.len(), 1);
        assert_eq!(
            state.complete(Err(translation::CANCELLED.into())),
            Completion::Stopped
        );
        assert_eq!(state.pending_answer(), "已停止回答");
        assert_eq!(state.history, history);

        state.busy = true;
        state.stopped = false;
        state.advance();
        assert_eq!(
            state.complete(Ok("complete retry answer".into())),
            Completion::Answered
        );
        assert_eq!(state.history.len(), 2);
        assert_eq!(
            state.history.last().unwrap().answer,
            "complete retry answer"
        );
        assert!(state.pending.is_none());
        assert!(!state.busy);
    }

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
