//! CommonMark/GFM text rendered into a small, native RichEdit document.
//!
//! No HTML, image, RTF field, embedded object, or executable link is emitted.
//! Markdown remains the source of truth in the application; `text` is the
//! readable copy representation, with LF paragraphs and tab-separated cells.
use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use std::fmt::Write;

const MAX_NESTING: usize = 128;
const MAX_TABLE_COLUMNS: usize = 32;
const MAX_MARKDOWN_BYTES: usize = 256 * 1024;
const MAX_RTF_BYTES: usize = 8 * 1024 * 1024;
const MAX_VISIBLE_URL_CHARS: usize = 384;

/// RGB24 colors and logical typography. RTF uses points/twips, not screen pixels,
/// so the native reader can lay out crisp text at its current monitor's DPI.
#[derive(Clone, Copy, Debug)]
pub struct RenderStyle {
    pub text: u32,
    pub background: u32,
    pub muted: u32,
    pub accent: u32,
    pub code_background: u32,
    pub body_font: &'static str,
    pub mono_font: &'static str,
    pub body_size_half_points: u16,
    /// Available text width; table columns divide this width equally.
    pub width_twips: i32,
}

impl Default for RenderStyle {
    fn default() -> Self {
        Self {
            text: 0xEEF6FF,
            background: 0x0C172A,
            muted: 0xA5B8D2,
            accent: 0x79B9FF,
            code_background: 0x1B2E49,
            body_font: "Segoe UI",
            mono_font: "Consolas",
            body_size_half_points: 24,
            width_twips: 4800,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Rendered {
    /// ASCII RTF without a terminating NUL, suitable for SF_RTF EM_STREAMIN.
    pub rtf: Vec<u8>,
    /// Readable text, with no formatting delimiters or invisible link targets.
    /// RichEdit APIs may expose their own CR/CRLF paragraph/cell separators.
    pub text: String,
}

/// Render a complete response or its current streaming prefix. The CommonMark
/// parser handles unfinished fences/delimiters; this event consumer is iterative
/// and bounds its own formatting stack. Its output grows linearly with input.
/// Documents above 256 KiB, tables wider than 32 columns, or unusually expansive
/// markup retain their full escaped source as plain text instead of truncating
/// content or building an oversized native document.
pub fn render(markdown: &str, style: &RenderStyle) -> Rendered {
    // Sparse, extremely wide GFM tables expand every short row to the header's
    // column count. Show those documents literally before building that tree.
    let wide_table = markdown.lines().any(|line| {
        line.bytes()
            .filter(|byte| *byte == b'|')
            .take(MAX_TABLE_COLUMNS + 2)
            .count()
            > MAX_TABLE_COLUMNS + 1
    });
    if markdown.len() > MAX_MARKDOWN_BYTES || wide_table {
        return literal(markdown, style);
    }
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES;
    let budget = markdown
        .len()
        .saturating_mul(128)
        .saturating_add(4096)
        .min(MAX_RTF_BYTES);
    let mut renderer = Renderer::new(style, markdown.len());
    for event in Parser::new_ext(markdown, options) {
        renderer.event(event);
        if renderer.rtf.len() > budget {
            // Preserve every source character rather than truncating a model's
            // answer or allocating a huge native rich-text document.
            drop(renderer);
            return literal(markdown, style);
        }
    }
    renderer.finish()
}

fn literal(markdown: &str, style: &RenderStyle) -> Rendered {
    let mut renderer = Renderer::new(style, markdown.len());
    renderer.begin_paragraph();
    renderer.append(markdown);
    renderer.finish()
}

/// Readable copy representation using the same Markdown rules as the native
/// reader. It never returns hidden targets or executes content.
pub fn plain_text(markdown: &str) -> String {
    render(markdown, &RenderStyle::default()).text
}

/// Close only an unfinished top-level fenced block before appending another
/// presentation section. CommonMark implicitly closes it at EOF; concatenating
/// a later exchange would otherwise turn that exchange into code. The model's
/// original response/history must remain unchanged; call this on display text.
/// Container fences need no added delimiter: an unindented next section closes
/// their quote/list/footnote container, and a new root fence would be incorrect.
pub fn close_unfinished_fence(output: &mut String) {
    // Larger documents are displayed literally by render(), so markup cannot
    // consume a later exchange and scanning them again has no benefit.
    if output.len() > MAX_MARKDOWN_BYTES {
        return;
    }
    let mut depth = 0usize;
    let mut candidate = None;
    for (event, range) in Parser::new_ext(output, Options::ENABLE_FOOTNOTES).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                if depth == 0
                    && matches!(tag, Tag::CodeBlock(CodeBlockKind::Fenced(_)))
                    && range.end == output.len()
                {
                    candidate = Some((range, 0usize));
                }
                depth += 1;
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Text(_) if depth == 1 => {
                if let Some((_, text_end)) = &mut candidate {
                    *text_end = (*text_end).max(range.end);
                }
            }
            _ => {}
        }
    }
    let Some((range, text_end)) = candidate else {
        return;
    };
    let block_start = range.start;
    let block = &output[range];
    let mut lines = block.lines();
    let Some(opening) = lines.next() else {
        return;
    };
    let opening = opening.trim_start_matches(' ');
    let Some(marker @ (b'`' | b'~')) = opening.as_bytes().first().copied() else {
        return;
    };
    let count = opening.bytes().take_while(|byte| *byte == marker).count();
    if count < 3 {
        return;
    }
    let closed = lines.last().is_some_and(|line| {
        let line = line.trim_end_matches('\r');
        let spaces = line.bytes().take_while(|byte| *byte == b' ').count();
        let line = &line[spaces..];
        let markers = line.bytes().take_while(|byte| *byte == marker).count();
        spaces <= 3
            && markers >= count
            // Match the parser, including line endings/whitespace it treats as
            // code rather than an actual closing delimiter.
            && text_end <= block_start + block.rfind(line).unwrap_or(block.len())
            && line[markers..]
                .bytes()
                .all(|byte| matches!(byte, b' ' | b'\t'))
    });
    if !closed {
        if !output.ends_with('\n') {
            output.push('\n');
        }
        output.extend(std::iter::repeat_n(marker as char, count));
    }
}

#[derive(Default)]
struct List {
    next: Option<u64>,
    first_paragraph: bool,
}

enum Frame<'a> {
    Other,
    Group,
    Link {
        destination: pulldown_cmark::CowStr<'a>,
        label_start: usize,
    },
}

