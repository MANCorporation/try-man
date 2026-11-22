//! The main window: a header bar with Start / Stop, and one page at a time
//! (welcome, download, the VM screen, errors).

use std::cell::{Cell, RefCell};
use std::fs;
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk::glib::translate::IntoGlib;
use gtk::{gdk, gio, glib};

use crate::release::{self, CANCELLED};
use crate::system::{self, Arch};
use crate::vm::{self, Qemu, VmConfig, VmPaths};
use crate::vnc::VncClient;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Idle,
    Preparing,
    Running,
    Saving,
    Stopping,
}

/// What to do once QEMU has exited.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AfterExit {
    Nothing,
    Close,
    Delete,
    DeleteAndClose,
    DeleteAndRestart,
}

enum WorkerMsg {
    Status(String),
    Progress(u64, u64),
    Verifying,
    Ready(Result<VmConfig, String>),
}

/// One running QEMU process.
struct Session {
    id: u64,
    child: Child,
    qemu: Qemu,
    vnc: Option<Arc<VncClient>>,
    sockets: vm::Sockets,
    stderr: Arc<Mutex<String>>,
    resuming: bool,
    shown_first_frame: bool,
    stop_flag: Arc<AtomicBool>,
    cfg: VmConfig,
    /// Accelerated 3D graphics are on for this run.
    gl: bool,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(vnc) = &self.vnc {
            vnc.shutdown();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.sockets.cleanup();
    }
}

pub struct TryMan {
    window: adw::ApplicationWindow,
    title: adw::WindowTitle,
    toasts: adw::ToastOverlay,
    banner: adw::Banner,
    stack: gtk::Stack,
    start_button: gtk::Button,
    stop_button: gtk::Button,

    welcome: adw::StatusPage,
    welcome_button: gtk::Button,
    welcome_note: gtk::Label,

    download_page: adw::StatusPage,
    progress: gtk::ProgressBar,
    progress_label: gtk::Label,

    busy_page: adw::StatusPage,

    error_page: adw::StatusPage,
    error_expander: gtk::Expander,
    error_details: gtk::Label,

    qemu_command: gtk::Label,

    display: gtk::Picture,
    display_loading: gtk::Box,
    display_loading_label: gtk::Label,

    phase: Cell<Phase>,
    after_exit: Cell<AfterExit>,
    session: RefCell<Option<Session>>,
    session_counter: Cell<u64>,
    cancel_download: RefCell<Arc<AtomicBool>>,
    arch: Option<Arch>,
    qemu: RefCell<Option<Qemu>>,
    paths: VmPaths,
    settings: RefCell<vm::Settings>,
    /// 3D graphics failed to start once; don't try again until restart.
    gl_failed: Cell<bool>,

    mouse_buttons: Cell<u8>,
    keys_down: RefCell<Vec<u32>>,
    shown_click_hint: Cell<bool>,
}

fn icon_button(icon: &str, label: &str, tooltip: &str) -> gtk::Button {
    let content = adw::ButtonContent::builder().icon_name(icon).label(label).build();
    gtk::Button::builder().child(&content).tooltip_text(tooltip).build()
}

fn pill(label: &str, suggested: bool) -> gtk::Button {
    let b = gtk::Button::with_label(label);
    b.add_css_class("pill");
    if suggested {
        b.add_css_class("suggested-action");
    }
    b.set_halign(gtk::Align::Center);
    b
}

