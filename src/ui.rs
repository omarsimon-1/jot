use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{Local, NaiveDate};
use gtk::prelude::*;
use gtk::{gdk, gdk_pixbuf, gio, glib};

use crate::ipc::{self, Shared, SharedRef};
use crate::markdown::{self, Fmt};
use crate::vault::{self, Entry, Media, Vault};

const APP_ID: &str = "app.journal.Journal";
const COLUMN: i32 = 520;

const HELP: &str = "Ctrl Enter\tfinish this entry, start the next
Ctrl B / I\tbold / italic
Ctrl Shift S\tstrikethrough
Ctrl E\tcode
Ctrl 1 / 2 / 0\theading / subheading / body
Ctrl Shift Q\tquote
Ctrl Shift L\tbullet list
Ctrl O\tattach photos or videos (or paste, or drop)
Ctrl Shift ⌫\tremove the last attachment
Alt ← / →\tprevious / next day
Ctrl T\ttoday
Ctrl F\tsearch
Ctrl Shift E\texport to markdown
Ctrl D\tlight / dark
Ctrl L\tlock
Ctrl /\tthis list";

struct Draft {
    entry: Entry,
    saved: bool,
    view: gtk::TextView,
    fmt: Rc<Fmt>,
    media_box: gtk::Box,
    time: gtk::Label,
}

struct State {
    day: NaiveDate,
    draft: Option<Draft>,
    dark: bool,
    last_activity: Instant,
    save_timer: Option<glib::SourceId>,
    status_timer: Option<glib::SourceId>,
    hits: Vec<NaiveDate>,
}

pub struct Ui {
    window: gtk::ApplicationWindow,
    stack: gtk::Stack,
    pw: gtk::PasswordEntry,
    pw2: gtk::PasswordEntry,
    lock_msg: gtk::Label,
    scroller: gtk::ScrolledWindow,
    page: gtk::Box,
    status: gtk::Label,
    search: gtk::Revealer,
    search_entry: gtk::Entry,
    results: gtk::ListBox,
    help: gtk::Revealer,
    css: gtk::CssProvider,
    shared: SharedRef,
    dir: PathBuf,
    st: RefCell<State>,
}

fn today() -> NaiveDate {
    Local::now().date_naive()
}

fn tmp_dir() -> PathBuf {
    ipc::socket_path().with_file_name("journal-media")
}

fn label(text: &str, class: &str) -> gtk::Label {
    let l = gtk::Label::builder().label(text).xalign(0.0).wrap(true).build();
    l.add_css_class(class);
    l
}

fn rule() -> gtk::Separator {
    let s = gtk::Separator::new(gtk::Orientation::Horizontal);
    s.add_css_class("rule");
    s
}

fn make_view(editable: bool) -> gtk::TextView {
    let v = gtk::TextView::builder()
        .wrap_mode(gtk::WrapMode::WordChar)
        .editable(editable)
        .cursor_visible(editable)
        .hexpand(true)
        .build();
    v.add_css_class("entry");
    markdown::setup_tags(&v.buffer());
    v
}

fn system_prefers_dark() -> (bool, Option<gio::Settings>) {
    let schema = "org.gnome.desktop.interface";
    let found = gio::SettingsSchemaSource::default().and_then(|s| s.lookup(schema, true));
    match found {
        Some(s) if s.has_key("color-scheme") => {
            let settings = gio::Settings::new(schema);
            (settings.string("color-scheme") == "prefer-dark", Some(settings))
        }
        _ => (true, None),
    }
}