struct Table {
    alignments: Vec<Alignment>,
    column: usize,
    heading: bool,
}

struct Renderer<'a> {
    rtf: String,
    text: String,
    frames: Vec<Frame<'a>>,
    overflow: usize,
    lists: Vec<List>,
    quote_depth: usize,
    footnote_prefix: bool,
    pending_paragraph: bool,
    table: Option<Table>,
    after_table_row: bool,
    last_was_cr: bool,
    size: u16,
    width: i32,
}

impl<'a> Renderer<'a> {
    fn new(style: &RenderStyle, input_bytes: usize) -> Self {
        let mut rtf = String::with_capacity(input_bytes.saturating_mul(2).saturating_add(512));
        let size = style.body_size_half_points.clamp(16, 72);
        let width = style.width_twips.clamp(720, 43_200);
        rtf.push_str("{\\rtf1\\ansi\\ansicpg1252\\deff0\\uc1\n{\\fonttbl{\\f0\\fnil\\fcharset0 ");
        font_name(&mut rtf, style.body_font);
        rtf.push_str(";}{\\f1\\fmodern\\fcharset0 ");
        font_name(&mut rtf, style.mono_font);
        rtf.push_str(";}}\n{\\colortbl;");
        for color in [
            style.text,
            style.muted,
            style.accent,
            style.code_background,
            style.background,
            border_color(style.text, style.background),
        ] {
            let _ = write!(
                rtf,
                "\\red{}\\green{}\\blue{};",
                (color >> 16) & 255,
                (color >> 8) & 255,
                color & 255
            );
        }
        let _ = write!(
            rtf,
            "}}\n\\viewkind4\\viewscale100\\fet0\\lang1033\\langfe2052\\paperw{}\\margl0\\margr0\\margt0\\margb0 ",
            width
        );
        Self {
            rtf,
            text: String::with_capacity(input_bytes),
            frames: Vec::new(),
            overflow: 0,
            lists: Vec::new(),
            quote_depth: 0,
            footnote_prefix: false,
            pending_paragraph: false,
            table: None,
            after_table_row: false,
            last_was_cr: false,
            size,
            width,
        }
    }

