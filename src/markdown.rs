//! Rich text with no visible markup: formatting lives in text tags while writing
//! and becomes markdown only when an entry is stored or exported.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::pango;
use gtk::prelude::*;

pub const INLINE: [&str; 4] = ["bold", "italic", "strike", "code"];
pub const BLOCK: [&str; 3] = ["h1", "h2", "quote"];
const BULLET: &str = "• ";
const HR: &str = "────────────";
/// How a private section is marked in stored markdown, and what the writer sees.
pub const PRIVATE_MD: &str = "<!--private-->";
const PRIVATE_TEXT: &str = "private";

fn marker(tag: &str) -> &'static str {
    match tag {
        "bold" => "**",
        "italic" => "*",
        "strike" => "~~",
        _ => "`",
    }
}

fn block_prefix(tag: &str) -> &'static str {
    match tag {
        "h1" => "# ",
        "h2" => "## ",
        _ => "> ",
    }
}

fn canon(name: &str) -> Option<&'static str> {
    INLINE.iter().chain(BLOCK.iter()).copied().find(|t| *t == name)
}

pub fn setup_tags(buffer: &gtk::TextBuffer) {
    buffer.create_tag(Some("bold"), &[("weight", &700i32)]);
    buffer.create_tag(Some("italic"), &[("style", &pango::Style::Italic)]);
    buffer.create_tag(Some("strike"), &[("strikethrough", &true)]);
    buffer.create_tag(Some("code"), &[("family", &"monospace"), ("scale", &0.88f64)]);
    buffer.create_tag(Some("h1"), &[("scale", &1.3f64), ("weight", &700i32)]);
    buffer.create_tag(Some("h2"), &[("scale", &1.12f64), ("weight", &700i32)]);
    buffer.create_tag(Some("privmark"), &[("scale", &0.8f64), ("family", &"monospace")]);
    buffer.create_tag(Some("veil"), &[("invisible", &true)]);
    buffer.create_tag(Some("hr"), &[("justification", &gtk::Justification::Center)]);
    buffer.create_tag(Some("quote"), &[("left-margin", &18i32), ("style", &pango::Style::Italic)]);
}

fn names_at(iter: &gtk::TextIter) -> Vec<&'static str> {
    iter.tags()
        .iter()
        .filter_map(|t| t.name())
        .filter_map(|n| canon(n.as_str()))
        .collect()
}

fn is_hr(iter: &gtk::TextIter) -> bool {
    iter.tags().iter().any(|t| t.name().is_some_and(|n| n == "hr"))
}

fn line_has_tag(start: &gtk::TextIter, end: &gtk::TextIter, name: &str) -> bool {
    let mut it = *start;
    while it < *end {
        if it.tags().iter().any(|t| t.name().is_some_and(|n| n == name)) {
            return true;
        }
        it.forward_char();
    }
    false
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '\\' | '*' | '`' | '~') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn close_from(out: &mut String, stack: &mut Vec<&'static str>, pos: usize) {
    let trimmed = out.trim_end().len();
    let trail = out.split_off(trimmed);
    for t in stack.drain(pos..).rev() {
        out.push_str(marker(t));
    }
    out.push_str(&trail);
}

fn emit_run(out: &mut String, stack: &mut Vec<&'static str>, set: &[&'static str], text: &str) {
    if let Some(pos) = stack.iter().position(|t| !set.contains(t)) {
        close_from(out, stack, pos);
    }
    if text.trim().is_empty() {
        out.push_str(text);
        return;
    }
    let lead = text.len() - text.trim_start().len();
    out.push_str(&text[..lead]);
    for t in INLINE {
        if set.contains(&t) && !stack.contains(&t) {
            out.push_str(marker(t));
            stack.push(t);
        }
    }
    out.push_str(&escape(&text[lead..]));
}

pub fn to_markdown(buffer: &gtk::TextBuffer) -> String {
    let mut lines = Vec::new();
    for i in 0..buffer.line_count() {
        let Some(start) = buffer.iter_at_line(i) else { continue };
        let mut end = start;
        if !end.ends_line() {
            end.forward_to_line_end();
        }
        let mut line = String::new();
        if line_has_tag(&start, &end, "privmark") {
            lines.push(PRIVATE_MD.to_string());
            continue;
        }
        if start != end && is_hr(&start) {
            lines.push("***".to_string());
            continue;
        }
        if start != end {
            if let Some(b) = names_at(&start).into_iter().find(|t| BLOCK.contains(t)) {
                line.push_str(block_prefix(b));
            }
        }
        let mut cur = start;
        if buffer.text(&start, &end, true).starts_with(BULLET) {
            line.push_str("- ");
            cur.forward_chars(2);
        }
        let mut stack: Vec<&'static str> = Vec::new();
        while cur < end {
            let mut next = cur;
            if !next.forward_to_tag_toggle(None::<&gtk::TextTag>) || next > end || next == cur {
                next = end;
            }
            let set: Vec<&'static str> = names_at(&cur).into_iter().filter(|t| INLINE.contains(t)).collect();
            emit_run(&mut line, &mut stack, &set, buffer.text(&cur, &next, true).as_str());
            cur = next;
        }
        close_from(&mut line, &mut stack, 0);
        lines.push(line);
    }
    lines.join("\n")
}