fn css_for(dark: bool) -> String {
    let (bg, fg, muted, rule, accent, panel) = if dark {
        ("#111111", "#cfcfcf", "#6e6e6e", "#2a2a2a", "#cfcfcf", "#181818")
    } else {
        ("#fafafa", "#1c1c1c", "#8c8c8c", "#e2e2e2", "#1c1c1c", "#ffffff")
    };
    format!(
        r#"
window, .page {{ background: {bg}; color: {fg}; }}
* {{ font-family: Georgia, Gelasio, "Noto Serif", serif; }}
scrolledwindow, viewport {{ background: transparent; }}
textview.entry, textview.entry text {{ background: transparent; color: {fg}; caret-color: {accent}; }}
textview.entry {{ font-size: 14px; line-height: 1.55; }}
textview.entry text selection {{ background: alpha({fg}, 0.18); color: {fg}; }}
.date {{ font-size: 12px; color: {muted}; margin-top: 36px; margin-bottom: 20px; }}
.time {{ font-size: 11px; color: {muted}; margin-bottom: 4px; }}
.muted {{ color: {muted}; font-size: 13px; }}
separator.rule {{ background: {rule}; min-height: 1px; margin-top: 20px; margin-bottom: 20px; }}
.status {{ color: {muted}; font-size: 12px; margin-bottom: 14px; padding: 4px 12px; background: {bg}; }}
.lock entry {{ background: transparent; color: {fg}; border: none; border-bottom: 1px solid {rule};
  border-radius: 0; box-shadow: none; outline: none; font-size: 14px; min-width: 220px; caret-color: {accent}; }}
.lock entry:focus-within {{ border-bottom-color: {accent}; }}
.lock-msg {{ color: {muted}; font-size: 12px; margin-top: 12px; }}
.panel {{ background: {panel}; border: 1px solid {rule}; border-radius: 4px; padding: 10px 14px; margin-top: 32px; }}
.panel entry {{ background: transparent; color: {fg}; border: none; box-shadow: none; outline: none;
  font-size: 14px; caret-color: {accent}; }}
.panel list, .panel row {{ background: transparent; color: {fg}; }}
.panel row {{ padding: 5px 4px; border-radius: 3px; }}
.panel row:selected {{ background: alpha({fg}, 0.10); }}
.hit-date {{ color: {muted}; font-size: 11px; }}
.hit-text {{ font-size: 13px; }}
.help {{ font-size: 13px; }}
button.video {{ background: transparent; color: {muted}; border: 1px solid {rule}; border-radius: 3px;
  box-shadow: none; padding: 5px 10px; font-size: 13px; }}
button.video:hover, button.video:focus {{ color: {fg}; border-color: {accent}; }}
.media {{ margin-top: 10px; }}
"#
    )
}

pub fn run() {
    let app = gtk::Application::builder().application_id(APP_ID).build();
    app.connect_activate(|app| {
        if let Some(w) = app.active_window() {
            w.present();
            return;
        }
        let shared: SharedRef = Arc::new(Mutex::new(Shared::default()));
        let (tx, rx) = channel();
        if let Err(e) = ipc::serve(shared.clone(), tx) {
            eprintln!("agent access unavailable: {e}");
        }
        Ui::build(app, shared, rx);
    });
    app.connect_shutdown(|_| {
        let _ = std::fs::remove_dir_all(tmp_dir());
        let _ = std::fs::remove_file(ipc::socket_path());
    });
    app.run_with_args::<&str>(&[]);
}

impl Ui {
    fn build(app: &gtk::Application, shared: SharedRef, rx: Receiver<String>) {
        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("Journal")
            .default_width(900)
            .default_height(960)
            .build();

        // Lock screen
        let pw = gtk::PasswordEntry::new();
        let pw2 = gtk::PasswordEntry::builder().visible(false).build();
        let lock_msg = gtk::Label::new(None);
        lock_msg.add_css_class("lock-msg");
        let lock = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(10)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        lock.add_css_class("lock");
        lock.append(&pw);
        lock.append(&pw2);
        lock.append(&lock_msg);

        // Journal page
        let page = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .halign(gtk::Align::Center)
            .width_request(COLUMN)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::External)
            .child(&page)
            .build();
        scroller.add_css_class("page");

        let stack = gtk::Stack::new();
        stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        stack.set_transition_duration(120);
        stack.add_named(&lock, Some("lock"));
        stack.add_named(&scroller, Some("journal"));

        // Overlays
        let status = gtk::Label::builder().halign(gtk::Align::Center).valign(gtk::Align::End).visible(false).build();
        status.add_css_class("status");