    fn event(&mut self, event: Event<'a>) {
        match event {
            Event::Start(tag) => {
                if self.overflow != 0 || self.frames.len() >= MAX_NESTING {
                    self.overflow += 1;
                } else {
                    let frame = self.start(tag);
                    self.frames.push(frame);
                }
            }
            Event::End(tag) => {
                if self.overflow != 0 {
                    self.overflow -= 1;
                } else if let Some(frame) = self.frames.pop() {
                    self.end(tag, frame);
                }
            }
            Event::Text(value) | Event::Html(value) | Event::InlineHtml(value) => {
                self.append(&value)
            }
            Event::Code(value) => {
                self.flush_paragraph();
                let _ = write!(
                    self.rtf,
                    "{{\\f1\\fs{}\\highlight4 ",
                    self.size.saturating_sub(2)
                );
                self.append(&value);
                self.rtf.push('}');
            }
            Event::SoftBreak | Event::HardBreak => self.append("\n"),
            Event::Rule => {
                self.begin_paragraph();
                self.rtf.push_str("\\cf2 ");
                self.append("────────");
                self.pending_paragraph = true;
            }
            Event::TaskListMarker(checked) => {
                if self.text.ends_with("• ") && self.rtf.ends_with("\\u8226? ") {
                    self.text.truncate(self.text.len() - "• ".len());
                    self.rtf.truncate(self.rtf.len() - "\\u8226? ".len());
                }
                self.append(if checked { "☑ " } else { "☐ " });
            }
            Event::FootnoteReference(label) => {
                self.flush_paragraph();
                let _ = write!(self.rtf, "{{\\super\\fs{} ", self.size.saturating_sub(4));
                self.append("[");
                self.append(&label);
                self.append("]");
                self.rtf.push('}');
            }
            Event::InlineMath(value) | Event::DisplayMath(value) => self.append(&value),
        }
    }