fn parse_inline(text: &str) -> Vec<(String, Vec<&'static str>)> {
    let chars: Vec<char> = text.chars().collect();
    let mut runs = Vec::new();
    let mut cur = String::new();
    let mut on = [false; 4];
    let flush = |cur: &mut String, on: &[bool; 4], runs: &mut Vec<(String, Vec<&'static str>)>| {
        if !cur.is_empty() {
            let tags = INLINE.iter().zip(on).filter(|(_, o)| **o).map(|(t, _)| *t).collect();
            runs.push((std::mem::take(cur), tags));
        }
    };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        let toggle = if on[3] {
            (c == '`').then_some((3, 1))
        } else if c == '\\' && next.is_some() {
            cur.push(next.unwrap());
            i += 2;
            continue;
        } else if c == '*' && next == Some('*') {
            Some((0, 2))
        } else if c == '*' {
            Some((1, 1))
        } else if c == '~' && next == Some('~') {
            Some((2, 2))
        } else if c == '`' {
            Some((3, 1))
        } else {
            None
        };
        match toggle {
            Some((idx, len)) => {
                flush(&mut cur, &on, &mut runs);
                on[idx] = !on[idx];
                i += len;
            }
            None => {
                cur.push(c);
                i += 1;
            }
        }
    }
    flush(&mut cur, &on, &mut runs);
    runs
}

pub fn load_markdown(buffer: &gtk::TextBuffer, md: &str) {
    buffer.begin_irreversible_action();
    buffer.set_text("");
    let mut iter = buffer.end_iter();
    for (n, raw) in md.split('\n').enumerate() {
        if n > 0 {
            buffer.insert(&mut iter, "\n");
        }
        if raw == PRIVATE_MD {
            buffer.insert_with_tags_by_name(&mut iter, PRIVATE_TEXT, &["privmark"]);
            continue;
        }
        if raw == "***" {
            buffer.insert_with_tags_by_name(&mut iter, HR, &["hr"]);
            continue;
        }
        let mut line = raw;
        let mut block = None;
        for b in BLOCK {
            if let Some(rest) = line.strip_prefix(block_prefix(b)) {
                block = Some(b);
                line = rest;
                break;
            }
        }
        if let Some(rest) = line.strip_prefix("- ") {
            match block {
                Some(b) => buffer.insert_with_tags_by_name(&mut iter, BULLET, &[b]),
                None => buffer.insert(&mut iter, BULLET),
            }
            line = rest;
        }
        for (text, mut tags) in parse_inline(line) {
            tags.extend(block);
            buffer.insert_with_tags_by_name(&mut iter, &text, &tags);
        }
    }
    buffer.end_irreversible_action();
}

/// Keyboard-driven formatting for one editable buffer.
pub struct Fmt {
    buffer: gtk::TextBuffer,
    /// Formatting chosen at a cursor position before anything is typed there.
    pending: RefCell<Option<(i32, Vec<&'static str>)>>,
    inserted: Cell<Option<(i32, i32)>>,
    raw: Cell<bool>,
}

impl Fmt {
    pub fn attach(buffer: &gtk::TextBuffer) -> Rc<Fmt> {
        let fmt = Rc::new(Fmt { buffer: buffer.clone(), pending: RefCell::new(None), inserted: Cell::new(None), raw: Cell::new(false) });
        let f = fmt.clone();
        buffer.connect_insert_text(move |_, iter, text| {
            f.inserted.set(Some((iter.offset(), text.chars().count() as i32)));
        });
        let f = fmt.clone();
        buffer.connect_changed(move |_| {
            if let Some((start, len)) = f.inserted.take() {
                f.style_insert(start, len);
            } else {
                f.pending.replace(None);
            }
        });
        fmt
    }

    fn cursor(&self) -> gtk::TextIter {
        self.buffer.iter_at_mark(&self.buffer.get_insert())
    }

    /// Tags that text typed at `at` should take on.
    fn inherited(&self, at: &gtk::TextIter, after: &gtk::TextIter) -> Vec<&'static str> {
        if at.starts_line() {
            if after.ends_line() {
                return Vec::new();
            }
            return names_at(after).into_iter().filter(|t| BLOCK.contains(t)).collect();
        }
        let mut prev = *at;
        prev.backward_char();
        names_at(&prev)
    }

