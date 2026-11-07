// Actual RichEdit formatting checks in the owned application process. These
// query native formatting; merely deleting Markdown punctuation cannot pass.
use super::*;
use std::{ffi::c_void, mem::size_of_val};
use windows_sys::Win32::{
    Foundation::{CloseHandle, BOOL, HANDLE},
    System::{
        Memory::{
            VirtualAllocEx, VirtualFreeEx, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE,
        },
        Threading::{OpenProcess, PROCESS_VM_OPERATION, PROCESS_VM_READ, PROCESS_VM_WRITE},
    },
};

const EM_GETCHARFORMAT: u32 = WM_USER + 58;
const SCF_SELECTION: usize = 1;
const BOLD: u32 = 1;
const ITALIC: u32 = 2;
const STRIKEOUT: u32 = 8;

#[link(name = "kernel32")]
extern "system" {
    fn ReadProcessMemory(
        process: HANDLE,
        base: *const c_void,
        buffer: *mut c_void,
        length: usize,
        read: *mut usize,
    ) -> BOOL;
    fn WriteProcessMemory(
        process: HANDLE,
        base: *mut c_void,
        buffer: *const c_void,
        length: usize,
        written: *mut usize,
    ) -> BOOL;
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NativeFormat {
    cb_size: u32,
    mask: u32,
    effects: u32,
    height: i32,
    offset: i32,
    text_color: u32,
    character_set: u8,
    family: u8,
    face: [u16; 32],
}

struct Process(HANDLE);
impl Drop for Process {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}
struct RemoteFormat<'a> {
    process: &'a Process,
    address: *mut c_void,
    release: bool,
}
impl Drop for RemoteFormat<'_> {
    fn drop(&mut self) {
        if self.release {
            unsafe { VirtualFreeEx(self.process.0, self.address, 0, MEM_RELEASE) };
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NativeRange {
    first: i32,
    last: i32,
}
#[repr(C)]
struct NativeFind {
    search: NativeRange,
    text: *const u16,
    found: NativeRange,
}

fn write_remote(
    process: &Process,
    address: *mut c_void,
    source: *const c_void,
    length: usize,
) -> Result<(), String> {
    let mut written = 0;
    if unsafe { WriteProcessMemory(process.0, address, source, length, &mut written) } == 0
        || written != length
    {
        Err("Cannot initialize the owned child's bounded native probe buffer.".into())
    } else {
        Ok(())
    }
}
fn read_remote(
    process: &Process,
    address: *const c_void,
    target: *mut c_void,
    length: usize,
) -> Result<(), String> {
    let mut read = 0;
    if unsafe { ReadProcessMemory(process.0, address, target, length, &mut read) } == 0
        || read != length
    {
        Err("Cannot read the owned child's actual native formatting structure.".into())
    } else {
        Ok(())
    }
}

fn native_probe(
    editor: HWND,
    expected_pid: u32,
    token: &str,
) -> Result<(NativeFormat, NativeRange), String> {
    let mut pid = 0;
    let mut class = [0_u16; 64];
    unsafe { GetWindowThreadProcessId(editor, &mut pid) };
    let length = unsafe { GetClassNameW(editor, class.as_mut_ptr(), class.len() as i32) }.max(0);
    let class = String::from_utf16_lossy(&class[..length as usize]);
    if pid != expected_pid || !class.eq_ignore_ascii_case("RICHEDIT50W") {
        return Err(format!(
            "The format probe target is not the owned RichEdit output: pid={pid}, class={class}."
        ));
    }
    let needle = wide(token);
    if needle.len() > 128 {
        return Err("The fixed public format token exceeds the bounded probe limit.".into());
    }
    let process = Process(unsafe {
        OpenProcess(
            PROCESS_VM_OPERATION | PROCESS_VM_READ | PROCESS_VM_WRITE,
            0,
            expected_pid,
        )
    });
    if process.0.is_null() {
        return Err("Cannot open only the owned child for its formatting probe.".into());
    }
    let address = unsafe {
        VirtualAllocEx(
            process.0,
            null(),
            512,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_READWRITE,
        )
    };
    if address.is_null() {
        return Err("Cannot allocate the owned child's bounded format buffer.".into());
    }
    let mut remote = RemoteFormat {
        process: &process,
        address,
        release: true,
    };
    let find_address = unsafe { address.byte_add(128) };
    let text_address = unsafe { address.byte_add(192) };
    let mut find = NativeFind {
        search: NativeRange { first: 0, last: -1 },
        text: text_address.cast(),
        found: NativeRange {
            first: -1,
            last: -1,
        },
    };
    write_remote(
        &process,
        text_address,
        needle.as_ptr().cast(),
        needle.len() * 2,
    )?;
    write_remote(
        &process,
        find_address,
        (&find as *const NativeFind).cast(),
        size_of_val(&find),
    )?;
    // Native search returns actual UTF-16 document positions, including table
    // cell markers and paragraph breaks that WM_GETTEXT normalizes differently.
    const EM_FINDTEXTEXW: u32 = WM_USER + 124;
    let found = match send(editor, EM_FINDTEXTEXW, 1 | 4, find_address as isize) {
        Ok(found) => found,
        Err(error) => {
            remote.release = false;
            return Err(error);
        }
    };
    if (found as isize) < 0 {
        return Err(format!(
            "The native rendered Markdown token {token:?} is missing."
        ));
    }
    read_remote(
        &process,
        find_address,
        (&mut find as *mut NativeFind).cast(),
        size_of_val(&find),
    )?;
    let previous = send(editor, EM_GETSEL, 0, 0)?;
    send(
        editor,
        EM_SETSEL,
        find.found.first as usize,
        find.found.last as isize,
    )?;
    let mut format: NativeFormat = unsafe { zeroed() };
    format.cb_size = size_of_val(&format) as u32;
    write_remote(
        &process,
        address,
        (&format as *const NativeFormat).cast(),
        size_of_val(&format),
    )?;
    // WM_USER pointers are not automatically marshalled across processes.
    // Never send a caller-local structure to another process's RichEdit.
    if let Err(error) = send(editor, EM_GETCHARFORMAT, SCF_SELECTION, address as isize) {
        // A timed-out receiver might still access this buffer. Let the owned
        // child's OS teardown reclaim it when Application::drop stops that child.
        remote.release = false;
        return Err(error);
    }
    read_remote(
        &process,
        address,
        (&mut format as *mut NativeFormat).cast(),
        size_of_val(&format),
    )?;
    send(
        editor,
        EM_SETSEL,
        previous & 0xffff,
        ((previous >> 16) & 0xffff) as isize,
    )?;
    Ok((format, find.found))
}

fn native_format(editor: HWND, expected_pid: u32, token: &str) -> Result<NativeFormat, String> {
    native_probe(editor, expected_pid, token).map(|value| value.0)
}

fn copy_native_code(editor: HWND, pid: u32) -> Result<(), String> {
    let (_, first) = native_probe(editor, pid, "let icon")?;
    let (_, last) = native_probe(editor, pid, "let size = 32;")?;
    let previous = send(editor, EM_GETSEL, 0, 0)?;
    send(editor, EM_SETSEL, first.first as usize, last.last as isize)?;
    send(editor, WM_COPY, 0, 0)?;
    let copied = lingoleaf::platform::selection::read_clipboard_text()?
        .text
        .replace("\r\n", "\n");
    send(
        editor,
        EM_SETSEL,
        previous & 0xffff,
        ((previous >> 16) & 0xffff) as isize,
    )?;
    if copied != "let icon = \"叶🌿\";\nlet size = 32;" {
        return Err(format!(
            "Native copying collapsed or changed a multiline Markdown code block: {copied:?}."
        ));
    }
    Ok(())
}

fn require_effect(editor: HWND, pid: u32, token: &str, effect: u32) -> Result<(), String> {
    let actual = native_format(editor, pid, token)?;
    if actual.mask & effect == 0 || actual.effects & effect == 0 {
        return Err(format!(
            "Markdown token {token:?} did not receive its native RichEdit style {effect}."
        ));
    }
    Ok(())
}

fn require_monospace(editor: HWND, pid: u32, token: &str) -> Result<(), String> {
    let actual = native_format(editor, pid, token)?;
    let length = actual
        .face
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(actual.face.len());
    let face = String::from_utf16_lossy(&actual.face[..length]).to_ascii_lowercase();
    if ![
        "consolas",
        "cascadia",
        "courier",
        "lucida console",
        "monospace",
    ]
    .iter()
    .any(|name| face.contains(name))
    {
        return Err(format!(
            "Markdown code token {token:?} is not monospace: {face:?}."
        ));
    }
    Ok(())
}

pub(super) const REQUESTS: [&str; 3] = [
    "markdown:translation",
    "markdown:question",
    "markdown:polish",
];
const POLISH_PROMPT: &str = "结合原文检查并润色当前译文，保留原意、事实与数字。只输出目标语言的完整润色译文，不列检查要点，不加开场白、解释、总结或重复版本；无需修改则直接输出原译文。";
const POLISH_FIRST: &str = "**Polished result中文**";
const POLISH_TAIL: &str = " 完整结束。";
const UNSENT_DRAFT: &str = "An unsent *draft* remains literal.";
const SPEAK: i32 = 285;
const POLISH: i32 = 286;
const QUESTION_MARKDOWN: &str = "Explain *this* Markdown response.";
const TRANSLATION_FIRST: &str = "# Clear translation\n\n**bold中文** with *italic* and ~~removed~~.\nInline `value` and [docs](https://example.com).\n\n- first item\n- second item\n\n> quoted text\n\n```rust\nlet icon = \"叶🌿\";\nlet size = 32;\n```\n\n| Word | Meaning |\n| --- | --- |\n| warm | 温暖 |\n\n<script>literal</script>\n\n**stream";
const TRANSLATION_TAIL: &str = "_suffix**\n\n```text\nunfinished_code";
const ANSWER_MARKDOWN: &str = "## Clear answer\n\n**response中文** with `reply_code`.\n\n1. alpha\n2. beta\n\n```text\n{\\rtf1\\b unsafe}\n```";

fn translation() -> String {
    format!("{TRANSLATION_FIRST}{TRANSLATION_TAIL}")
}

pub(super) fn check(
    source: &Source,
    app: &Application,
    background: HWND,
    inline: HWND,
    mock: &Mock,
) -> Result<(), String> {
    mock.finish_light.store(false, Ordering::Release);
    select_word(source, background, "markdown")?;
    require_same_inline(app, inline)?;
    wait_for(
        "a streamed Markdown heading and code before its incomplete tail",
        || {
            let rendered = text(inline, 251)?;
            Ok(rendered.contains("Clear translation") && rendered.contains("stream"))
        },
    )?;
    let editor = unsafe { GetDlgItem(inline, 251) };
    let pid = app.child.id();
    check_translation_format(editor, pid)?;
    if mock.finish_light.load(Ordering::Acquire) {
        return Err("The Markdown body gate opened before the incomplete-stream check.".into());
    }
    mock.finish_light.store(true, Ordering::Release);
    wait_for("the fully rendered Markdown translation", || {
        Ok(text(inline, 251)?.contains("stream_suffix") && text(inline, 254)?.contains("完成"))
    })?;
    let rendered = text(inline, 251)?;
    for required in [
        "first item",
        "second item",
        "quoted text",
        "Word",
        "Meaning",
        "warm",
        "温暖",
        "叶🌿",
        "<script>literal</script>",
    ] {
        if !rendered.contains(required) {
            return Err(format!("Markdown rendering lost public text {required:?}."));
        }
    }
    if rendered.contains("**bold中文**")
        || rendered.contains("```rust")
        || rendered.contains("| --- |")
    {
        return Err(
            "The translation still displays raw Markdown syntax instead of formatting.".into(),
        );
    }
    require_effect(editor, pid, "stream_suffix", BOLD)?;
    copy_native_code(editor, pid)?;
    press_button(inline, 104)?;
    wait_for("the plain-text translation copy", || {
        Ok(text(inline, 254)?.contains("译文已复制"))
    })?;
    let plain = lingoleaf::platform::selection::read_clipboard_text()?.text;
    if !plain.contains("bold中文")
        || plain.contains("**bold中文**")
        || plain.contains("```rust")
        || plain.contains(QUESTION_MARKDOWN)
    {
        return Err(
            "Default translation copying did not produce only readable plain translation text."
                .into(),
        );
    }

    require_ready_actions(inline)?;
    mock.finish_control.store(false, Ordering::Release);
    ask(inline, QUESTION_MARKDOWN)?;
    wait_for("the streaming Markdown answer", || {
        Ok(text(inline, 251)?.contains("Clear answer"))
    })?;
    if unsafe { IsWindowEnabled(GetDlgItem(inline, POLISH)) } != 0 {
        return Err("Polish remained enabled while a reply was still streaming.".into());
    }
    mock.finish_control.store(true, Ordering::Release);
    wait_for("the rendered Markdown answer", || {
        Ok(text(inline, 251)?.contains("Clear answer") && text(inline, 254)?.contains("回答完成"))
    })?;
    let rendered = text(inline, 251)?;
    for expected in [
        QUESTION_MARKDOWN,
        "response中文",
        "alpha",
        "beta",
        "{\\rtf1\\b unsafe}",
    ] {
        if !rendered.contains(expected) {
            return Err(format!(
                "The rendered conversation lost or reinterpreted literal public text {expected:?}."
            ));
        }
    }
    require_effect(editor, pid, "response中文", BOLD)?;
    require_monospace(editor, pid, "reply_code")?;
    let heading = native_format(editor, pid, "Clear answer")?;
    let body = native_format(editor, pid, "with")?;
    if heading.height <= body.height {
        return Err("A Markdown reply heading did not receive a larger native font.".into());
    }
    copy_command(inline, 266)?;
    wait_for("the plain-text full conversation copy", || {
        Ok(text(inline, 254)?.contains("译文与问答已复制"))
    })?;
    let plain = lingoleaf::platform::selection::read_clipboard_text()?.text;
    if !plain.contains(QUESTION_MARKDOWN)
        || !plain.contains("response中文")
        || plain.contains("**response中文**")
    {
        return Err(
            "Full conversation copying did not preserve the question and rendered plain answer."
                .into(),
        );
    }
    copy_command(inline, 287)?;
    wait_for("the explicitly requested Markdown source copy", || {
        // Do not OpenClipboard while the queued application command is trying
        // to write it; its native status is the completion barrier.
        Ok(text(inline, 254)? == "Markdown 原文已复制")
    })?;
    let raw = lingoleaf::platform::selection::read_clipboard_text()?
        .text
        .replace("\r\n", "\n");
    if !raw.contains(&translation()) || !raw.contains(ANSWER_MARKDOWN) {
        return Err(
            "Explicit Markdown copying changed the original translation or reply source.".into(),
        );
    }
    let input = unsafe { GetDlgItem(inline, QUESTION) };
    let draft = wide(UNSENT_DRAFT);
    send(input, WM_SETTEXT, 0, draft.as_ptr() as isize)?;
    mock.finish_light.store(false, Ordering::Release);
    require_ready_actions(inline)?;
    press_button(inline, POLISH)?;
    wait_for("the Polish response while its tail is held", || {
        Ok(text(inline, 251)?.contains("Polished result中文"))
    })?;
    if text(inline, QUESTION)? != UNSENT_DRAFT
        || unsafe { IsWindowEnabled(GetDlgItem(inline, POLISH)) } != 0
        || mock.finish_light.load(Ordering::Acquire)
    {
        return Err(
            "Polishing discarded the draft or allowed another polish during its active request."
                .into(),
        );
    }
    mock.finish_light.store(true, Ordering::Release);
    wait_for(
        "the complete Polish result without altering its unsent draft",
        || Ok(text(inline, 251)?.contains("完整结束") && text(inline, 254)?.contains("回答完成")),
    )?;
    require_ready_actions(inline)?;
    if text(inline, QUESTION)? != UNSENT_DRAFT {
        return Err("Completing Polish replaced an unsent user draft.".into());
    }
    press_button(inline, 104)?;
    wait_for("translation-only copying after Polish", || {
        Ok(text(inline, 254)?.contains("译文已复制"))
    })?;
    let original = lingoleaf::platform::selection::read_clipboard_text()?.text;
    if !original.contains("bold中文")
        || original.contains("Polished result")
        || original.contains("**bold中文**")
    {
        return Err("Polish overwrote the selected passage's original translation.".into());
    }
    send(inline, WM_NULL, 0, 0)?;
    println!("Markdown passed: actual native heading/bold/italic/strikeout/monospace formats; streamed incomplete syntax; list/table/Unicode/literal HTML and RTF text; plain and explicit Markdown copying; raw question context.");
    println!("Polish passed: one real POST with raw source/translation/history, busy-state gating, original translation and unsent draft preserved.");
    Ok(())
}

fn require_ready_actions(parent: HWND) -> Result<(), String> {
    for id in [SPEAK, POLISH] {
        let button = unsafe { GetDlgItem(parent, id) };
        if button.is_null()
            || unsafe { IsWindowVisible(button) } == 0
            || unsafe { IsWindowEnabled(button) } == 0
        {
            return Err(format!(
                "The ready translation is missing enabled action {id}."
            ));
        }
    }
    Ok(())
}

fn copy_command(parent: HWND, id: usize) -> Result<(), String> {
    if unsafe { PostMessageW(parent, WM_COMMAND, id, 0) } == 0 {
        Err("Cannot invoke the owned conversation copy action.".into())
    } else {
        Ok(())
    }
}

fn check_translation_format(editor: HWND, pid: u32) -> Result<(), String> {
    require_effect(editor, pid, "bold中文", BOLD)?;
    require_effect(editor, pid, "italic", ITALIC)?;
    require_effect(editor, pid, "removed", STRIKEOUT)?;
    require_monospace(editor, pid, "value")?;
    require_monospace(editor, pid, "let icon")?;
    let heading = native_format(editor, pid, "Clear translation")?;
    let body = native_format(editor, pid, "with")?;
    if heading.height <= body.height || body.effects & BOLD != 0 {
        return Err(
            "Markdown headings and ordinary body text did not retain distinct native formats."
                .into(),
        );
    }
    Ok(())
}

pub(super) fn try_serve(
    body: &serde_json::Value,
    stream: &mut TcpStream,
    requests: &Mutex<Vec<String>>,
    gate: &AtomicBool,
    answer_gate: &AtomicBool,
    stop: &AtomicBool,
) -> Result<bool, String> {
    let messages = body["messages"]
        .as_array()
        .ok_or("The Markdown fixture has no messages.")?;
    let question = messages
        .last()
        .and_then(|message| message["content"].as_str());
    let is_translation = messages
        .get(1)
        .and_then(|message| message["content"].as_str())
        == Some("markdown");
    if !is_translation && !matches!(question, Some(QUESTION_MARKDOWN | POLISH_PROMPT)) {
        return Ok(false);
    }
    let (label, first, last, waiting) = if is_translation {
        if messages.len() != 2 || messages[0]["role"] != "system" || messages[1]["role"] != "user" {
            return Err(
                "The Markdown translation did not use the normal two-message request.".into(),
            );
        }
        (REQUESTS[0], TRANSLATION_FIRST, TRANSLATION_TAIL, Some(gate))
    } else {
        let context = messages
            .get(1)
            .and_then(|message| message["content"].as_str())
            .ok_or("The Markdown question reference is missing.")?;
        let context: serde_json::Value =
            serde_json::from_str(context).map_err(|_| "The Markdown reference is not JSON.")?;
        if context
            != serde_json::json!({"kind":"selection_context", "source_text":"markdown", "translation":translation()})
        {
            return Err("The Markdown question did not preserve the complete raw current translation as context.".into());
        }
        if question == Some(QUESTION_MARKDOWN) {
            if messages.len() != 3 {
                return Err(
                    "The first Markdown question unexpectedly inherited old selection history."
                        .into(),
                );
            }
            (REQUESTS[1], ANSWER_MARKDOWN, "", Some(answer_gate))
        } else {
            if messages.len() != 5
                || messages[2]["role"] != "user"
                || messages[2]["content"] != QUESTION_MARKDOWN
                || messages[3]["role"] != "assistant"
                || messages[3]["content"] != ANSWER_MARKDOWN
                || messages[4]["role"] != "user"
                || messages[4]["content"] != POLISH_PROMPT
            {
                return Err("Polish did not use the exact current Markdown context, successful history and review prompt.".into());
            }
            (REQUESTS[2], POLISH_FIRST, POLISH_TAIL, Some(gate))
        }
    };
    requests
        .lock()
        .map_err(|_| "The Markdown request lock failed.")?
        .push(label.to_owned());
    let tail = match begin_sse_response(stream, first, last) {
        Ok(tail) => tail,
        Err(_) => return Ok(true),
    };
    let started = Instant::now();
    while waiting.is_some_and(|gate| !gate.load(Ordering::Acquire))
        && !stop.load(Ordering::Acquire)
        && started.elapsed() < Duration::from_secs(12)
    {
        thread::sleep(Duration::from_millis(10));
    }
    let _ = stream.write_all(&tail).and_then(|_| stream.flush());
    Ok(true)
}
