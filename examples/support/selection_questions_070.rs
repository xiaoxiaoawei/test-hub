// Live UI questions exercise failure recovery, cancellation, drafts and copying.
// Included inside the Windows controller smoke; all response text is public and
// only the owned local HTTP service is used.
use super::*;
use lingoleaf::platform::selection;

pub(super) const REQUESTS: [&str; 4] = [
    "question:failure",
    "question:retry",
    "question:stopped",
    "question:after-stop",
];
pub(super) const LATE_ANSWER: &str = "不应显示的迟到回答";

const QUESTION_FAIL: &str = "Retry this temporary failure.";
const QUESTION_STOP: &str = "Stop this slow answer.";
const QUESTION_AFTER_STOP: &str = "Continue after stopping.";
const NEXT_DRAFT: &str = "A new draft written while the answer runs.";
const ANSWER_RETRY: &str = "重试成功";
const ANSWER_AFTER_STOP: &str = "继续成功";
const ANSWER_PARTIAL: &str = "停止前片段";
const COPY_CONVERSATION: usize = 266;

pub(super) fn check(inline: HWND, mock: &Mock) -> Result<(), String> {
    let input = unsafe { GetDlgItem(inline, QUESTION) };
    let button = unsafe { GetDlgItem(inline, ASK) };
    if input.is_null() || button.is_null() {
        return Err("The question recovery composer was not created.".into());
    }
    let previous = format!("静译文\n\n你：{QUESTION_NEW}\n轻译：{ANSWER_NEW}");
    ask(inline, QUESTION_FAIL)?;
    wait_for("a failed answer preserving the submitted question", || {
        Ok(text(inline, 254)?.contains("提问失败")
            && text(inline, QUESTION)? == QUESTION_FAIL
            && text(inline, 251)?.contains("503")
            && unsafe { IsWindowEnabled(button) } != 0)
    })
    .map_err(|error| question_failure(error, mock, inline))?;
    ask(inline, QUESTION_FAIL)?;
    let retried = format!("{previous}\n\n你：{QUESTION_FAIL}\n轻译：{ANSWER_RETRY}");
    wait_for(
        "a successful retry without a failed history exchange",
        || Ok(text(inline, 251)? == retried && text(inline, 254)?.contains("回答完成")),
    )
    .map_err(|error| question_failure(error, mock, inline))?;

    if mock.finish_control.load(Ordering::Acquire) {
        return Err("The controlled answer gate opened before the cancellation test.".into());
    }
    ask(inline, QUESTION_STOP)?;
    wait_for("a cancellable partial answer", || {
        Ok(text(inline, 251)?.ends_with(ANSWER_PARTIAL)
            && text(inline, ASK)?.contains("停止")
            && unsafe { IsWindowEnabled(button) } != 0)
    })
    .map_err(|error| question_failure(error, mock, inline))?;
    let draft = wide(NEXT_DRAFT);
    send(input, WM_SETTEXT, 0, draft.as_ptr() as isize)?;
    press_button(inline, ASK)?;
    wait_for(
        "stopping an answer without overwriting a newer draft",
        || {
            let output = text(inline, 251)?;
            if output.contains(LATE_ANSWER) {
                return Err("A canceled answer's late tail became visible.".into());
            }
            Ok(text(inline, QUESTION)? == NEXT_DRAFT
                && text(inline, 254)?.contains("已停止")
                && output.contains(ANSWER_PARTIAL)
                && output.contains("已停止")
                && !text(inline, ASK)?.contains("停止"))
        },
    )
    .map_err(|error| question_failure(error, mock, inline))?;

    // Keep the old server blocked. A new question must finish before that gate
    // is released, proving cancellation does not merely hide an old request.
    ask(inline, QUESTION_AFTER_STOP)?;
    let continued = format!("{retried}\n\n你：{QUESTION_AFTER_STOP}\n轻译：{ANSWER_AFTER_STOP}");
    wait_for(
        "a fresh answer while the canceled response remains blocked",
        || {
            let output = text(inline, 251)?;
            if output.contains(QUESTION_STOP) || output.contains(LATE_ANSWER) {
                return Err("A canceled exchange leaked into the next answer.".into());
            }
            Ok(output == continued && text(inline, 254)?.contains("回答完成"))
        },
    )
    .map_err(|error| question_failure(error, mock, inline))?;
    if mock.finish_control.load(Ordering::Acquire) {
        return Err("The canceled response completed before the fresh question.".into());
    }

    // These intentionally distinct commands protect the existing copy-only-
    // translation behavior and the new opt-in copy of the successful transcript.
    if unsafe { PostMessageW(inline, WM_COMMAND, COPY_CONVERSATION, 0) } == 0 {
        return Err("Cannot queue copying the successful conversation.".into());
    }
    wait_for("the completed transcript copy command", || {
        Ok(text(inline, 254)? == "译文与问答已复制")
    })?;
    if selection::read_clipboard_text()?.text.replace("\r\n", "\n") != continued {
        return Err("The complete-copy command did not write the successful transcript.".into());
    }
    press_button(inline, 104)?;
    wait_for("the completed translation-only copy command", || {
        Ok(text(inline, 254)? == "译文已复制")
    })?;
    if selection::read_clipboard_text()?.text != "静译文" {
        return Err("The copy glyph did not preserve translation-only copying.".into());
    }
    println!("Question controls passed: HTTP 503 retry, failure/stop history isolation, newer draft preservation, independent next request before the old gate opens, and distinct translation/transcript copy commands.");
    Ok(())
}