    fn start(&mut self, tag: Tag<'a>) -> Frame<'a> {
        match tag {
            Tag::Paragraph => {
                let list_prefix = self.lists.last_mut().is_some_and(|list| {
                    let first = list.first_paragraph;
                    list.first_paragraph = false;
                    first
                });
                if self.footnote_prefix && !self.pending_paragraph {
                    self.footnote_prefix = false;
                } else if !list_prefix || self.pending_paragraph {
                    self.begin_paragraph();
                }
            }
            Tag::Heading { level, .. } => {
                self.begin_paragraph();
                let extra = match level {
                    HeadingLevel::H1 => 12,
                    HeadingLevel::H2 => 8,
                    HeadingLevel::H3 => 4,
                    _ => 2,
                };
                let _ = write!(self.rtf, "\\b\\fs{}\\sb160\\sa100 ", self.size + extra);
            }
            Tag::BlockQuote(_) => {
                self.quote_depth += 1;
            }
            Tag::CodeBlock(_) => {
                self.begin_paragraph();
                let indent = self.indent() + 120;
                let _ = write!(
                    self.rtf,
                    "\\f1\\fs{}\\highlight4\\li{}\\ri120\\sb0\\sa0 ",
                    self.size.saturating_sub(2),
                    indent
                );
            }
            Tag::HtmlBlock => self.begin_paragraph(),
            Tag::List(next) => self.lists.push(List {
                next,
                first_paragraph: false,
            }),
            Tag::Item => {
                self.begin_paragraph();
                self.rtf.push_str("\\fi-180 ");
                let next = self.lists.last_mut().and_then(|list| {
                    list.first_paragraph = true;
                    let next = list.next;
                    if let Some(value) = list.next.as_mut() {
                        *value = value.saturating_add(1);
                    }
                    next
                });
                if let Some(number) = next {
                    self.append(&format!("{number}. "));
                } else {
                    self.append("• ");
                }
            }
            Tag::FootnoteDefinition(label) => {
                self.begin_paragraph();
                let _ = write!(self.rtf, "\\fs{}\\cf2 ", self.size.saturating_sub(2));
                self.append("[");
                self.append(&label);
                self.append("] ");
                self.footnote_prefix = true;
            }
            Tag::Table(alignments) => {
                self.flush_paragraph();
                self.table = Some(Table {
                    alignments,
                    column: 0,
                    heading: false,
                });
            }
            Tag::TableHead => self.start_row(true),
            Tag::TableRow => self.start_row(false),
            Tag::TableCell => self.start_cell(),
            Tag::Emphasis => return self.group("\\i "),
            Tag::Strong => return self.group("\\b "),
            Tag::Strikethrough => return self.group("\\strike "),
            Tag::Superscript => return self.group("\\super "),
            Tag::Subscript => return self.group("\\sub "),
            Tag::Link { dest_url, .. } => {
                self.flush_paragraph();
                self.rtf.push_str("{\\cf3\\ul ");
                return Frame::Link {
                    destination: dest_url,
                    label_start: self.text.len(),
                };
            }
            Tag::Image { .. } => self.append("[图片："),
            Tag::DefinitionList | Tag::DefinitionListTitle | Tag::DefinitionListDefinition => {
                self.begin_paragraph();
            }
            Tag::MetadataBlock(_) => {}
        }
        Frame::Other
    }

    fn end(&mut self, tag: TagEnd, frame: Frame<'a>) {
        match frame {
            Frame::Group => self.rtf.push('}'),
            Frame::Link {
                destination,
                label_start,
            } => {
                self.rtf.push('}');
                // A visible URL can be copied without hidden, executable fields.
                if self.text[label_start..].trim() != destination.as_ref()
                    && !destination.is_empty()
                {
                    self.append(" (");
                    let mut visible = String::new();
                    for (index, character) in destination
                        .chars()
                        .take(MAX_VISIBLE_URL_CHARS + 1)
                        .enumerate()
                    {
                        if index == MAX_VISIBLE_URL_CHARS {
                            visible.push('…');
                        } else {
                            visible.push(character);
                        }
                    }
                    self.append(&visible);
                    self.append(")");
                }
            }
            Frame::Other => match tag {
                TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::HtmlBlock | TagEnd::Item => {
                    self.pending_paragraph = true;
                }
                TagEnd::BlockQuote(_) => {
                    self.quote_depth = self.quote_depth.saturating_sub(1);
                    self.pending_paragraph = true;
                }
                TagEnd::CodeBlock => {
                    // CommonMark's code text includes the fence's final LF. Keep
                    // real inner blank lines, without adding an empty final line.
                    if self.text.ends_with('\n') && self.rtf.ends_with("\\sa0\\par ") {
                        self.text.pop();
                        self.rtf.truncate(self.rtf.len() - "\\sa0\\par ".len());
                    }
                    self.pending_paragraph = true;
                }
                TagEnd::List(_) => {
                    self.lists.pop();
                    self.pending_paragraph = true;
                }
                TagEnd::FootnoteDefinition => {
                    self.footnote_prefix = false;
                    self.pending_paragraph = true;
                }
                TagEnd::TableCell => {
                    self.rtf.push_str("\\cell ");
                    if let Some(table) = &mut self.table {
                        table.column += 1;
                        if table.column < table.alignments.len() {
                            self.text.push('\t');
                        }
                    }
                }
                TagEnd::TableHead | TagEnd::TableRow => {
                    self.rtf.push_str("\\row\n");
                    self.text.push('\n');
                    self.after_table_row = true;
                }
                TagEnd::Table => {
                    self.table = None;
                    self.pending_paragraph = true;
                }
                TagEnd::Image => self.append("]"),
                _ => {}
            },
        }
    }

    fn group(&mut self, controls: &str) -> Frame<'a> {
        self.flush_paragraph();
        self.rtf.push('{');
        self.rtf.push_str(controls);
        Frame::Group
    }

    fn indent(&self) -> usize {
        (self.lists.len().saturating_mul(240) + self.quote_depth.saturating_mul(240))
            .min(self.width as usize / 2)
    }

    fn begin_paragraph(&mut self) {
        self.flush_paragraph();
        let indent = self.indent();
        let _ = write!(
            self.rtf,
            "\\pard\\plain\\f0\\fs{}\\cf{}\\highlight0\\li{}\\ri0\\fi0\\sb0\\sa100\\sl{}\\slmult0 ",
            self.size,
            if self.quote_depth > 0 { 2 } else { 1 },
            indent,
            self.size as usize * 13
        );
        if self.quote_depth > 0 {
            self.rtf
                .push_str("\\brdrl\\brdrs\\brdrw12\\brdrcf3\\brsp80 ");
        }
    }

    fn flush_paragraph(&mut self) {
        if self.pending_paragraph {
            if !self.text.is_empty() {
                if !self.after_table_row {
                    self.rtf.push_str("\\sa100\\par\n");
                }
                if !self.text.ends_with('\n') {
                    self.text.push('\n');
                }
            }
            self.pending_paragraph = false;
            self.last_was_cr = false;
        }
        self.after_table_row = false;
    }