    fn style_insert(&self, start: i32, len: i32) {
        if self.raw.get() {
            return;
        }
        let s = self.buffer.iter_at_offset(start);
        let e = self.buffer.iter_at_offset(start + len);
        let mut want = self.inherited(&s, &e);
        let mut pending = self.pending.borrow_mut();
        match pending.as_mut() {
            Some((offset, set)) if *offset == start => {
                want.retain(|t| BLOCK.contains(t));
                want.extend(set.iter().copied());
                *offset = start + len;
            }
            _ => *pending = None,
        }
        if self.buffer.text(&s, &e, true).contains('\n') {
            *pending = None;
        }
        drop(pending);
        self.buffer.remove_tag_by_name("hr", &s, &e);
        self.buffer.remove_tag_by_name("privmark", &s, &e);
        for t in INLINE.iter().chain(BLOCK.iter()) {
            if want.contains(t) {
                self.buffer.apply_tag_by_name(t, &s, &e);
            } else {
                self.buffer.remove_tag_by_name(t, &s, &e);
            }
        }
    }

    fn active_at_cursor(&self) -> (i32, Vec<&'static str>) {
        let cur = self.cursor();
        let offset = cur.offset();
        if let Some((o, set)) = self.pending.borrow().as_ref() {
            if *o == offset {
                return (offset, set.clone());
            }
        }
        (offset, self.inherited(&cur, &cur))
    }

    pub fn toggle_inline(&self, name: &'static str) {
        if let Some((s, e)) = self.buffer.selection_bounds() {
            let tag = self.buffer.tag_table().lookup(name).expect("tag exists");
            if s.has_tag(&tag) {
                self.buffer.remove_tag(&tag, &s, &e);
            } else {
                self.buffer.apply_tag(&tag, &s, &e);
            }
            return;
        }
        let (offset, mut set) = self.active_at_cursor();
        match set.iter().position(|t| *t == name) {
            Some(i) => {
                set.remove(i);
            }
            None => set.push(name),
        }
        self.pending.replace(Some((offset, set)));
    }

    fn selected_lines(&self) -> (i32, i32) {
        match self.buffer.selection_bounds() {
            Some((s, e)) => (s.line(), e.line()),
            None => {
                let l = self.cursor().line();
                (l, l)
            }
        }
    }

    fn line_bounds(&self, line: i32) -> Option<(gtk::TextIter, gtk::TextIter)> {
        let start = self.buffer.iter_at_line(line)?;
        let mut end = start;
        if !end.ends_line() {
            end.forward_to_line_end();
        }
        Some((start, end))
    }

    /// `None` returns the lines to body text.
    pub fn set_block(&self, name: Option<&'static str>) {
        let (first, last) = self.selected_lines();
        for line in first..=last {
            let Some((s, e)) = self.line_bounds(line) else { continue };
            let had = name.is_some_and(|n| s != e && names_at(&s).contains(&n));
            for b in BLOCK {
                self.buffer.remove_tag_by_name(b, &s, &e);
            }
            let apply = name.filter(|_| !had);
            if let Some(n) = apply {
                self.buffer.apply_tag_by_name(n, &s, &e);
            }
            if s == e && first == last {
                let (offset, mut set) = self.active_at_cursor();
                let was_on = name.is_some_and(|n| set.contains(&n));
                set.retain(|t| INLINE.contains(t));
                set.extend(apply.filter(|_| !was_on));
                self.pending.replace(Some((offset, set)));
            }
        }
    }

    /// Hides everything except the last `chars` characters (or shows it all again).
    pub fn veil(&self, chars: Option<usize>) {
        let (start, end) = self.buffer.bounds();
        self.buffer.remove_tag_by_name("veil", &start, &end);
        let Some(chars) = chars else { return };
        let cut = self.buffer.iter_at_offset((end.offset() - chars as i32).max(0));
        self.buffer.apply_tag_by_name("veil", &start, &cut);
    }

    /// Marks (or unmarks) the section the cursor is in as private. A section is
    /// the text between two dividers, or the whole entry when there are none.
    pub fn toggle_private_section(&self) {
        let lines = self.buffer.line_count();
        let is_rule = |l: i32| self.line_bounds(l).is_some_and(|(s, e)| s != e && is_hr(&s));
        let mut first = self.cursor().line();
        if is_rule(first) && first + 1 < lines {
            first += 1;
        } else {
            while first > 0 && !is_rule(first - 1) {
                first -= 1;
            }
        }
        let mut last = first;
        while last + 1 < lines && !is_rule(last + 1) {
            last += 1;
        }
        let marked = (first..=last)
            .find(|l| self.line_bounds(*l).is_some_and(|(s, e)| line_has_tag(&s, &e, "privmark")));
        match marked {
            Some(line) => {
                let Some((mut s, mut e)) = self.line_bounds(line) else { return };
                if !e.forward_char() {
                    s.backward_char();
                }
                self.buffer.delete(&mut s, &mut e);
            }
            None => {
                let Some(mut at) = self.buffer.iter_at_line(first) else { return };
                self.raw.set(true);
                self.buffer.insert_with_tags_by_name(&mut at, PRIVATE_TEXT, &["privmark"]);
                self.buffer.insert(&mut at, "\n");
                self.raw.set(false);
            }
        }
    }