pub(super) fn try_serve(
    body: &serde_json::Value,
    stream: &mut TcpStream,
    requests: &Mutex<Vec<String>>,
    control: &AtomicBool,
    stop: &AtomicBool,
) -> Result<bool, String> {
    let question = body
        .get("messages")
        .and_then(serde_json::Value::as_array)
        .and_then(|messages| messages.last())
        .and_then(|message| message.get("content"))
        .and_then(serde_json::Value::as_str);
    let Some(question @ (QUESTION_FAIL | QUESTION_STOP | QUESTION_AFTER_STOP)) = question else {
        return Ok(false);
    };
    let mut calls = requests
        .lock()
        .map_err(|_| "Question mock request lock failed.")?;
    let failure_seen = calls.iter().any(|label| label == REQUESTS[0]);
    let retry_seen = calls.iter().any(|label| label == REQUESTS[1]);
    let (label, history, first, last, blocked): (_, &[(&str, &str)], _, _, _) = match question {
        QUESTION_FAIL if !failure_seen => {
            (REQUESTS[0], &[(QUESTION_NEW, ANSWER_NEW)], "", "", false)
        }
        QUESTION_FAIL if !retry_seen => (
            REQUESTS[1],
            &[(QUESTION_NEW, ANSWER_NEW)],
            "重试",
            "成功",
            false,
        ),
        QUESTION_STOP => (
            REQUESTS[2],
            &[(QUESTION_NEW, ANSWER_NEW), (QUESTION_FAIL, ANSWER_RETRY)],
            ANSWER_PARTIAL,
            LATE_ANSWER,
            true,
        ),
        QUESTION_AFTER_STOP => (
            REQUESTS[3],
            &[(QUESTION_NEW, ANSWER_NEW), (QUESTION_FAIL, ANSWER_RETRY)],
            "继续",
            "成功",
            false,
        ),
        _ => return Err("The question fixture received an unexpected repeated retry.".into()),
    };
    validate_request(body, question, history)?;
    calls.push(label.to_owned());
    drop(calls);
    if label == REQUESTS[0] {
        stream
            .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .map_err(|_| "Cannot write the local temporary failure response.")?;
        return Ok(true);
    }
    let tail = match begin_sse_response(stream, first, last) {
        Ok(tail) => tail,
        Err(_) => return Ok(true),
    };
    let started = Instant::now();
    while blocked
        && !control.load(Ordering::Acquire)
        && !stop.load(Ordering::Acquire)
        && started.elapsed() < Duration::from_secs(20)
    {
        thread::sleep(Duration::from_millis(10));
    }
    // A stopped request will normally have disconnected by this point. The
    // caller later releases the gate and verifies it cannot repaint the view.
    let _ = stream.write_all(&tail).and_then(|_| stream.flush());
    Ok(true)
}

fn validate_request(
    body: &serde_json::Value,
    question: &str,
    history: &[(&str, &str)],
) -> Result<(), String> {
    if body["model"] != "selection-smoke-model" || body["stream"] != true {
        return Err(
            "The question recovery request changed its configured model or streaming choice."
                .into(),
        );
    }
    let messages = body["messages"]
        .as_array()
        .ok_or("The question recovery request has no message list.")?;
    let system = messages
        .first()
        .filter(|message| message["role"] == "system")
        .and_then(|message| message["content"].as_str())
        .ok_or("The question recovery request has no system instruction.")?;
    if !system.contains("primarily in Japanese")
        || !system.contains("Infer the passage's source language")
    {
        return Err("The question recovery request lost Auto -> Japanese preferences.".into());
    }
    let selection = messages
        .get(1)
        .filter(|message| message["role"] == "user")
        .and_then(|message| message["content"].as_str())
        .ok_or("The question recovery request has no reference-data message.")?;
    let reference: serde_json::Value = serde_json::from_str(selection)
        .map_err(|_| "The question recovery reference data was not JSON.")?;
    if reference
        != serde_json::json!({
            "kind":"selection_context",
            "source_text":"calm",
            "translation":"静译文",
        })
    {
        return Err(
            "The question recovery request referenced a stale selection or translation.".into(),
        );
    }
    let mut expected = Vec::with_capacity(history.len() * 2 + 1);
    for (question, answer) in history {
        expected.push(serde_json::json!({"role":"user","content":question}));
        expected.push(serde_json::json!({"role":"assistant","content":answer}));
    }
    expected.push(serde_json::json!({"role":"user","content":question}));
    if messages.len() != expected.len() + 2 || messages[2..] != expected {
        return Err(
            "A failed or canceled exchange leaked into history, or a successful exchange was lost."
                .into(),
        );
    }
    Ok(())
}