    fn append(&mut self, text: &str) {
        self.flush_paragraph();
        for character in text.chars() {
            if character == '\n' && self.last_was_cr {
                self.last_was_cr = false;
                continue;
            }
            self.last_was_cr = character == '\r';
            let character = match character {
                '\r' => '\n',
                '\n' | '\t' => character,
                c if c.is_control() => '\u{fffd}',
                c => c,
            };
            self.text.push(character);
            rtf_character(&mut self.rtf, character);
        }
    }

    fn start_row(&mut self, heading: bool) {
        let Some(table) = self.table.as_mut() else {
            return;
        };
        table.heading = heading;
        table.column = 0;
        let columns = table.alignments.len().max(1);
        self.rtf.push_str("\\trowd\\trgaph80\\trleft0 ");
        for column in 1..=columns {
            let edge = i64::from(self.width) * column as i64 / columns as i64;
            let _ = write!(
                self.rtf,
                "\\clvertalt\\clcbpat{}\\clbrdrb\\brdrs\\brdrw6\\brdrcf6\\clbrdrr\\brdrs\\brdrw6\\brdrcf6\\cellx{} ",
                if heading { 4 } else { 5 },
                edge
            );
        }
        self.after_table_row = false;
    }

    fn start_cell(&mut self) {
        let Some(table) = self.table.as_ref() else {
            return;
        };
        let align = match table.alignments.get(table.column) {
            Some(Alignment::Right) => "\\qr",
            Some(Alignment::Center) => "\\qc",
            _ => "\\ql",
        };
        let _ = write!(
            self.rtf,
            "\\pard\\plain\\intbl\\f0\\fs{}\\cf1\\li0\\ri0\\fi0\\sb0\\sa40{}{} ",
            self.size,
            align,
            if table.heading { "\\b" } else { "" }
        );
    }

    fn finish(mut self) -> Rendered {
        while let Some(frame) = self.frames.pop() {
            if matches!(frame, Frame::Group | Frame::Link { .. }) {
                self.rtf.push('}');
            }
        }
        self.rtf.push('}');
        // A table contributes row separators during rendering; the final row
        // doesn't need an extra copy-only LF after the last visible content.
        if self.after_table_row && self.text.ends_with('\n') {
            self.text.pop();
        }
        Rendered {
            rtf: self.rtf.into_bytes(),
            text: self.text,
        }
    }
}

fn font_name(output: &mut String, value: &str) {
    for character in value.chars() {
        if character == ';' {
            output.push_str("\\'3b");
        } else if character.is_control() {
            output.push(' ');
        } else {
            rtf_character(output, character);
        }
    }
}

fn rtf_character(output: &mut String, value: char) {
    match value {
        '\\' | '{' | '}' => {
            output.push('\\');
            output.push(value);
        }
        '\n' => output.push_str("\\sa0\\par "),
        '\t' => output.push_str("\\tab "),
        value if value.is_ascii() => output.push(value),
        value => {
            for code_unit in value.encode_utf16(&mut [0u16; 2]) {
                let _ = write!(output, "\\u{}?", *code_unit as i16);
            }
        }
    }
}

