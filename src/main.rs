mod hand;
mod view;

use gtk4::gdk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use hand::Hand;
use std::cell::{Cell, OnceCell, RefCell};
use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;
use view::PointerView;

const USAGE: &str = "\
Usage: tinyhand-linux [COMMAND]

Commands (sent to the running instance, starting one if needed):
  --on       Turn the pointer on (default)
  --off      Turn the pointer off and clear the drawing
  --toggle   Turn the pointer on or off
  --clear    Clear the drawing
  --quit     Stop the running instance
  --help     Show this help
";

#[derive(Clone, Copy, PartialEq)]
enum Cmd {
    On,
    Off,
    Toggle,
    Clear,
    Quit,
}

fn parse(args: &[String]) -> Result<Cmd, String> {
    match args {
        [] => Ok(Cmd::On),
        [arg] => match arg.as_str() {
            "--on" => Ok(Cmd::On),
            "--off" => Ok(Cmd::Off),
            "--toggle" => Ok(Cmd::Toggle),
            "--clear" => Ok(Cmd::Clear),
            "--quit" => Ok(Cmd::Quit),
            other => Err(format!("unknown argument: {other}")),
        },
        _ => Err("expected at most one command".into()),
    }
}

fn asset_path(name: &str) -> PathBuf {
    if let Ok(dir) = std::env::var("TINYHAND_ASSETS") {
        return PathBuf::from(dir).join(name);
    }
    let project = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(name);
    if project.exists() {
        return project;
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home)
            .join(".local/share/tinyhand-linux")
            .join(name);
    }
    project
}

pub fn play_tap() {
    if let Ok(mut child) = Command::new("pw-play").arg(asset_path("tap_plastic.wav")).spawn() {
        std::thread::spawn(move || child.wait());
    }
}

/// One full-screen overlay per monitor. The hand shows on whichever one the
/// pointer is over; the compositor's own cursor is hidden on all of them.
struct Overlay {
    app: Application,
    hand: Option<Rc<Hand>>,
    windows: RefCell<Vec<(ApplicationWindow, PointerView)>>,
    active: Cell<bool>,
    _hold: gio::ApplicationHoldGuard,
}

impl Overlay {
    fn new(app: &Application) -> Rc<Self> {
        let hand = match Hand::load(&asset_path("hand_pastel.png")) {
            Ok(hand) => Some(Rc::new(hand)),
            Err(e) => {
                eprintln!("tinyhand-linux: {e}");
                None
            }
        };

        let display = gdk::Display::default().expect("no display");
        let css = gtk4::CssProvider::new();
        css.load_from_string("window.tinyhand { background: transparent; }");
        gtk4::style_context_add_provider_for_display(
            &display,
            &css,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        let overlay = Rc::new(Overlay {
            app: app.clone(),
            hand,
            windows: RefCell::new(Vec::new()),
            active: Cell::new(false),
            // Stay running while the pointer is off so --toggle can bring it back.
            _hold: app.hold(),
        });
        overlay.rebuild();
        let weak = Rc::downgrade(&overlay);
        display.monitors().connect_items_changed(move |_, _, _, _| {
            if let Some(overlay) = weak.upgrade() {
                overlay.rebuild();
            }
        });
        overlay
    }

    fn rebuild(&self) {
        for (window, _) in self.windows.borrow_mut().drain(..) {
            window.destroy();
        }
        let display = gdk::Display::default().expect("no display");
        let monitors = display.monitors();
        let mut windows = self.windows.borrow_mut();
        for monitor in monitors.iter::<gdk::Monitor>().flatten() {
            let window = ApplicationWindow::builder().application(&self.app).build();
            window.add_css_class("tinyhand");
            window.set_decorated(false);
            window.init_layer_shell();
            window.set_layer(Layer::Overlay);
            window.set_namespace(Some("tinyhand-linux"));
            window.set_monitor(Some(&monitor));
            window.set_exclusive_zone(-1);
            for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
                window.set_anchor(edge, true);
            }
            // Never take keyboard focus, so keys keep going to the presentation.
            window.set_keyboard_mode(KeyboardMode::None);

            let view = PointerView::new(self.hand.clone());
            window.set_child(Some(&view));
            if self.active.get() {
                window.present();
            }
            windows.push((window, view));
        }
    }

    fn set_active(&self, on: bool) {
        self.active.set(on);
        for (window, view) in self.windows.borrow().iter() {
            view.reset();
            if on {
                window.present();
            } else {
                window.set_visible(false);
            }
        }
    }

    fn clear(&self) {
        for (_, view) in self.windows.borrow().iter() {
            view.clear_ink();
        }
    }
}

fn main() -> glib::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{USAGE}");
        return glib::ExitCode::SUCCESS;
    }
    if let Err(e) = parse(&args) {
        eprint!("tinyhand-linux: {e}\n\n{USAGE}");
        return glib::ExitCode::FAILURE;
    }

    let app = Application::builder()
        .application_id("dev.atjhsu.TinyHandLinux")
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

    let overlay: Rc<OnceCell<Rc<Overlay>>> = Rc::default();
    app.connect_command_line(move |app, cmdline| {
        let args: Vec<String> = cmdline
            .arguments()
            .iter()
            .skip(1)
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let Ok(cmd) = parse(&args) else {
            return glib::ExitCode::FAILURE;
        };
        // Commands that only affect a running pointer do nothing on a fresh start.
        let running = overlay.get().cloned();
        match cmd {
            Cmd::On => overlay.get_or_init(|| Overlay::new(app)).set_active(true),
            Cmd::Toggle => {
                let o = overlay.get_or_init(|| Overlay::new(app));
                o.set_active(!(running.is_some() && o.active.get()));
            }
            Cmd::Off => {
                if let Some(o) = running {
                    o.set_active(false);
                }
            }
            Cmd::Clear => {
                if let Some(o) = running {
                    o.clear();
                }
            }
            Cmd::Quit => app.quit(),
        }
        glib::ExitCode::SUCCESS
    });

    app.run()
}