    /// Puts a short centred rule on its own line and leaves the cursor below it.
    pub fn insert_divider(&self) {
        let mut cur = self.cursor();
        if !cur.starts_line() {
            self.buffer.insert(&mut cur, "\n");
        }
        self.raw.set(true);
        self.buffer.insert_with_tags_by_name(&mut cur, HR, &["hr"]);
        self.raw.set(false);
        self.buffer.insert(&mut cur, "\n");
        self.buffer.place_cursor(&cur);
    }

    pub fn toggle_bullet(&self) {
        let (first, last) = self.selected_lines();
        for line in first..=last {
            let Some((mut s, e)) = self.line_bounds(line) else { continue };
            if self.buffer.text(&s, &e, true).starts_with(BULLET) {
                let mut after = s;
                after.forward_chars(2);
                self.buffer.delete(&mut s, &mut after);
            } else {
                self.buffer.insert(&mut s, BULLET);
            }
        }
    }

    /// Continues or ends a bullet list. Returns true when it handled the key.
    pub fn handle_return(&self) -> bool {
        if self.buffer.has_selection() {
            return false;
        }
        let cur = self.cursor();
        let Some((mut s, mut e)) = self.line_bounds(cur.line()) else { return false };
        let text = self.buffer.text(&s, &e, true);
        if !text.starts_with(BULLET) {
            return false;
        }
        if text.as_str() == BULLET {
            self.buffer.delete(&mut s, &mut e);
        } else {
            self.buffer.insert_at_cursor(&format!("\n{BULLET}"));
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(md: &str) -> String {
        let b = gtk::TextBuffer::new(None);
        setup_tags(&b);
        load_markdown(&b, md);
        to_markdown(&b)
    }

    #[test]
    fn markdown_roundtrips() {
        if gtk::init().is_err() {
            eprintln!("no display; skipping");
            return;
        }
        for md in [
            "plain text",
            "some **bold** and *italic* and ~~gone~~ and `code`",
            "**bold *both* bold**",
            "# Title\n## Sub\n> quoted *line*\n- one\n- **two**\n\nlast",
            "literal \\* star and \\\\ slash",
            "above\n***\nbelow",
            "open\n***\n<!--private-->\nsecret\n***\nopen again",
            "",
        ] {
            assert_eq!(roundtrip(md), md);
        }
    }

    #[test]
    fn typing_follows_shortcuts() {
        if gtk::init().is_err() {
            return;
        }
        let b = gtk::TextBuffer::new(None);
        setup_tags(&b);
        let f = Fmt::attach(&b);
        b.insert_at_cursor("plain ");
        f.toggle_inline("bold");
        b.insert_at_cursor("bold ");
        b.insert_at_cursor("more");
        f.toggle_inline("bold");
        b.insert_at_cursor(" done");
        assert_eq!(to_markdown(&b), "plain **bold more** done");

        b.insert_at_cursor("\n");
        f.set_block(Some("h1"));
        b.insert_at_cursor("Heading");
        b.insert_at_cursor("\n");
        b.insert_at_cursor("body");
        f.toggle_bullet();
        assert!(f.handle_return());
        b.insert_at_cursor("next");
        assert_eq!(to_markdown(&b), "plain **bold more** done\n# Heading\n- body\n- next");

        // A block format chosen on an empty line is undone by the same shortcut.
        b.insert_at_cursor("\n");
        f.set_block(Some("h2"));
        f.set_block(Some("h2"));
        b.insert_at_cursor("plain again");
        f.insert_divider();
        b.insert_at_cursor("after");
        assert!(to_markdown(&b).ends_with("- next\nplain again\n***\nafter"));

        // Private marks only the section under the cursor, and toggles off again.
        f.toggle_private_section();
        assert!(to_markdown(&b).ends_with("plain again\n***\n<!--private-->\nafter"));
        b.insert_at_cursor(" more");
        assert!(to_markdown(&b).ends_with("***\n<!--private-->\nafter more"));
        f.toggle_private_section();
        assert!(to_markdown(&b).ends_with("plain again\n***\nafter more"));
    }
}