fn border_color(foreground: u32, background: u32) -> u32 {
    let blend =
        |shift: u32| (((foreground >> shift) & 255) + ((background >> shift) & 255) * 4) / 5;
    (blend(16) << 16) | (blend(8) << 8) | blend(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(markdown: &str) -> Rendered {
        render(markdown, &RenderStyle::default())
    }

    fn controls(rtf: &[u8]) -> (Vec<String>, usize) {
        assert!(rtf.is_ascii());
        let mut words = Vec::new();
        let mut index = 0;
        let mut depth = 0usize;
        let mut maximum = 0;
        while index < rtf.len() {
            match rtf[index] {
                b'{' => {
                    depth += 1;
                    maximum = maximum.max(depth);
                }
                b'}' => {
                    assert!(depth > 0, "unmatched RTF group");
                    depth -= 1;
                }
                b'\\' => {
                    index += 1;
                    if index < rtf.len() && rtf[index].is_ascii_alphabetic() {
                        let start = index;
                        while index < rtf.len() && rtf[index].is_ascii_alphabetic() {
                            index += 1;
                        }
                        words.push(String::from_utf8(rtf[start..index].to_vec()).unwrap());
                        continue;
                    }
                }
                _ => {}
            }
            index += 1;
        }
        assert_eq!(depth, 0);
        (words, maximum)
    }

    #[test]
    fn formats_mixed_chinese_and_english_with_real_block_and_inline_styles() {
        let value = document("# 标题 😀\n\n正文 **加粗**、*emphasis*、~~删除~~ 和 `a < b`。\n\n> 一段引用\n\n3. first\n4. second\n\n- [x] 完成\n- [ ] 等待\n\n```rust\nlet x = 1;\nprintln!(\"中文\");\n```\n");
        assert_eq!(value.text, "标题 😀\n正文 加粗、emphasis、删除 和 a < b。\n一段引用\n3. first\n4. second\n☑ 完成\n☐ 等待\nlet x = 1;\nprintln!(\"中文\");");
        let (commands, _) = controls(&value.rtf);
        for expected in ["b", "i", "strike", "f", "highlight", "brdrl"] {
            assert!(commands.iter().any(|command| command == expected));
        }
        let rtf = String::from_utf8(value.rtf).unwrap();
        assert!(rtf.contains("\\u-10179?\\u-8704?"));
        assert!(rtf.contains("\\fs36"));
    }

    #[test]
    fn table_uses_native_cells_and_preserves_readable_tab_separated_copy() {
        let value = render(
            "| 名称 | Value |\n| :--- | ---: |\n| **甲** | `42` |\n| 乙 | 7 |",
            &RenderStyle {
                width_twips: 3600,
                ..RenderStyle::default()
            },
        );
        assert_eq!(value.text, "名称\tValue\n甲\t42\n乙\t7");
        let rtf = String::from_utf8(value.rtf.clone()).unwrap();
        assert_eq!(rtf.matches("\\trowd").count(), 3);
        assert_eq!(rtf.matches("\\cell ").count(), 6);
        assert!(rtf.contains("\\cellx1800"));
        assert!(rtf.contains("\\cellx3600"));
        assert!(rtf.contains("\\qr"));
        controls(&value.rtf);
    }

    #[test]
    fn untrusted_html_images_urls_and_rtf_commands_stay_inert_visible_text() {
        let value = render("<script>alert(1)</script>\n\n[link](javascript:alert%281%29)\n\n![外部图片](https://example.com/private.png)\n\n`{\\object\\objdata deadbeef} {\\field HYPERLINK}`", &RenderStyle { body_font: "Segoe UI;{\\field evil}", ..RenderStyle::default() });
        assert!(value.text.contains("<script>alert(1)</script>"));
        assert!(value.text.contains("link (javascript:alert%281%29)"));
        assert!(value.text.contains("[图片：外部图片]"));
        assert!(!value.text.contains("private.png"));
        assert!(value.text.contains(r"{\object\objdata deadbeef}"));
        let (commands, _) = controls(&value.rtf);
        for forbidden in ["field", "fldinst", "object", "objdata", "pict", "htmltag"] {
            assert!(
                !commands.iter().any(|command| command == forbidden),
                "executable RTF control {forbidden}"
            );
        }
    }

    #[test]
    fn every_utf8_stream_prefix_is_valid_and_keeps_unicode_and_unfinished_code() {
        let source = "## 中文 😀\n\n**强调** 和 [链接](https://example.com)\n\n```rs\nlet café = \"你好\";\n```\n\n|A|B|\n|-|-|\n|1|2|";
        for boundary in source
            .char_indices()
            .map(|(index, _)| index)
            .chain([source.len()])
        {
            let value = document(&source[..boundary]);
            controls(&value.rtf);
            assert!(value.rtf.len() <= source[..boundary].len() * 40 + 2048);
        }
        assert_eq!(document("```text\n正在流式输出😀").text, "正在流式输出😀");
        assert_eq!(document("unfinished **bold").text, "unfinished **bold");
    }

    #[test]
    fn deeply_nested_input_has_bounded_renderer_groups_without_recursion() {
        let source = format!("{}text{}", "*a ".repeat(400), "*".repeat(400));
        let rendered = document(&source);
        let (_, maximum) = controls(&rendered.rtf);
        assert!(maximum <= MAX_NESTING + 4);
        assert!(rendered.text.contains("text"));
        let nested_quotes = format!("{}内容", "> ".repeat(512));
        controls(&document(&nested_quotes).rtf);
    }

    #[test]
    fn ordinary_text_escapes_controls_and_does_not_duplicate_autolink_urls() {
        assert_eq!(document("A\r\nB\r\n\r\nC").text, "A\nB\nC");
        assert_eq!(
            document("<https://example.com/path>").text,
            "https://example.com/path"
        );
        assert_eq!(document("正文\0尾部").text, "正文�尾部");
        assert_eq!(
            document(r"\*literal\* \\ \{safe\}").text,
            "*literal* \\ {safe}"
        );
    }

    #[test]
    fn shared_long_targets_and_sparse_wide_tables_cannot_expand_unbounded_rtf() {
        let destination = format!("https://example.com/{}", "a".repeat(10_000));
        let source = format!("{}\n\n[x]: {}", "[label][x] ".repeat(100), destination);
        let rendered = document(&source);
        assert!(rendered.rtf.len() < source.len() * 12);
        assert!(rendered.text.contains('…'));
        assert!(!rendered.text.contains(&destination));
        controls(&rendered.rtf);

        let table = format!(
            "|{}\n|{}\n{}",
            "head|".repeat(80),
            "---|".repeat(80),
            "|x|\n".repeat(1000)
        );
        let literal = document(&table);
        assert_eq!(literal.text, table);
        assert!(literal.rtf.len() < table.len() * 4);
        let long = format!("**{}**", "中".repeat(MAX_MARKDOWN_BYTES / 3 + 1));
        assert_eq!(plain_text(&long), long);
    }

    #[test]
    fn loose_paragraphs_after_nested_lists_return_to_the_outer_indent() {
        let value = document("- outer\n  - inner\n\n  outer again\n\nnext");
        assert_eq!(value.text, "• outer\n• inner\nouter again\nnext");
        let rtf = String::from_utf8(value.rtf).unwrap();
        let again = rtf.find("outer again").unwrap();
        assert!(rtf[..again]
            .rsplit("\\pard")
            .next()
            .unwrap()
            .contains("\\li240"));
    }

    #[test]
    fn appended_exchange_stays_outside_unfinished_fences_of_each_length_and_kind() {
        for initial in [
            "```rust\nlet n = 1;",
            "````markdown\n```\nnot closed by three ticks",
            "~~~~text\r\n中文 😀\r\n",
            "  ```info\nbody\n",
            "~~~text\r\ncode\r\n  ~~~~\t\r\n",
            "```",
        ] {
            let mut display = initial.to_owned();
            close_unfinished_fence(&mut display);
            let closed = display.clone();
            close_unfinished_fence(&mut display);
            assert_eq!(display, closed, "closure must be idempotent");
            display.push_str("\n\n**你：** literal question\n\n**轻译：** new **answer**");
            let visible = plain_text(&display);
            assert!(visible.contains("你： literal question\n轻译： new answer"));
            assert!(!visible.contains("**你：**"));
        }
    }

    #[test]
    fn existing_closures_and_container_fences_do_not_open_new_root_code_blocks() {
        for source in [
            "```rust\ncode\n```",
            "````info\ncode\n`````  ",
            "~~~text\r\ncode\r\n  ~~~~  \r\n",
            "```\ncode\n```\n\nnormal text",
            "> ```rust\n> code",
            "- ```rust\n  code",
            "[^x]:\n    ```rust\n    code\n\nref[^x]",
        ] {
            let mut display = source.to_owned();
            close_unfinished_fence(&mut display);
            assert_eq!(display, source);
            display.push_str("\n\n**Next exchange**");
            assert!(
                !plain_text(&display).contains("**Next exchange**"),
                "source: {source:?}"
            );
        }
    }

    #[test]
    fn multiline_text_and_code_use_copyable_paragraphs_without_soft_line_controls() {
        let source = "第一行 😀\nsecond line\n\n```rs\nlet 中文 = 1;\nprintln!(\"😀\");\n```";
        let value = document(source);
        assert_eq!(
            value.text,
            "第一行 😀\nsecond line\nlet 中文 = 1;\nprintln!(\"😀\");"
        );
        let (commands, _) = controls(&value.rtf);
        assert!(commands.iter().any(|control| control == "par"));
        assert!(!commands.iter().any(|control| control == "line"));
        let rtf = String::from_utf8(value.rtf).unwrap();
        assert!(rtf.contains("\\sa0\\par "));
        assert!(rtf.contains("\\ri120\\sb0\\sa0 "));
        assert!(rtf.contains("\\u-10179?\\u-8704?"));
        assert_eq!(plain_text(source), value.text);
    }
}