        let search_entry = gtk::Entry::builder().build();
        let results = gtk::ListBox::new();
        results.set_selection_mode(gtk::SelectionMode::Browse);
        let results_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(420)
            .child(&results)
            .build();
        let search_box = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).width_request(COLUMN - 80).build();
        search_box.add_css_class("panel");
        search_box.append(&search_entry);
        search_box.append(&results_scroll);
        let search = gtk::Revealer::builder()
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Start)
            .transition_type(gtk::RevealerTransitionType::Crossfade)
            .transition_duration(100)
            .child(&search_box)
            .build();

        let help_label = gtk::Label::builder().label(HELP).xalign(0.0).build();
        help_label.add_css_class("help");
        let tabs = gtk::pango::TabArray::new(1, true);
        let mut tabs = tabs;
        tabs.set_tab(0, gtk::pango::TabAlign::Left, 130);
        help_label.set_tabs(Some(&tabs));
        let help_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        help_box.add_css_class("panel");
        help_box.append(&help_label);
        let help = gtk::Revealer::builder()
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .transition_type(gtk::RevealerTransitionType::Crossfade)
            .transition_duration(100)
            .child(&help_box)
            .build();

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&stack));
        overlay.add_overlay(&status);
        overlay.add_overlay(&search);
        overlay.add_overlay(&help);
        window.set_child(Some(&overlay));

        let css = gtk::CssProvider::new();
        let display = gdk::Display::default().expect("display");
        gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        if let Some(s) = gtk::Settings::default() {
            s.set_gtk_cursor_aspect_ratio(0.09);
        }

        let (dark, settings) = system_prefers_dark();
        let ui = Rc::new(Ui {
            window,
            stack,
            pw,
            pw2,
            lock_msg,
            scroller,
            page,
            status,
            search,
            search_entry,
            results,
            help,
            css,
            shared,
            dir: vault::default_dir(),
            st: RefCell::new(State {
                day: today(),
                draft: None,
                dark,
                last_activity: Instant::now(),
                save_timer: None,
                status_timer: None,
                hits: Vec::new(),
            }),
        });
        ui.apply_theme();
        if let Some(settings) = settings {
            let u = ui.clone();
            settings.connect_changed(Some("color-scheme"), move |s, _| {
                u.st.borrow_mut().dark = s.string("color-scheme") == "prefer-dark";
                u.apply_theme();
            });
            // Keep the settings object alive for the life of the app.
            std::mem::forget(settings);
        }

        ui.connect_lock_screen();
        ui.connect_keys();
        ui.connect_search();
        ui.connect_drop();
        ui.connect_background(rx);
        let u = ui.clone();
        ui.window.connect_close_request(move |_| {
            u.lock();
            glib::Propagation::Proceed
        });

        ui.show_lock();
        ui.window.present();
        // Debug builds only: lets automated checks open a throwaway vault.
        #[cfg(debug_assertions)]
        if let Ok(p) = std::env::var("JOURNAL_DEBUG_PASSWORD") {
            ui.pw.set_text(&p);
            ui.pw2.set_text(&p);
            ui.unlock();
        }
        #[cfg(debug_assertions)]
        if let Ok(path) = std::env::var("JOURNAL_DEBUG_SHOT") {
            if let Ok(v) = std::env::var("JOURNAL_DEBUG_DARK") {
                ui.st.borrow_mut().dark = v == "1";
                ui.apply_theme();
            }
            let u = ui.clone();
            glib::timeout_add_local_once(Duration::from_millis(600), move || {
                let d = u.st.borrow().draft.as_ref().map(|d| (d.view.buffer(), d.fmt.clone()));
                if let Some((b, f)) = d {
                    b.insert_at_cursor("Writing this ");
                    f.toggle_inline("bold");
                    b.insert_at_cursor("right now");
                    f.toggle_inline("bold");
                    b.insert_at_cursor(", just a cursor and text.");
                }
                if std::env::var("JOURNAL_DEBUG_SEARCH").is_ok() {
                    u.open_search();
                    u.search_entry.set_text("the");
                    let u2 = u.clone();
                    glib::timeout_add_local_once(Duration::from_millis(900), move || {
                        u2.scroller.vadjustment().set_value(0.0)
                    });
                }
            });
            let u = ui.clone();
            glib::timeout_add_local_once(Duration::from_millis(2500), move || {
                let w = &u.window;
                let paintable = gtk::WidgetPaintable::new(Some(w));
                let snap = gtk::Snapshot::new();
                paintable.snapshot(&snap, w.width() as f64, w.height() as f64);
                if let (Some(node), Some(r)) = (snap.to_node(), w.native().and_then(|n| n.renderer())) {
                    let _ = r.render_texture(&node, None).save_to_png(&path);
                }
                u.window.close();
            });
        }
    }

    fn apply_theme(&self) {
        self.css.load_from_string(&css_for(self.st.borrow().dark));
    }

    fn vault(&self) -> Option<Vault> {
        self.shared.lock().unwrap().vault.clone()
    }

    fn set_status(self: &Rc<Self>, text: &str) {
        self.status.set_text(text);
        self.status.set_visible(true);
        let u = self.clone();
        let id = glib::timeout_add_local_once(Duration::from_secs(4), move || {
            u.st.borrow_mut().status_timer = None;
            u.status.set_visible(false);
        });
        if let Some(old) = self.st.borrow_mut().status_timer.replace(id) {
            old.remove();
        }
    }

    // ---- lock / unlock -------------------------------------------------

    fn show_lock(&self) {
        let creating = !Vault::exists(&self.dir);
        self.pw.set_text("");
        self.pw2.set_text("");
        self.pw2.set_visible(creating);
        self.lock_msg.set_text(if creating { "new password, twice · it cannot be recovered" } else { "" });
        self.stack.set_visible_child_name("lock");
        self.pw.grab_focus();
    }

    fn connect_lock_screen(self: &Rc<Self>) {
        let u = self.clone();
        self.pw.connect_activate(move |_| {
            if u.pw2.is_visible() {
                u.pw2.grab_focus();
            } else {
                u.unlock();
            }
        });
        let u = self.clone();
        self.pw2.connect_activate(move |_| u.unlock());
    }

    fn unlock(self: &Rc<Self>) {
        let password = self.pw.text();
        if password.is_empty() {
            return;
        }
        let result = if self.pw2.is_visible() {
            if password != self.pw2.text() {
                self.lock_msg.set_text("Those don't match.");
                self.pw2.set_text("");
                return;
            }
            Vault::create(&self.dir, &password)
        } else {
            Vault::open(&self.dir, &password)
        };
        let loaded = result.and_then(|v| v.load_entries().map(|e| (v, e)));
        match loaded {
            Ok((vault, entries)) => {
                {
                    let mut s = self.shared.lock().unwrap();
                    s.vault = Some(vault);
                    s.entries = entries;
                }
                self.pw.set_text("");
                self.pw2.set_text("");
                let _ = std::fs::remove_dir_all(tmp_dir());
                let mut st = self.st.borrow_mut();
                st.day = today();
                st.last_activity = Instant::now();
                drop(st);
                self.stack.set_visible_child_name("journal");
                self.render();
            }
            Err(e) => {
                self.lock_msg.set_text(&e.to_string());
                self.pw.set_text("");
                self.pw.grab_focus();
            }
        }
    }

    fn lock(self: &Rc<Self>) {
        if self.vault().is_none() {
            return;
        }
        self.save_draft();
        self.st.borrow_mut().draft = None;
        {
            let mut s = self.shared.lock().unwrap();
            s.vault = None;
            s.entries.clear();
        }
        self.clear_page();
        self.search.set_reveal_child(false);
        self.help.set_reveal_child(false);
        let _ = std::fs::remove_dir_all(tmp_dir());
        self.show_lock();
    }

    // ---- page ----------------------------------------------------------

    fn clear_page(&self) {
        while let Some(c) = self.page.first_child() {
            self.page.remove(&c);
        }
    }

    fn flush_draft(&self) {
        if let Some(d) = self.st.borrow_mut().draft.as_mut() {
            d.entry.text = markdown::to_markdown(&d.view.buffer());
        }
    }

    fn render(self: &Rc<Self>) {
        let Some(vault) = self.vault() else { return };
        self.flush_draft();
        self.clear_page();
        let day = self.st.borrow().day;
        let draft_id = self.st.borrow().draft.as_ref().map(|d| d.entry.id.clone()).unwrap_or_default();
        let entries: Vec<Entry> = {
            let s = self.shared.lock().unwrap();
            s.entries.iter().filter(|e| e.date() == day && e.id != draft_id).cloned().collect()
        };

        self.page.append(&label(&day.format("%A, %B %-d, %Y").to_string(), "date"));
        for (i, e) in entries.iter().enumerate() {
            if i > 0 {
                self.page.append(&rule());
            }
            self.page.append(&label(&e.time_label(), "time"));
            let view = make_view(false);
            markdown::load_markdown(&view.buffer(), &e.text);
            if !e.text.trim().is_empty() {
                self.page.append(&view);
            }
            let media_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
            for m in &e.media {
                media_box.append(&self.media_widget(&vault, m));
            }
            self.page.append(&media_box);
        }

        if day == today() {
            if !entries.is_empty() {
                self.page.append(&rule());
            }
            self.build_editor(&vault);
        }
        let spacer = gtk::Box::builder().height_request(480).build();
        self.page.append(&spacer);

        if day != today() {
            self.scroller.vadjustment().set_value(0.0);
        }
    }

    fn build_editor(self: &Rc<Self>, vault: &Vault) {
        let entry = match self.st.borrow_mut().draft.take() {
            Some(d) => (d.entry, d.saved),
            None => (Entry { id: String::new(), created: Local::now(), text: String::new(), media: Vec::new() }, false),
        };
        let (entry, saved) = entry;
        let time_text = if entry.id.is_empty() { String::new() } else { entry.time_label() };
        let time = label(&time_text, "time");
        let view = make_view(true);
        let buffer = view.buffer();
        markdown::load_markdown(&buffer, &entry.text);
        let fmt = Fmt::attach(&buffer);
        let media_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        for m in &entry.media {
            media_box.append(&self.media_widget(vault, m));
        }
        self.page.append(&time);
        self.page.append(&view);
        self.page.append(&media_box);

        let u = self.clone();
        buffer.connect_changed(move |_| u.draft_changed());
        buffer.place_cursor(&buffer.end_iter());
        self.st.borrow_mut().draft = Some(Draft { entry, saved, view: view.clone(), fmt, media_box, time });
        view.grab_focus();
        let u = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(60), move || u.follow_cursor(true));
    }

    fn draft_changed(self: &Rc<Self>) {
        let u = self.clone();
        let id = glib::timeout_add_local_once(Duration::from_millis(500), move || {
            u.st.borrow_mut().save_timer = None;
            u.save_draft();
        });
        if let Some(old) = self.st.borrow_mut().save_timer.replace(id) {
            old.remove();
        }
        let u = self.clone();
        glib::idle_add_local_once(move || u.follow_cursor(false));
    }

    /// Typewriter scrolling: keep the line being written around mid-screen.
    fn follow_cursor(&self, force: bool) {
        let st = self.st.borrow();
        let Some(d) = st.draft.as_ref() else { return };
        let buffer = d.view.buffer();
        let rect = d.view.iter_location(&buffer.iter_at_mark(&buffer.get_insert()));
        let (_, wy) = d.view.buffer_to_window_coords(gtk::TextWindowType::Widget, rect.x(), rect.y());
        let Some(p) = d.view.compute_point(&self.page, &gtk::graphene::Point::new(0.0, wy as f32)) else { return };
        let adj = self.scroller.vadjustment();
        let y = p.y() as f64;
        let seen = y - adj.value();
        if force || seen > adj.page_size() * 0.62 || seen < 40.0 {
            let target = (y - adj.page_size() * 0.45).clamp(0.0, (adj.upper() - adj.page_size()).max(0.0));
            adj.set_value(target);
        }
    }

    /// Writes the entry being composed. An entry gets its timestamp at its first words.
    fn save_draft(self: &Rc<Self>) {
        if let Some(id) = self.st.borrow_mut().save_timer.take() {
            id.remove();
        }
        let Some(vault) = self.vault() else { return };
        self.flush_draft();
        let mut st = self.st.borrow_mut();
        let Some(d) = st.draft.as_mut() else { return };
        let empty = d.entry.text.trim().is_empty() && d.entry.media.is_empty();
        if empty {
            if d.saved {
                vault.delete_entry(&d.entry.id);
                self.shared.lock().unwrap().entries.retain(|e| e.id != d.entry.id);
                d.saved = false;
            }
            d.entry.id.clear();
            d.time.set_text("");
            return;
        }
        if d.entry.id.is_empty() {
            d.entry.created = Local::now();
            d.entry.id = d.entry.created.format("%Y%m%dT%H%M%S%3f").to_string();
            d.time.set_text(&d.entry.time_label());
        }
        let entry = d.entry.clone();
        match vault.save_entry(&entry) {
            Ok(()) => {
                d.saved = true;
                drop(st);
                let mut s = self.shared.lock().unwrap();
                match s.entries.iter_mut().find(|e| e.id == entry.id) {
                    Some(e) => *e = entry,
                    None => s.entries.push(entry),
                }
            }
            Err(e) => {
                drop(st);
                self.set_status(&format!("Could not save: {e}"));
            }
        }
    }

    fn new_entry(self: &Rc<Self>) {
        self.save_draft();
        let mut st = self.st.borrow_mut();
        if st.day != today() {
            st.day = today();
        } else if st.draft.as_ref().is_some_and(|d| !d.saved) {
            return;
        } else {
            st.draft = None;
        }
        drop(st);
        self.render();
    }

    fn go_to(self: &Rc<Self>, day: NaiveDate) {
        self.save_draft();
        self.st.borrow_mut().day = day;
        self.render();
    }

    fn step_day(self: &Rc<Self>, forward: bool) {
        let cur = self.st.borrow().day;
        let mut days: Vec<NaiveDate> = self.shared.lock().unwrap().entries.iter().map(|e| e.date()).collect();
        days.push(today());
        days.sort();
        days.dedup();
        let next = if forward {
            days.into_iter().find(|d| *d > cur)
        } else {
            days.into_iter().rev().find(|d| *d < cur)
        };
        if let Some(d) = next {
            self.go_to(d);
        }
    }

    // ---- media ---------------------------------------------------------

    fn media_widget(self: &Rc<Self>, vault: &Vault, m: &Media) -> gtk::Widget {
        let holder = gtk::Box::new(gtk::Orientation::Vertical, 0);
        holder.add_css_class("media");
        holder.set_halign(gtk::Align::Start);
        if m.video {
            let button = gtk::Button::with_label(&format!("▶   {}", m.name));
            button.add_css_class("video");
            holder.append(&button);
            let (vault, file, holder2) = (vault.clone(), m.file.clone(), holder.clone());
            let u = self.clone();
            button.connect_clicked(move |b| {
                b.set_sensitive(false);
                let (vault, file, holder, u, b) = (vault.clone(), file.clone(), holder2.clone(), u.clone(), b.clone());
                glib::spawn_future_local(async move {
                    let dest = tmp_dir().join(&file);
                    let d2 = dest.clone();
                    let res = gio::spawn_blocking(move || {
                        vault::private_dir(&tmp_dir())?;
                        vault.decrypt_media_to(&file, &d2)
                    })
                    .await;
                    match res {
                        Ok(Ok(())) => {
                            let video = gtk::Video::for_file(Some(&gio::File::for_path(&dest)));
                            video.set_autoplay(true);
                            video.set_size_request(COLUMN, COLUMN * 9 / 16);
                            holder.remove(&b);
                            holder.append(&video);
                        }
                        _ => {
                            b.set_sensitive(true);
                            u.set_status("Could not open that video.");
                        }
                    }
                });
            });
        } else {
            let (vault, file, name, holder2) = (vault.clone(), m.file.clone(), m.name.clone(), holder.clone());
            glib::idle_add_local_once(move || {
                let texture = vault.read_media(&file).ok().and_then(|data| {
                    let stream = gio::MemoryInputStream::from_bytes(&glib::Bytes::from_owned(data));
                    let pb = gdk_pixbuf::Pixbuf::from_stream_at_scale(&stream, 1400, 1400, true, gio::Cancellable::NONE).ok()?;
                    let pb = pb.apply_embedded_orientation().unwrap_or(pb);
                    Some((gdk::Texture::for_pixbuf(&pb), pb.width(), pb.height()))
                });
                match texture {
                    Some((t, w, h)) => {
                        let pic = gtk::Picture::for_paintable(&t);
                        pic.set_content_fit(gtk::ContentFit::Contain);
                        let width = w.min(COLUMN);
                        let height = (width as f64 * h as f64 / w.max(1) as f64) as i32;
                        pic.set_size_request(width, height);
                        holder2.append(&pic);
                    }
                    None => holder2.append(&label(&format!("[{name}]"), "muted")),
                }
            });
        }
        holder.upcast()
    }

    fn attach_done(self: &Rc<Self>, vault: &Vault, res: anyhow::Result<Media>) {
        match res {
            Ok(m) => {
                let widget = self.media_widget(vault, &m);
                let mut st = self.st.borrow_mut();
                match st.draft.as_mut() {
                    Some(d) => {
                        d.entry.media.push(m);
                        d.media_box.append(&widget);
                    }
                    None => {
                        vault.delete_media(&m.file);
                        return;
                    }
                }
                drop(st);
                self.save_draft();
            }
            Err(e) => self.set_status(&format!("Could not attach: {e}")),
        }
    }

    fn ready_to_attach(self: &Rc<Self>) -> Option<Vault> {
        let vault = self.vault()?;
        if self.st.borrow().day != today() {
            self.go_to(today());
        }
        Some(vault)
    }

    fn attach_files(self: &Rc<Self>, paths: Vec<PathBuf>) {
        let Some(vault) = self.ready_to_attach() else { return };
        for path in paths {
            if vault::media_kind(&path).is_none() {
                self.set_status("Only photos and videos can be attached.");
                continue;
            }
            self.set_status("Attaching…");
            let (u, v) = (self.clone(), vault.clone());
            glib::spawn_future_local(async move {
                let v2 = v.clone();
                let res = gio::spawn_blocking(move || v2.import_media(&path)).await;
                u.attach_done(&v, res.unwrap_or_else(|_| Err(anyhow::anyhow!("import failed"))));
            });
        }
    }

    fn attach_dialog(self: &Rc<Self>) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Photos and videos"));
        filter.add_mime_type("image/*");
        filter.add_mime_type("video/*");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder().title("Attach").modal(true).filters(&filters).build();
        let u = self.clone();
        dialog.open_multiple(Some(&self.window), gio::Cancellable::NONE, move |res| {
            if let Ok(files) = res {
                let paths = (0..files.n_items())
                    .filter_map(|i| files.item(i).and_downcast::<gio::File>())
                    .filter_map(|f| f.path())
                    .collect();
                u.attach_files(paths);
            }
        });
    }

    fn remove_last_attachment(self: &Rc<Self>) {
        let Some(vault) = self.vault() else { return };
        let mut st = self.st.borrow_mut();
        let Some(d) = st.draft.as_mut() else { return };
        let Some(m) = d.entry.media.pop() else { return };
        if let Some(w) = d.media_box.last_child() {
            d.media_box.remove(&w);
        }
        drop(st);
        vault.delete_media(&m.file);
        self.save_draft();
    }

    /// Returns true when the clipboard held files or an image and the paste was taken over.
    fn paste_media(self: &Rc<Self>) -> bool {
        let clipboard = self.window.clipboard();
        let formats = clipboard.formats();
        if formats.contains_type(gdk::FileList::static_type()) {
            let u = self.clone();
            clipboard.read_value_async(gdk::FileList::static_type(), glib::Priority::DEFAULT, gio::Cancellable::NONE, move |res| {
                if let Some(list) = res.ok().and_then(|v| v.get::<gdk::FileList>().ok()) {
                    u.attach_files(list.files().iter().filter_map(|f| f.path()).collect());
                }
            });
            return true;
        }
        if formats.contains_type(gdk::Texture::static_type()) && !formats.contains_type(glib::Type::STRING) {
            let u = self.clone();
            clipboard.read_texture_async(gio::Cancellable::NONE, move |res| {
                let Ok(Some(texture)) = res else { return };
                let Some(vault) = u.ready_to_attach() else { return };
                let bytes = texture.save_to_png_bytes();
                let res = vault.import_png(&bytes);
                u.attach_done(&vault, res);
            });
            return true;
        }
        false
    }

    fn connect_drop(self: &Rc<Self>) {
        let target = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        let u = self.clone();
        target.connect_drop(move |_, value, _, _| {
            match value.get::<gdk::FileList>() {
                Ok(list) if u.vault().is_some() => {
                    u.attach_files(list.files().iter().filter_map(|f| f.path()).collect());
                    true
                }
                _ => false,
            }
        });
        self.window.add_controller(target);
    }

    fn export_dialog(self: &Rc<Self>) {
        self.save_draft();
        let dialog = gtk::FileDialog::builder().title("Export journal into…").modal(true).build();
        let u = self.clone();
        dialog.select_folder(Some(&self.window), gio::Cancellable::NONE, move |res| {
            let Some(folder) = res.ok().and_then(|f| f.path()) else { return };
            let (vault, entries) = {
                let s = u.shared.lock().unwrap();
                (s.vault.clone(), s.entries.clone())
            };
            let Some(vault) = vault else { return };
            let dest = folder.join("Journal");
            u.set_status("Exporting…");
            let u = u.clone();
            glib::spawn_future_local(async move {
                let d2 = dest.clone();
                let res = gio::spawn_blocking(move || vault.export(&entries, &d2)).await;
                match res {
                    Ok(Ok(n)) => u.set_status(&format!("Exported {n} day(s) to {}", dest.display())),
                    _ => u.set_status("Export failed."),
                }
            });
        });
    }

    // ---- search --------------------------------------------------------

    fn open_search(&self) {
        self.help.set_reveal_child(false);
        self.search_entry.set_text("");
        self.search.set_reveal_child(true);
        self.search_entry.grab_focus();
    }

    fn close_overlays(&self) {
        self.search.set_reveal_child(false);
        self.help.set_reveal_child(false);
        if let Some(d) = self.st.borrow().draft.as_ref() {
            d.view.grab_focus();
        }
    }

    fn connect_search(self: &Rc<Self>) {
        let u = self.clone();
        self.search_entry.connect_changed(move |e| {
            while let Some(c) = u.results.first_child() {
                u.results.remove(&c);
            }
            let hits: Vec<(NaiveDate, String, String)> = {
                let s = u.shared.lock().unwrap();
                ipc::search(&s.entries, &e.text(), 40)
                    .into_iter()
                    .map(|(en, snip)| (en.date(), format!("{}  ·  {}", en.date().format("%a %b %-d, %Y"), en.time_label()), snip))
                    .collect()
            };
            for (_, when, snip) in &hits {
                let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
                row.append(&label(&when, "hit-date"));
                let text = label(snip, "hit-text");
                text.set_wrap(false);
                text.set_ellipsize(gtk::pango::EllipsizeMode::End);
                row.append(&text);
                u.results.append(&row);
            }
            if let Some(first) = u.results.row_at_index(0) {
                u.results.select_row(Some(&first));
            }
            u.st.borrow_mut().hits = hits.into_iter().map(|h| h.0).collect();
        });
        let u = self.clone();
        self.results.connect_row_activated(move |_, row| u.open_hit(row.index()));
    }

    fn open_hit(self: &Rc<Self>, index: i32) {
        let day = self.st.borrow().hits.get(index as usize).copied();
        if let Some(day) = day {
            self.search.set_reveal_child(false);
            self.go_to(day);
            if let Some(d) = self.st.borrow().draft.as_ref() {
                d.view.grab_focus();
            }
        }
    }

    fn move_hit(&self, delta: i32) {
        let cur = self.results.selected_row().map(|r| r.index()).unwrap_or(-1);
        if let Some(row) = self.results.row_at_index((cur + delta).max(0)) {
            self.results.select_row(Some(&row));
            row.grab_focus();
            self.search_entry.grab_focus_without_selecting();
        }
    }

    // ---- keys ----------------------------------------------------------

    fn with_fmt(self: &Rc<Self>, f: impl FnOnce(&Fmt)) {
        let fmt = {
            let st = self.st.borrow();
            st.draft.as_ref().filter(|d| d.view.has_focus()).map(|d| d.fmt.clone())
        };
        if let Some(fmt) = fmt {
            f(&fmt);
            self.draft_changed();
        }
    }

    fn connect_keys(self: &Rc<Self>) {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let u = self.clone();
        keys.connect_key_pressed(move |_, key, _, mods| {
            if u.on_key(key, mods) {
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        self.window.add_controller(keys);

        let motion = gtk::EventControllerMotion::new();
        let u = self.clone();
        motion.connect_motion(move |_, _, _| u.st.borrow_mut().last_activity = Instant::now());
        self.window.add_controller(motion);
    }

    fn on_key(self: &Rc<Self>, key: gdk::Key, mods: gdk::ModifierType) -> bool {
        use gdk::Key;
        self.st.borrow_mut().last_activity = Instant::now();
        if self.vault().is_none() {
            return false;
        }
        let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);
        let alt = mods.contains(gdk::ModifierType::ALT_MASK);
        let k = key.to_lower();

        let overlay_open = self.search.reveals_child() || self.help.reveals_child();
        if key == Key::Escape && overlay_open {
            self.close_overlays();
            return true;
        }
        if self.search.reveals_child() && !ctrl {
            match key {
                Key::Down => self.move_hit(1),
                Key::Up => self.move_hit(-1),
                Key::Return | Key::KP_Enter => {
                    let i = self.results.selected_row().map(|r| r.index()).unwrap_or(0);
                    self.open_hit(i);
                }
                _ => return false,
            }
            return true;
        }
        if alt && !ctrl {
            match key {
                Key::Left => self.step_day(false),
                Key::Right => self.step_day(true),
                _ => return false,
            }
            return true;
        }
        if !ctrl {
            if matches!(key, Key::Return | Key::KP_Enter) && !shift {
                let fmt = {
                    let st = self.st.borrow();
                    st.draft.as_ref().filter(|d| d.view.has_focus()).map(|d| d.fmt.clone())
                };
                return fmt.is_some_and(|f| f.handle_return());
            }
            return false;
        }
        match (k, shift) {
            (Key::Return, _) | (Key::KP_Enter, _) => self.new_entry(),
            (Key::b, false) => self.with_fmt(|f| f.toggle_inline("bold")),
            (Key::i, false) => self.with_fmt(|f| f.toggle_inline("italic")),
            (Key::s, true) => self.with_fmt(|f| f.toggle_inline("strike")),
            (Key::e, false) => self.with_fmt(|f| f.toggle_inline("code")),
            (Key::_1, _) | (Key::exclam, _) => self.with_fmt(|f| f.set_block(Some("h1"))),
            (Key::_2, _) | (Key::at, _) => self.with_fmt(|f| f.set_block(Some("h2"))),
            (Key::_0, _) | (Key::parenright, _) => self.with_fmt(|f| f.set_block(None)),
            (Key::q, true) => self.with_fmt(|f| f.set_block(Some("quote"))),
            (Key::l, true) => self.with_fmt(|f| f.toggle_bullet()),
            (Key::l, false) => self.lock(),
            (Key::o, false) => self.attach_dialog(),
            (Key::BackSpace, true) => self.remove_last_attachment(),
            (Key::v, false) => return self.paste_media(),
            (Key::t, false) => self.go_to(today()),
            (Key::Page_Up, _) => self.step_day(false),
            (Key::Page_Down, _) => self.step_day(true),
            (Key::f, false) => self.open_search(),
            (Key::e, true) => self.export_dialog(),
            (Key::d, false) => {
                let mut st = self.st.borrow_mut();
                st.dark = !st.dark;
                drop(st);
                self.apply_theme();
            }
            (Key::slash, _) | (Key::question, _) => {
                self.search.set_reveal_child(false);
                self.help.set_reveal_child(!self.help.reveals_child());
            }
            _ => return false,
        }
        true
    }

    // ---- background: idle lock, sleep lock, agent notices ----------------

    fn connect_background(self: &Rc<Self>, rx: Receiver<String>) {
        let idle = std::env::var("JOURNAL_IDLE_MINUTES").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(5);
        let u = self.clone();
        glib::timeout_add_local(Duration::from_millis(400), move || {
            while let Ok(note) = rx.try_recv() {
                u.set_status(&note);
            }
            glib::ControlFlow::Continue
        });
        let u = self.clone();
        glib::timeout_add_seconds_local(10, move || {
            let idle_for = u.st.borrow().last_activity.elapsed();
            if idle > 0 && idle_for > Duration::from_secs(idle * 60) {
                u.lock();
            }
            glib::ControlFlow::Continue
        });

        // Lock when the machine sleeps or the session is locked.
        if let Ok(bus) = gio::bus_get_sync(gio::BusType::System, gio::Cancellable::NONE) {
            for (iface, member) in [
                ("org.freedesktop.login1.Manager", "PrepareForSleep"),
                ("org.freedesktop.login1.Session", "Lock"),
            ] {
                let u = self.clone();
                #[allow(deprecated)]
                let id = bus.signal_subscribe(
                    Some("org.freedesktop.login1"),
                    Some(iface),
                    Some(member),
                    None,
                    None,
                    gio::DBusSignalFlags::NONE,
                    move |_, _, _, _, _, _| u.lock(),
                );
                std::mem::forget(id);
            }
            std::mem::forget(bus);
        }
    }
}