fn spinner(size: i32) -> gtk::Spinner {
    let s = gtk::Spinner::new();
    s.set_size_request(size, size);
    s.start();
    s
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(
        ".vm-display { background-color: black; }
         .vm-loading { color: white; }
         .man-welcome image.icon { -gtk-icon-size: 192px; }",
    );
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

/// The official MAN logo, unchanged.
const MAN_LOGO_PNG: &[u8] = include_bytes!("../data/man-logo.png");

/// The MAN logo in GNOME's "development version" style: the logo shape is
/// wrapped in black-and-yellow hazard stripes. Also used as the app icon.
pub(crate) fn man_logo_with_tape() -> Option<gdk::MemoryTexture> {
    use gtk::cairo;

    let logo = gdk::Texture::from_bytes(&glib::Bytes::from_static(MAN_LOGO_PNG)).ok()?;
    let (w, h) = (logo.width(), logo.height());
    let stride = cairo::Format::ARgb32.stride_for_width(w as u32).ok()?;
    let mut pixels = vec![0u8; stride as usize * h as usize];
    // Downloads as premultiplied native-endian ARGB, which is cairo's format.
    logo.download(&mut pixels, stride as usize);
    let logo_surface =
        cairo::ImageSurface::create_for_data(pixels, cairo::Format::ARgb32, w, h, stride).ok()?;

    // Hazard stripes over the whole picture (0.0 = from the very top).
    let stripes = cairo::ImageSurface::create(cairo::Format::ARgb32, w, h).ok()?;
    {
        let cr = cairo::Context::new(&stripes).ok()?;
        // Work in a 1000 x 1000 box whatever the image size is.
        cr.scale(w as f64 / 1000.0, h as f64 / 1000.0);
        let top = 0.0;
        let band = 1000.0 - top;
        cr.rectangle(0.0, top, 1000.0, band);
        cr.clip();
        cr.set_source_rgb(0xf6 as f64 / 255.0, 0xd3 as f64 / 255.0, 0x2d as f64 / 255.0);
        cr.paint().ok()?;
        cr.set_source_rgb(0x1e as f64 / 255.0, 0x1e as f64 / 255.0, 0x1e as f64 / 255.0);
        let period = 120.0;
        let mut x = -band;
        while x < 1000.0 {
            cr.move_to(x, 1000.0);
            cr.line_to(x + band, top);
            cr.line_to(x + band + period / 2.0, top);
            cr.line_to(x + period / 2.0, 1000.0);
            cr.close_path();
            x += period;
        }
        cr.fill().ok()?;
    }

    // The logo, then the stripes painted only where the logo is.
    let mut out = cairo::ImageSurface::create(cairo::Format::ARgb32, w, h).ok()?;
    {
        let cr = cairo::Context::new(&out).ok()?;
        cr.set_source_surface(&logo_surface, 0.0, 0.0).ok()?;
        cr.paint().ok()?;
        cr.set_source_surface(&stripes, 0.0, 0.0).ok()?;
        cr.mask_surface(&logo_surface, 0.0, 0.0).ok()?;
    }
    out.flush();
    let out_stride = out.stride() as usize;
    let data = out.data().ok()?.to_vec();
    let format = if cfg!(target_endian = "little") {
        gdk::MemoryFormat::B8g8r8a8Premultiplied
    } else {
        gdk::MemoryFormat::A8r8g8b8Premultiplied
    };
    Some(gdk::MemoryTexture::new(w, h, format, &glib::Bytes::from_owned(data), out_stride))
}

/// Makes the striped logo the icon of the window and the About dialog,
/// even when the app was never installed system-wide.
fn install_app_icon(logo: &gdk::MemoryTexture) {
    let dir = glib::user_data_dir().join("try-man").join("icons");
    if fs::create_dir_all(&dir).is_ok() {
        let _ = logo.save_to_png(dir.join(format!("{}.png", crate::APP_ID)));
        if let Some(display) = gdk::Display::default() {
            gtk::IconTheme::for_display(&display).add_search_path(&dir);
        }
    }
    gtk::Window::set_default_icon_name(crate::APP_ID);
}

fn format_gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

impl TryMan {
    pub fn new(app: &adw::Application) -> Rc<Self> {
        load_css();

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Try MAN")
            .default_width(1100)
            .default_height(760)
            .width_request(480)
            .height_request(400)
            .build();

        // --- Header bar -----------------------------------------------------
        let title = adw::WindowTitle::new("Try MAN", "");
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&title));

        let start_button = icon_button("media-playback-start-symbolic", "Start", "Start MAN");
        start_button.add_css_class("suggested-action");
        let stop_button = icon_button("media-playback-stop-symbolic", "Stop", "Turn MAN off");
        stop_button.add_css_class("destructive-action");
        header.pack_start(&start_button);
        header.pack_start(&stop_button);

        let menu = gio::Menu::new();
        menu.append(Some("Use a MAN ISO File…"), Some("win.import"));
        menu.append(Some("Fast 3D Graphics"), Some("win.fast-graphics"));
        menu.append(Some("Delete MAN From This Computer…"), Some("win.delete"));
        menu.append(Some("About Try MAN"), Some("win.about"));
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu)
            .tooltip_text("Menu")
            .primary(true)
            .build();
        header.pack_end(&menu_button);

        let banner = adw::Banner::new("");
        banner.set_button_label(Some("Get It"));

        // --- Welcome --------------------------------------------------------
        let welcome_button = pill("Start MAN", true);
        let welcome_note = gtk::Label::builder()
            .wrap(true)
            .justify(gtk::Justification::Center)
            .css_classes(["dim-label"])
            .build();
        let welcome_box = gtk::Box::new(gtk::Orientation::Vertical, 18);
        welcome_box.append(&welcome_button);
        welcome_box.append(&welcome_note);
        let import_button = gtk::Button::builder()
            .label("Use a MAN ISO File You Already Have…")
            .halign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        welcome_box.append(&import_button);
        let welcome = adw::StatusPage::builder()
            .title("Try MAN")
            .child(&welcome_box)
            .css_classes(["man-welcome"])
            .build();
        match man_logo_with_tape() {
            Some(logo) => {
                welcome.set_paintable(Some(&logo));
                install_app_icon(&logo);
            }
            None => welcome.set_icon_name(Some("computer-symbolic")),
        }

        // --- Download -------------------------------------------------------
        let progress = gtk::ProgressBar::new();
        progress.set_size_request(340, -1);
        progress.set_halign(gtk::Align::Center);
        let progress_label = gtk::Label::builder().css_classes(["dim-label", "numeric"]).build();
        let cancel_button = pill("Cancel", false);
        let download_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        download_box.append(&progress);
        download_box.append(&progress_label);
        download_box.append(&cancel_button);
        cancel_button.set_margin_top(12);
        let download_page = adw::StatusPage::builder()
            .icon_name("folder-download-symbolic")
            .title("Getting MAN Ready")
            .child(&download_box)
            .build();

        // --- Busy (saving / turning off) -------------------------------------
        let busy_page = adw::StatusPage::builder().child(&spinner(48)).build();

        // --- Error ----------------------------------------------------------
        let error_retry = pill("Try Again", true);
        let error_details = gtk::Label::builder()
            .selectable(true)
            .wrap(true)
            .xalign(0.0)
            .max_width_chars(90)
            .css_classes(["monospace", "dim-label"])
            .build();
        let error_expander = gtk::Expander::builder()
            .label("Technical Details")
            .child(&error_details)
            .halign(gtk::Align::Center)
            .build();
        let error_box = gtk::Box::new(gtk::Orientation::Vertical, 18);
        error_box.append(&error_retry);
        error_box.append(&error_expander);
        let error_page = adw::StatusPage::builder()
            .icon_name("dialog-error-symbolic")
            .child(&error_box)
            .build();

        // --- QEMU missing ---------------------------------------------------
        let qemu_command = gtk::Label::builder()
            .selectable(true)
            .wrap(true)
            .css_classes(["monospace", "title-4"])
            .build();
        let copy_button = icon_button("edit-copy-symbolic", "Copy Command", "Copy the command");
        copy_button.add_css_class("pill");
        copy_button.set_halign(gtk::Align::Center);
        let qemu_retry = pill("I Installed It", true);
        let qemu_box = gtk::Box::new(gtk::Orientation::Vertical, 18);
        qemu_box.append(&qemu_command);
        qemu_box.append(&copy_button);
        qemu_box.append(&qemu_retry);
        let qemu_page = adw::StatusPage::builder()
            .icon_name("dialog-warning-symbolic")
            .title("One More Thing Is Needed")
            .description(
                "Try MAN uses a free program called QEMU to run MAN.\n\
                 Open the Terminal app, paste this command and press Enter:",
            )
            .child(&qemu_box)
            .build();

        // --- VM screen ------------------------------------------------------
        let display = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Contain)
            .can_shrink(true)
            .hexpand(true)
            .vexpand(true)
            .focusable(true)
            .can_focus(true)
            .build();
        let display_loading_label = gtk::Label::builder()
            .label("Starting MAN…")
            .css_classes(["vm-loading", "title-3"])
            .build();
        let display_loading = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(18)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .can_target(false)
            .build();
        let loading_spinner = spinner(40);
        loading_spinner.add_css_class("vm-loading");
        display_loading.append(&loading_spinner);
        display_loading.append(&display_loading_label);
        let overlay = gtk::Overlay::new();
        overlay.add_css_class("vm-display");
        overlay.set_child(Some(&display));
        overlay.add_overlay(&display_loading);

        // --- Put it together ------------------------------------------------
        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .build();
        stack.add_named(&welcome, Some("welcome"));
        stack.add_named(&download_page, Some("download"));
        stack.add_named(&busy_page, Some("busy"));
        stack.add_named(&error_page, Some("error"));
        stack.add_named(&qemu_page, Some("qemu"));
        stack.add_named(&overlay, Some("display"));

        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&stack));

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.add_top_bar(&banner);
        toolbar.set_content(Some(&toasts));
        window.set_content(Some(&toolbar));

        let this = Rc::new(TryMan {
            window,
            title,
            toasts,
            banner,
            stack,
            start_button,
            stop_button,
            welcome,
            welcome_button,
            welcome_note,
            download_page,
            progress,
            progress_label,
            busy_page,
            error_page,
            error_expander,
            error_details,
            qemu_command,
            display,
            display_loading,
            display_loading_label,
            phase: Cell::new(Phase::Idle),
            after_exit: Cell::new(AfterExit::Nothing),
            session: RefCell::new(None),
            session_counter: Cell::new(0),
            cancel_download: RefCell::new(Arc::new(AtomicBool::new(false))),
            arch: Arch::host(),
            qemu: RefCell::new(None),
            settings: RefCell::new(vm::load_settings(&VmPaths::new())),
            gl_failed: Cell::new(false),
            paths: VmPaths::new(),
            mouse_buttons: Cell::new(0),
            keys_down: RefCell::new(Vec::new()),
            shown_click_hint: Cell::new(false),
        });

        this.connect_signals(&cancel_button, &error_retry, &copy_button, &qemu_retry);
        this.connect_display_input();
        let t = this.clone();
        import_button.connect_clicked(move |_| t.ask_import());
        this.set_phase(Phase::Idle);
        this.show_welcome();
        this.check_for_new_version();
        this
    }

    pub fn present(&self) {
        self.window.present();
    }

    /// Called when the application quits: never leave QEMU running.
    pub fn shutdown(&self) {
        self.session.borrow_mut().take();
    }

    fn toast(&self, text: &str) {
        self.toasts.add_toast(adw::Toast::new(text));
    }

    fn vnc(&self) -> Option<Arc<VncClient>> {
        self.session.borrow().as_ref().and_then(|s| s.vnc.clone())
    }

    // -----------------------------------------------------------------------
    // Signals

    fn connect_signals(
        self: &Rc<Self>,
        cancel_button: &gtk::Button,
        error_retry: &gtk::Button,
        copy_button: &gtk::Button,
        qemu_retry: &gtk::Button,
    ) {
        let this = self.clone();
        self.start_button.connect_clicked(move |_| this.start());
        let this = self.clone();
        self.welcome_button.connect_clicked(move |_| this.start());
        let this = self.clone();
        self.stop_button.connect_clicked(move |_| this.ask_stop());
        let this = self.clone();
        error_retry.connect_clicked(move |_| this.start());
        let this = self.clone();
        qemu_retry.connect_clicked(move |_| this.start());
        let this = self.clone();
        copy_button.connect_clicked(move |_| {
            this.window.clipboard().set_text(&this.qemu_command.label());
            this.toast("Command copied");
        });
        let this = self.clone();
        cancel_button.connect_clicked(move |_| {
            this.cancel_download.borrow().store(true, Ordering::Relaxed);
            this.set_phase(Phase::Idle);
            this.show_welcome();
            if this.download_page.title() == "Importing MAN" {
                this.toast("Import cancelled");
            } else {
                this.toast("Download paused. Press Start to continue it.");
            }
        });
        let this = self.clone();
        self.banner.connect_button_clicked(move |_| this.ask_update());

        let this = self.clone();
        self.window.connect_close_request(move |_| this.on_close_request());

        let delete = gio::SimpleAction::new("delete", None);
        let this = self.clone();
        delete.connect_activate(move |_, _| this.ask_delete());
        self.window.add_action(&delete);

        let fast = gio::SimpleAction::new_stateful(
            "fast-graphics",
            None,
            &self.settings.borrow().fast_graphics.to_variant(),
        );
        // Only offer it where the computer can actually do it.
        fast.set_enabled(
            self.arch
                .and_then(vm::find_qemu)
                .is_some_and(|q| q.gl),
        );
        let this = self.clone();
        fast.connect_activate(move |action, _| {
            let on = !action.state().and_then(|v| v.get::<bool>()).unwrap_or(true);
            action.set_state(&on.to_variant());
            this.settings.borrow_mut().fast_graphics = on;
            vm::save_settings(&this.paths, &this.settings.borrow());
            let when = if this.session.borrow().is_some() { " the next time MAN starts" } else { "" };
            this.toast(&if on {
                format!("Fast 3D graphics will be used{when}")
            } else {
                format!("Normal graphics will be used{when}. Saving MAN is now possible.")
            });
        });
        self.window.add_action(&fast);

        let import = gio::SimpleAction::new("import", None);
        let this = self.clone();
        import.connect_activate(move |_, _| this.ask_import());
        self.window.add_action(&import);

        // Dropping an .iso file onto the window imports it.
        let drop = gtk::DropTarget::new(gio::File::static_type(), gdk::DragAction::COPY);
        let this = self.clone();
        drop.connect_drop(move |_, value, _, _| {
            match value.get::<gio::File>().ok().and_then(|f| f.path()) {
                Some(path) => {
                    this.import_iso(path);
                    true
                }
                None => false,
            }
        });
        self.window.add_controller(drop);

        let about = gio::SimpleAction::new("about", None);
        let this = self.clone();
        about.connect_activate(move |_, _| this.show_about());
        self.window.add_action(&about);
    }

    fn connect_display_input(self: &Rc<Self>) {
        // Mouse buttons
        let click = gtk::GestureClick::new();
        click.set_button(0);
        let this = self.clone();
        click.connect_pressed(move |g, _, x, y| {
            this.display.grab_focus();
            let bit = mouse_bit(g.current_button());
            this.mouse_buttons.set(this.mouse_buttons.get() | bit);
            this.send_pointer(x, y, 0);
        });
        let this = self.clone();
        click.connect_released(move |g, _, x, y| {
            let bit = mouse_bit(g.current_button());
            this.mouse_buttons.set(this.mouse_buttons.get() & !bit);
            this.send_pointer(x, y, 0);
        });
        self.display.add_controller(click);

        // Mouse movement
        let motion = gtk::EventControllerMotion::new();
        let this = self.clone();
        motion.connect_motion(move |_, x, y| this.send_pointer(x, y, 0));
        self.display.add_controller(motion);

        // Scroll wheel
        let scroll = gtk::EventControllerScroll::new(
            gtk::EventControllerScrollFlags::VERTICAL | gtk::EventControllerScrollFlags::DISCRETE,
        );
        let this = self.clone();
        scroll.connect_scroll(move |ctrl, _dx, dy| {
            let Some(vnc) = this.vnc() else { return glib::Propagation::Proceed };
            let bit = if dy < 0.0 { 1 << 3 } else if dy > 0.0 { 1 << 4 } else { return glib::Propagation::Stop };
            let Some((x, y)) = ctrl
                .current_event()
                .and_then(|e| e.position())
                .and_then(|(x, y)| this.to_guest(&vnc, x, y))
            else {
                return glib::Propagation::Stop;
            };
            let buttons = this.mouse_buttons.get();
            for _ in 0..(dy.abs().round() as u32).max(1) {
                vnc.pointer(buttons | bit, x, y);
                vnc.pointer(buttons, x, y);
            }
            glib::Propagation::Stop
        });
        self.display.add_controller(scroll);

        // Keyboard
        let keys = gtk::EventControllerKey::new();
        let this = self.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(vnc) = this.vnc() else { return glib::Propagation::Proceed };
            let sym = keysym(key);
            vnc.key(sym, true);
            let mut down = this.keys_down.borrow_mut();
            if !down.contains(&sym) {
                down.push(sym);
            }
            glib::Propagation::Stop
        });
        let this = self.clone();
        keys.connect_key_released(move |_, key, _, _| {
            let sym = keysym(key);
            if let Some(vnc) = this.vnc() {
                vnc.key(sym, false);
            }
            this.keys_down.borrow_mut().retain(|k| *k != sym);
        });
        self.display.add_controller(keys);

        // Don't leave keys "stuck" when the window loses focus.
        let focus = gtk::EventControllerFocus::new();
        let this = self.clone();
        focus.connect_leave(move |_| this.release_keys());
        self.display.add_controller(focus);
    }

    fn release_keys(&self) {
        let keys: Vec<u32> = self.keys_down.borrow_mut().drain(..).collect();
        if let Some(vnc) = self.vnc() {
            for k in keys {
                vnc.key(k, false);
            }
        }
    }

    /// Window coordinates -> VM screen coordinates.
    fn to_guest(&self, vnc: &VncClient, x: f64, y: f64) -> Option<(u16, u16)> {
        let (fw, fh) = vnc.size();
        if fw == 0 || fh == 0 {
            return None;
        }
        let (fw, fh) = (fw as f64, fh as f64);
        let ww = self.display.width() as f64;
        let wh = self.display.height() as f64;
        let scale = (ww / fw).min(wh / fh);
        if scale <= 0.0 {
            return None;
        }
        let gx = ((x - (ww - fw * scale) / 2.0) / scale).clamp(0.0, fw - 1.0);
        let gy = ((y - (wh - fh * scale) / 2.0) / scale).clamp(0.0, fh - 1.0);
        Some((gx as u16, gy as u16))
    }

    fn send_pointer(&self, x: f64, y: f64, extra: u8) {
        if let Some(vnc) = self.vnc() {
            if let Some((gx, gy)) = self.to_guest(&vnc, x, y) {
                vnc.pointer(self.mouse_buttons.get() | extra, gx, gy);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Pages

    fn set_phase(&self, phase: Phase) {
        self.phase.set(phase);
        self.start_button.set_sensitive(phase == Phase::Idle);
        self.stop_button.set_sensitive(phase == Phase::Running);
        self.title.set_subtitle(match phase {
            Phase::Idle => "",
            Phase::Preparing => "Getting ready…",
            Phase::Running => "Running",
            Phase::Saving => "Saving…",
            Phase::Stopping => "Turning off…",
        });
    }

    fn show_welcome(&self) {
        let cfg = vm::load_config(&self.paths).ok();
        let normal = "Try MAN safely in a virtual computer.\nNothing on your computer will be changed.";
        match &cfg {
            None => {
                self.welcome.set_title("Try MAN");
                self.welcome.set_description(Some(normal));
                self.welcome_button.set_label("Download and Start");
                self.welcome_note.set_label(
                    "MAN will be downloaded first (about 2 GB).\nThis can take a while on a slow connection.",
                );
            }
            Some(cfg) if self.paths.state.exists() => {
                self.welcome.set_title("Welcome Back");
                self.welcome
                    .set_description(Some("MAN is saved exactly the way you left it."));
                self.welcome_button.set_label("Continue");
                self.welcome_note.set_label(&cfg.version_name);
            }
            Some(cfg) => {
                self.welcome.set_title("Try MAN");
                self.welcome.set_description(Some(normal));
                self.welcome_button.set_label("Start MAN");
                self.welcome_note.set_label(&cfg.version_name);
            }
        }
        if self.arch.is_none() {
            self.welcome_note
                .set_label("Sorry, MAN doesn't run on this kind of computer.");
        }
        self.stack.set_visible_child_name("welcome");
    }

    fn show_busy(&self, title: &str, description: &str) {
        self.busy_page.set_title(title);
        self.busy_page.set_description(Some(description));
        self.stack.set_visible_child_name("busy");
    }

    fn show_error(&self, title: &str, description: &str, details: &str) {
        self.error_page.set_title(title);
        self.error_page.set_description(Some(description));
        self.error_details.set_label(details.trim());
        self.error_expander.set_visible(!details.trim().is_empty());
        self.error_expander.set_expanded(false);
        self.stack.set_visible_child_name("error");
    }

    // -----------------------------------------------------------------------
    // Starting

    fn start(self: &Rc<Self>) {
        if self.phase.get() != Phase::Idle {
            return;
        }
        let Some(arch) = self.arch else {
            self.show_error(
                "Your Computer Isn't Supported",
                &format!(
                    "MAN can only run on Intel, AMD or ARM 64-bit processors.\nThis computer has: {}",
                    std::env::consts::ARCH
                ),
                "",
            );
            return;
        };

        let qemu = vm::find_qemu(arch);
        *self.qemu.borrow_mut() = qemu.clone();
        let Some(qemu) = qemu else {
            self.qemu_command.set_label(&system::qemu_install_command());
            self.stack.set_visible_child_name("qemu");
            return;
        };

        match vm::load_config(&self.paths) {
            Ok(cfg) => self.launch(cfg),
            Err(_) => self.prepare(arch, qemu),
        }
    }

    /// Downloads MAN (if needed) and creates the virtual computer.
    fn prepare(self: &Rc<Self>, arch: Arch, qemu: Qemu) {
        let paths = self.paths.clone();
        self.run_preparation(
            "Getting MAN Ready",
            "Looking for the newest version of MAN…",
            move |send, cancel| {
                let (iso, name) = match release::latest(arch) {
                    Ok(rel) => {
                        send(WorkerMsg::Status(format!("Downloading {}…", rel.name)));
                        let iso = release::download(
                            &rel,
                            &paths.iso_dir,
                            cancel,
                            |done, total| send(WorkerMsg::Progress(done, total)),
                            || send(WorkerMsg::Verifying),
                        )?;
                        release::remove_others(&paths.iso_dir, &iso);
                        (iso, rel.name)
                    }
                    // No internet? Use an earlier download if there is one.
                    Err(e) => match release::local_iso(&paths.iso_dir, arch) {
                        Some(iso) => {
                            let name = release::name_from_iso(&iso);
                            (iso, name)
                        }
                        None => return Err(e),
                    },
                };
                send(WorkerMsg::Status("Setting up the virtual computer…".into()));
                let cfg = VmConfig::new(arch, iso, name);
                vm::create(&paths, &cfg, &qemu)?;
                Ok(cfg)
            },
        );
    }

    /// Copies an ISO file the person picked and creates the virtual computer.
    fn prepare_from_iso(self: &Rc<Self>, arch: Arch, qemu: Qemu, source: PathBuf) {
        let paths = self.paths.clone();
        self.run_preparation("Importing MAN", "Copying the ISO file…", move |send, cancel| {
            let iso = release::copy_iso(&source, &paths.iso_dir, cancel, |done, total| {
                send(WorkerMsg::Progress(done, total))
            })?;
            release::remove_others(&paths.iso_dir, &iso);
            send(WorkerMsg::Status("Setting up the virtual computer…".into()));
            let cfg = VmConfig::new(arch, iso.clone(), release::name_from_iso(&iso));
            vm::create(&paths, &cfg, &qemu)?;
            Ok(cfg)
        });
    }

    /// Shows the progress page while `work` runs in the background, then
    /// starts MAN.
    fn run_preparation<F>(self: &Rc<Self>, title: &str, description: &str, work: F)
    where
        F: FnOnce(&dyn Fn(WorkerMsg), &AtomicBool) -> Result<VmConfig, String> + Send + 'static,
    {
        self.set_phase(Phase::Preparing);
        self.download_page.set_title(title);
        self.download_page.set_description(Some(description));
        self.progress.set_fraction(0.0);
        self.progress_label.set_label("");
        self.stack.set_visible_child_name("download");

        let cancel = Arc::new(AtomicBool::new(false));
        *self.cancel_download.borrow_mut() = cancel.clone();

        let (tx, rx) = async_channel::unbounded();
        std::thread::spawn(move || {
            let send = |m: WorkerMsg| {
                let _ = tx.send_blocking(m);
            };
            let result = work(&send, &cancel);
            send(WorkerMsg::Ready(result));
        });

        let this = self.clone();
        glib::spawn_future_local(async move {
            let mut speed_start: Option<(Instant, u64)> = None;
            while let Ok(msg) = rx.recv().await {
                if this.phase.get() != Phase::Preparing {
                    continue; // cancelled
                }
                match msg {
                    WorkerMsg::Status(text) => this.download_page.set_description(Some(&text)),
                    WorkerMsg::Progress(done, total) => {
                        this.progress
                            .set_fraction(if total > 0 { done as f64 / total as f64 } else { 0.0 });
                        let (t0, b0) = *speed_start.get_or_insert((Instant::now(), done));
                        let secs = t0.elapsed().as_secs_f64();
                        let mut text = format!("{} of {}", format_gb(done), format_gb(total));
                        if secs > 3.0 && done > b0 {
                            let left = (total - done) as f64 / ((done - b0) as f64 / secs);
                            text.push_str(&if left < 60.0 {
                                " — less than a minute left".to_string()
                            } else {
                                format!(" — about {} min left", (left / 60.0).ceil() as u64)
                            });
                        }
                        this.progress_label.set_label(&text);
                    }
                    WorkerMsg::Verifying => {
                        this.download_page
                            .set_description(Some("Checking that the download is complete…"));
                        this.progress.set_fraction(1.0);
                        this.progress_label.set_label("");
                    }
                    WorkerMsg::Ready(Ok(cfg)) => this.launch(cfg),
                    WorkerMsg::Ready(Err(e)) => {
                        if e != CANCELLED {
                            this.set_phase(Phase::Idle);
                            this.show_error("Couldn't Get MAN Ready", &e, "");
                        }
                    }
                }
            }
        });
    }

    // -----------------------------------------------------------------------
    // Using an ISO file the person already has

    fn ask_import(self: &Rc<Self>) {
        if self.phase.get() != Phase::Idle {
            self.toast("Turn MAN off first, then try again.");
            return;
        }
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("ISO disc images"));
        filter.add_pattern("*.iso");
        filter.add_pattern("*.ISO");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder()
            .title("Choose a MAN ISO File")
            .modal(true)
            .filters(&filters)
            .default_filter(&filter)
            .build();
        let this = self.clone();
        dialog.open(Some(&self.window), gio::Cancellable::NONE, move |result| {
            if let Some(path) = result.ok().and_then(|f| f.path()) {
                this.import_iso(path);
            }
        });
    }

    fn import_iso(self: &Rc<Self>, path: PathBuf) {
        if self.phase.get() != Phase::Idle {
            self.toast("Turn MAN off first, then try again.");
            return;
        }
        let Some(arch) = self.arch else {
            self.start(); // explains that this computer isn't supported
            return;
        };
        if let Err(e) = release::check_iso(&path) {
            self.show_message("This File Can't Be Used", &e);
            return;
        }

        let mut warnings = Vec::new();
        match release::iso_kind(&path) {
            release::IsoKind::AppleSilicon => warnings.push(
                "This is the ISO for installing MAN directly on an Apple Silicon Mac. \
                 To try MAN in a virtual computer, the arm64 ISO is the right one."
                    .to_string(),
            ),
            release::IsoKind::Arch(iso_arch) if iso_arch != arch => warnings.push(format!(
                "This ISO is made for computers with an {}, but this computer has an {}. \
                 MAN probably won't start.",
                iso_arch.label(),
                arch.label()
            )),
            _ => {}
        }
        if self.paths.config.exists() {
            warnings.push(
                "Your current MAN virtual computer will be replaced, including anything saved inside it."
                    .to_string(),
            );
        }

        if warnings.is_empty() {
            self.use_iso(arch, path);
            return;
        }
        let dialog = adw::AlertDialog::new(Some("Use This ISO File?"), Some(&warnings.join("\n\n")));
        dialog.add_responses(&[("cancel", "Cancel"), ("use", "Use It")]);
        dialog.set_response_appearance("use", adw::ResponseAppearance::Suggested);
        dialog.set_close_response("cancel");
        let this = self.clone();
        dialog.connect_response(None, move |_, response| {
            if response == "use" {
                this.use_iso(arch, path.clone());
            }
        });
        dialog.present(Some(&self.window));
    }

    fn use_iso(self: &Rc<Self>, arch: Arch, path: PathBuf) {
        let qemu = vm::find_qemu(arch);
        *self.qemu.borrow_mut() = qemu.clone();
        let Some(qemu) = qemu else {
            self.qemu_command.set_label(&system::qemu_install_command());
            self.stack.set_visible_child_name("qemu");
            return;
        };
        // Only the virtual computer: the picked file may be in the downloads folder.
        let _ = fs::remove_dir_all(&self.paths.dir);
        self.banner.set_revealed(false);
        self.prepare_from_iso(arch, qemu, path);
    }

    fn show_message(&self, heading: &str, body: &str) {
        let dialog = adw::AlertDialog::new(Some(heading), Some(body));
        dialog.add_response("ok", "OK");
        dialog.present(Some(&self.window));
    }

    /// Starts QEMU and shows its screen.
    fn launch(self: &Rc<Self>, cfg: VmConfig) {
        let Some(qemu) = self.qemu.borrow().clone() else { return };
        let resume = self.paths.state.exists();
        vm::stop_leftover_vms();

        let sockets = match vm::Sockets::new() {
            Ok(s) => s,
            Err(e) => {
                self.set_phase(Phase::Idle);
                self.show_error("Couldn't Start MAN", "Something went wrong while starting.", &e.to_string());
                return;
            }
        };
        // A saved state can only be restored with normal graphics,
        // because QEMU can't save a VM that uses 3D acceleration.
        let gl = qemu.gl
            && !resume
            && self.settings.borrow().fast_graphics
            && !self.gl_failed.get();
        let args = vm::build_args(&cfg, &self.paths, &qemu, &sockets, resume, gl);

        let mut cmd = qemu.command(&qemu.system);
        cmd.args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        #[cfg(target_os = "linux")]
        unsafe {
            use std::os::unix::process::CommandExt;
            // If this app crashes, take QEMU down with it.
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                Ok(())
            });
        }
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                self.set_phase(Phase::Idle);
                sockets.cleanup();
                self.show_error("Couldn't Start MAN", "QEMU could not be started.", &e.to_string());
                return;
            }
        };

        // Keep the last part of QEMU's error output for error messages.
        let stderr = Arc::new(Mutex::new(String::new()));
        if let Some(pipe) = child.stderr.take() {
            let stderr = stderr.clone();
            std::thread::spawn(move || {
                use std::io::BufRead;
                for line in std::io::BufReader::new(pipe).lines().map_while(Result::ok) {
                    let mut s = stderr.lock().unwrap();
                    s.push_str(&line);
                    s.push('\n');
                    if s.len() > 8000 {
                        let cut = s.len() - 6000;
                        let cut = (cut..s.len()).find(|i| s.is_char_boundary(*i)).unwrap_or(0);
                        s.drain(..cut);
                    }
                }
            });
        }

        let id = self.session_counter.get() + 1;
        self.session_counter.set(id);
        let stop_flag = Arc::new(AtomicBool::new(false));
        let vnc_path = sockets.vnc.clone();
        let qmp_path = sockets.qmp.clone();
        *self.session.borrow_mut() = Some(Session {
            id,
            child,
            qemu,
            vnc: None,
            sockets,
            stderr,
            resuming: resume,
            shown_first_frame: false,
            stop_flag: stop_flag.clone(),
            cfg: cfg.clone(),
            gl,
        });

        self.set_phase(Phase::Running);
        self.after_exit.set(AfterExit::Nothing);
        self.display.set_paintable(None::<&gdk::Paintable>);
        self.display.set_cursor(None);
        self.display_loading.set_visible(true);
        self.display_loading_label.set_label(if resume {
            "Continuing where you left off…"
        } else {
            "Starting MAN…"
        });
        self.stack.set_visible_child_name("display");
        self.display.grab_focus();

        if vm::is_slow(cfg.arch) {
            self.toast("This computer can't speed up virtual computers, so MAN will be slow.");
        }

        // Connect to the screen.
        let (tx, rx) = async_channel::bounded(1);
        {
            let stop = stop_flag.clone();
            std::thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(60);
                while Instant::now() < deadline && !stop.load(Ordering::Relaxed) {
                    if vnc_path.ready() {
                        if let Ok(client) = VncClient::connect(&vnc_path) {
                            let _ = tx.send_blocking(client);
                            return;
                        }
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
            });
        }
        let this = self.clone();
        glib::spawn_future_local(async move {
            if let Ok(client) = rx.recv().await {
                match this.session.borrow_mut().as_mut() {
                    Some(s) if s.id == id => s.vnc = Some(client),
                    _ => client.shutdown(),
                }
            }
        });

        // A restored state must only be used once.
        if resume {
            let (tx, rx) = async_channel::bounded(1);
            let state = self.paths.state.clone();
            std::thread::spawn(move || {
                if vm::wait_until_running(&qmp_path, Duration::from_secs(300)) {
                    let _ = fs::remove_file(&state);
                    let _ = tx.send_blocking(());
                }
            });
            let this = self.clone();
            glib::spawn_future_local(async move {
                if rx.recv().await.is_ok() {
                    if let Some(s) = this.session.borrow_mut().as_mut() {
                        if s.id == id {
                            s.resuming = false;
                        }
                    }
                }
            });
        }

        let this = self.clone();
        glib::timeout_add_local(Duration::from_millis(16), move || this.tick(id));
    }

    /// Runs ~60 times a second while the VM is on: draws the screen and
    /// notices when QEMU exits.
    fn tick(self: &Rc<Self>, id: u64) -> glib::ControlFlow {
        let mut exited: Option<bool> = None;
        let mut first_frame = false;
        {
            let mut guard = self.session.borrow_mut();
            let Some(s) = guard.as_mut().filter(|s| s.id == id) else {
                return glib::ControlFlow::Break;
            };

            if let Some(vnc) = &s.vnc {
                if let Some((w, h, pixels)) = vnc.take_frame() {
                    let bytes = glib::Bytes::from_owned(pixels);
                    let texture = gdk::MemoryTexture::new(
                        w as i32,
                        h as i32,
                        gdk::MemoryFormat::B8g8r8x8,
                        &bytes,
                        (w * 4) as usize,
                    );
                    self.display.set_paintable(Some(&texture));
                    if !s.shown_first_frame {
                        s.shown_first_frame = true;
                        first_frame = true;
                    }
                }
                if let Some(c) = vnc.take_cursor() {
                    let cursor = if c.width == 0 || c.height == 0 {
                        gdk::Cursor::from_name("none", None)
                    } else {
                        let bytes = glib::Bytes::from_owned(c.rgba);
                        let texture = gdk::MemoryTexture::new(
                            c.width as i32,
                            c.height as i32,
                            gdk::MemoryFormat::R8g8b8a8,
                            &bytes,
                            (c.width * 4) as usize,
                        );
                        Some(gdk::Cursor::from_texture(&texture, c.hot_x as i32, c.hot_y as i32, None))
                    };
                    self.display.set_cursor(cursor.as_ref());
                }
                if vnc.is_closed() {
                    s.vnc = None;
                }
            }

            match s.child.try_wait() {
                Ok(Some(status)) => exited = Some(status.success()),
                Ok(None) => {}
                Err(_) => exited = Some(false),
            }
        }

        if first_frame {
            self.display_loading.set_visible(false);
            if !self.shown_click_hint.replace(true) {
                self.toast("Click inside MAN to use your mouse and keyboard there.");
            }
        }

        if let Some(success) = exited {
            self.on_exit(success);
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    }

    fn on_exit(self: &Rc<Self>, success: bool) {
        let Some(session) = self.session.borrow_mut().take() else { return };
        let resuming = session.resuming;
        let log = session.stderr.lock().unwrap().clone();
        let gl_never_started = session.gl && !session.shown_first_frame;
        let cfg = session.cfg.clone();
        drop(session);

        let phase = self.phase.get();
        self.set_phase(Phase::Idle);
        self.display.set_paintable(None::<&gdk::Paintable>);
        self.display.set_cursor(None);
        self.mouse_buttons.set(0);
        self.keys_down.borrow_mut().clear();

        match self.after_exit.replace(AfterExit::Nothing) {
            AfterExit::Close => {
                self.window.close();
                return;
            }
            AfterExit::DeleteAndClose => {
                self.delete_files();
                self.window.close();
                return;
            }
            AfterExit::Delete => {
                self.delete_files();
                self.show_welcome();
                self.toast("MAN was deleted from this computer");
                return;
            }
            AfterExit::DeleteAndRestart => {
                self.delete_files();
                self.start();
                return;
            }
            AfterExit::Nothing => {}
        }

        if phase == Phase::Running && !success && gl_never_started {
            // 3D graphics didn't work here; quietly use normal graphics.
            self.gl_failed.set(true);
            self.toast("Fast 3D graphics didn't work on this computer, so MAN uses normal graphics.");
            self.launch(cfg);
            return;
        }

        if phase == Phase::Running && resuming && !success {
            let _ = fs::remove_file(&self.paths.state);
            self.show_error(
                "Couldn't Continue Where You Left Off",
                "The saved session couldn't be opened, so it was removed.\nPress Try Again to start MAN normally.",
                &log,
            );
        } else if phase == Phase::Running && !success {
            self.show_error(
                "MAN Stopped Unexpectedly",
                "The virtual computer stopped working. Press Try Again to start it again.",
                &log,
            );
        } else {
            self.show_welcome();
            self.toast("MAN was turned off");
        }
    }

    // -----------------------------------------------------------------------
    // Stopping, saving, deleting

    fn ask_stop(self: &Rc<Self>) {
        if self.phase.get() != Phase::Running {
            return;
        }
        let dialog = adw::AlertDialog::new(
            Some("Turn Off MAN?"),
            Some("This is like pulling the power plug. Anything you didn't save inside MAN will be lost."),
        );
        dialog.add_responses(&[("cancel", "Cancel"), ("stop", "Turn Off")]);
        dialog.set_response_appearance("stop", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let this = self.clone();
        dialog.connect_response(None, move |_, response| {
            if response == "stop" {
                this.turn_off(AfterExit::Nothing);
            }
        });
        dialog.present(Some(&self.window));
    }

    fn turn_off(self: &Rc<Self>, after: AfterExit) {
        let Some((qmp, id)) = self
            .session
            .borrow()
            .as_ref()
            .map(|s| (s.sockets.qmp.clone(), s.id))
        else {
            return;
        };
        self.after_exit.set(after);
        self.set_phase(Phase::Stopping);
        self.show_busy("Turning Off MAN…", "This only takes a moment.");
        std::thread::spawn(move || {
            let _ = vm::quit(&qmp);
        });
        self.kill_later(id, Duration::from_secs(5));
    }

    /// Safety net in case QEMU doesn't react.
    fn kill_later(self: &Rc<Self>, id: u64, delay: Duration) {
        let this = self.clone();
        glib::timeout_add_local_once(delay, move || {
            if let Some(s) = this.session.borrow_mut().as_mut() {
                if s.id == id {
                    let _ = s.child.kill();
                }
            }
        });
    }

    fn save_and_close(self: &Rc<Self>) {
        let Some((qmp, qemu, id)) = self
            .session
            .borrow()
            .as_ref()
            .map(|s| (s.sockets.qmp.clone(), s.qemu.clone(), s.id))
        else {
            self.window.close();
            return;
        };
        self.set_phase(Phase::Saving);
        self.after_exit.set(AfterExit::Close);
        self.show_busy(
            "Saving MAN…",
            "Next time, MAN will continue right where you left off.",
        );

        let (tx, rx) = async_channel::bounded(1);
        let state = self.paths.state.clone();
        std::thread::spawn(move || {
            let _ = tx.send_blocking(vm::save_state(&qmp, &qemu, &state));
        });

        let this = self.clone();
        glib::spawn_future_local(async move {
            let Ok(result) = rx.recv().await else { return };
            let still_running = this.session.borrow().as_ref().is_some_and(|s| s.id == id);
            match result {
                Ok(()) => this.kill_later(id, Duration::from_secs(15)),
                Err(e) if still_running => {
                    this.after_exit.set(AfterExit::Nothing);
                    this.set_phase(Phase::Running);
                    this.stack.set_visible_child_name("display");
                    let dialog = adw::AlertDialog::new(
                        Some("Couldn't Save MAN"),
                        Some(&format!(
                            "MAN is still running. You can keep using it, or close without saving.\n\n{e}"
                        )),
                    );
                    dialog.add_responses(&[("keep", "Keep Using MAN"), ("close", "Close Without Saving")]);
                    dialog.set_response_appearance("close", adw::ResponseAppearance::Destructive);
                    dialog.set_default_response(Some("keep"));
                    dialog.set_close_response("keep");
                    let this2 = this.clone();
                    dialog.connect_response(None, move |_, r| {
                        if r == "close" {
                            this2.turn_off(AfterExit::Close);
                        }
                    });
                    dialog.present(Some(&this.window));
                }
                Err(_) => {}
            }
        });
    }

    fn on_close_request(self: &Rc<Self>) -> glib::Propagation {
        match self.phase.get() {
            Phase::Idle => glib::Propagation::Proceed,
            Phase::Preparing => {
                // The partial download is kept and continued next time.
                self.cancel_download.borrow().store(true, Ordering::Relaxed);
                glib::Propagation::Proceed
            }
            Phase::Saving | Phase::Stopping => {
                self.toast("Please wait a moment…");
                glib::Propagation::Stop
            }
            Phase::Running => {
                self.ask_before_closing();
                glib::Propagation::Stop
            }
        }
    }

    fn ask_before_closing(self: &Rc<Self>) {
        let gl = self.session.borrow().as_ref().is_some_and(|s| s.gl);
        let dialog = if gl {
            // QEMU can't save a VM while it uses 3D acceleration.
            let d = adw::AlertDialog::new(
                Some("Close MAN?"),
                Some(
                    "Turn Off shuts MAN down. Programs you installed inside MAN are kept.\n\n\
                     Delete removes MAN and everything in it from this computer.\n\n\
                     Saving isn't possible while Fast 3D Graphics is on. \
                     You can switch it off in the menu.",
                ),
            );
            d.add_responses(&[("cancel", "Cancel"), ("delete", "Delete"), ("off", "Turn Off")]);
            d.set_response_appearance("off", adw::ResponseAppearance::Suggested);
            d.set_default_response(Some("off"));
            d
        } else {
            let d = adw::AlertDialog::new(
                Some("Save MAN for Next Time?"),
                Some(
                    "Save keeps everything exactly as it is now, so you can continue where you left off.\n\n\
                     Turn Off shuts MAN down. Programs you installed inside MAN are kept.\n\n\
                     Delete removes MAN and everything in it from this computer.",
                ),
            );
            d.add_responses(&[
                ("cancel", "Cancel"),
                ("delete", "Delete"),
                ("off", "Turn Off"),
                ("save", "Save"),
            ]);
            d.set_response_appearance("save", adw::ResponseAppearance::Suggested);
            d.set_default_response(Some("save"));
            d
        };
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_close_response("cancel");
        let this = self.clone();
        dialog.connect_response(None, move |_, response| match response {
            "save" => this.save_and_close(),
            "off" => this.turn_off(AfterExit::Close),
            "delete" => this.turn_off(AfterExit::DeleteAndClose),
            _ => {}
        });
        dialog.present(Some(&self.window));
    }

    fn delete_files(&self) {
        let _ = fs::remove_dir_all(&self.paths.dir);
        let _ = fs::remove_dir_all(&self.paths.iso_dir);
        self.banner.set_revealed(false);
    }

    fn ask_delete(self: &Rc<Self>) {
        match self.phase.get() {
            Phase::Preparing | Phase::Saving | Phase::Stopping => {
                self.toast("Please wait a moment and try again.");
                return;
            }
            _ => {}
        }
        if !self.paths.dir.exists() && !self.paths.iso_dir.exists() {
            self.toast("MAN isn't on this computer yet.");
            return;
        }
        let dialog = adw::AlertDialog::new(
            Some("Delete MAN From This Computer?"),
            Some(
                "This removes the MAN virtual computer and its download, \
                 including anything you saved inside it. You can always download MAN again.",
            ),
        );
        dialog.add_responses(&[("cancel", "Cancel"), ("delete", "Delete")]);
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let this = self.clone();
        dialog.connect_response(None, move |_, response| {
            if response != "delete" {
                return;
            }
            if this.session.borrow().is_some() {
                this.turn_off(AfterExit::Delete);
            } else {
                this.delete_files();
                this.show_welcome();
                this.toast("MAN was deleted from this computer");
            }
        });
        dialog.present(Some(&self.window));
    }

    // -----------------------------------------------------------------------
    // New versions

    fn check_for_new_version(self: &Rc<Self>) {
        let Some(arch) = self.arch else { return };
        let Ok(cfg) = vm::load_config(&self.paths) else { return };
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send_blocking(release::latest(arch));
        });
        let this = self.clone();
        glib::spawn_future_local(async move {
            if let Ok(Ok(rel)) = rx.recv().await {
                let current = cfg.iso.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if rel.iso_name != current {
                    this.banner.set_title(&format!("A new version is available: {}", rel.name));
                    this.banner.set_revealed(true);
                }
            }
        });
    }

    fn ask_update(self: &Rc<Self>) {
        let dialog = adw::AlertDialog::new(
            Some("Get the New Version?"),
            Some(
                "Your current MAN virtual computer will be deleted, including anything saved inside it. \
                 Then the newest MAN will be downloaded and started.",
            ),
        );
        dialog.add_responses(&[("cancel", "Cancel"), ("update", "Get New Version")]);
        dialog.set_response_appearance("update", adw::ResponseAppearance::Suggested);
        dialog.set_close_response("cancel");
        let this = self.clone();
        dialog.connect_response(None, move |_, response| {
            if response != "update" {
                return;
            }
            match this.phase.get() {
                Phase::Running => this.turn_off(AfterExit::DeleteAndRestart),
                Phase::Idle => {
                    this.delete_files();
                    this.start();
                }
                _ => this.toast("Please wait a moment and try again."),
            }
        });
        dialog.present(Some(&self.window));
    }

    fn show_about(&self) {
        let about = adw::AboutDialog::builder()
            .application_name("Try MAN")
            .application_icon(crate::APP_ID)
            .developer_name("MAN Corporation")
            .version(env!("CARGO_PKG_VERSION_MAJOR"))
            .website("https://github.com/MANCorporation/MAN")
            .issue_url("https://github.com/MANCorporation/MAN/issues")
            .comments("Try MAN in a virtual computer, without changing anything on your computer.")
            .build();
        about.present(Some(&self.window));
    }
}

fn mouse_bit(gdk_button: u32) -> u8 {
    match gdk_button {
        1 => 1,      // left
        2 => 1 << 1, // middle
        3 => 1 << 2, // right
        _ => 0,
    }
}

/// GDK key values are X11 keysyms, which is what VNC wants.
fn keysym(key: gdk::Key) -> u32 {
    match key.into_glib() {
        0xfe20 => 0xff09, // ISO_Left_Tab (Shift+Tab) -> Tab
        other => other,
    }
}
