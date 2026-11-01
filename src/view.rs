//! The overlay widget: a hand on a rod held from below the screen, aimed so
//! the fingertip sits on the pointer, plus fading ink strokes.
//!
//! Geometry and timing follow TinyHand's `ArmView`, `Ink` and
//! `OverlayController`, converted to GTK's y-down coordinates.

use crate::hand::Hand;
use gtk4::gdk;
use gtk4::glib;
use gtk4::graphene;
use gtk4::gsk;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use std::rc::Rc;

/// Hand length (wrist to fingertip) as a fraction of the screen height.
const HAND_LEN: f64 = 0.13;
/// The rod is held this far below the bottom edge, as a fraction of screen height.
const GRIP_DEPTH: f64 = 0.12;
/// The grip sits this far right of the pointer, as a fraction of screen width.
const GRIP_LEAN: f64 = 0.30;
/// Per-tick (at 120 Hz) fraction of the distance the grip moves toward its target.
const GRIP_FOLLOW: f64 = 0.08;
/// Per-tick (at 120 Hz) decay of the press animation once released.
const PRESS_DECAY: f64 = 0.18;
const MIN_ROD: f64 = 0.05;
const LIFT: f64 = 0.06;

const INK_COLOR: gdk::RGBA = gdk::RGBA::new(0.53, 0.33, 0.95, 1.0);
const INK_START_DISTANCE: f64 = 4.0;
const INK_MIN_STEP: f64 = 1.5;
const INK_FADE_DELAY: f64 = 3.0;
const INK_FADE_DURATION: f64 = 0.6;

struct Stroke {
    points: Vec<(f64, f64)>,
    width: f64,
    ended_at: Option<f64>,
}

impl Stroke {
    fn opacity(&self, now: f64) -> f64 {
        let Some(ended) = self.ended_at else {
            return 1.0;
        };
        let t = ((now - ended - INK_FADE_DELAY) / INK_FADE_DURATION).clamp(0.0, 1.0);
        1.0 - t * t
    }

    fn faded(&self, now: f64) -> bool {
        self.ended_at
            .is_some_and(|ended| now - ended >= INK_FADE_DELAY + INK_FADE_DURATION)
    }
}

#[derive(Default)]
pub struct State {
    hand: Option<Rc<Hand>>,
    /// `None` while the pointer is on another output.
    pointer: Option<(f64, f64)>,
    grip_x: Option<f64>,
    press: f64,
    held: bool,
    /// Where the left button went down; drawing starts once the pointer moves away.
    down: Option<(f64, f64)>,
    strokes: Vec<Stroke>,
    drawing: bool,
    last_frame: Option<i64>,
}

fn now() -> f64 {
    glib::monotonic_time() as f64 / 1e6
}

mod imp {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    pub struct PointerView {
        pub state: RefCell<State>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PointerView {
        const NAME: &'static str = "TinyHandPointerView";
        type Type = super::PointerView;
        type ParentType = gtk4::Widget;
    }

    impl ObjectImpl for PointerView {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_hexpand(true);
            obj.set_vexpand(true);
            obj.set_cursor_from_name(Some("none"));

            let events = gtk4::EventControllerLegacy::new();
            events.connect_event(glib::clone!(
                #[weak]
                obj,
                #[upgrade_or]
                glib::Propagation::Proceed,
                move |_, event| obj.handle_event(event)
            ));
            obj.add_controller(events);

            // Crossing events are delivered to motion controllers, not legacy ones.
            let crossing = gtk4::EventControllerMotion::new();
            crossing.connect_enter(glib::clone!(
                #[weak]
                obj,
                move |_, x, y| obj.moved(x, y)
            ));
            crossing.connect_leave(glib::clone!(
                #[weak]
                obj,
                move |_| obj.left()
            ));
            obj.add_controller(crossing);

            obj.add_tick_callback(|view, clock| {
                view.tick(clock.frame_time());
                glib::ControlFlow::Continue
            });
        }
    }

    impl WidgetImpl for PointerView {
        fn snapshot(&self, snapshot: &gtk4::Snapshot) {
            let obj = self.obj();
            let s = self.state.borrow();
            // GTK skips attaching a buffer when nothing is drawn, which leaves the
            // layer surface unmapped: no input, no frames, so the hand never appears.
            let bounds = graphene::Rect::new(0.0, 0.0, obj.width() as f32, obj.height() as f32);
            snapshot.append_color(&gdk::RGBA::TRANSPARENT, &bounds);
            draw_ink(snapshot, &s, now());
            if let (Some(hand), Some(pointer), Some(grip_x)) = (&s.hand, s.pointer, s.grip_x) {
                draw_hand(snapshot, hand, pointer, grip_x, s.press, obj.height() as f64);
            }
        }
    }
}

glib::wrapper! {
    pub struct PointerView(ObjectSubclass<imp::PointerView>)
        @extends gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl PointerView {
    pub fn new(hand: Option<Rc<Hand>>) -> Self {
        let view: Self = glib::Object::new();
        view.imp().state.borrow_mut().hand = hand;
        view
    }

    /// Drops the ink and any press in progress, e.g. when the overlay turns off.
    pub fn reset(&self) {
        let mut s = self.imp().state.borrow_mut();
        s.strokes.clear();
        s.drawing = false;
        s.down = None;
        s.held = false;
        s.press = 0.0;
        s.pointer = None;
        s.grip_x = None;
        drop(s);
        self.queue_draw();
    }

    pub fn clear_ink(&self) {
        let mut s = self.imp().state.borrow_mut();
        s.strokes.clear();
        s.drawing = false;
        drop(s);
        self.queue_draw();
    }

    fn handle_event(&self, event: &gdk::Event) -> glib::Propagation {
        let position = event.position();
        match event.event_type() {
            gdk::EventType::MotionNotify => {
                if let Some((x, y)) = position {
                    self.moved(x, y);
                }
            }
            gdk::EventType::ButtonPress => {
                let button = event.downcast_ref::<gdk::ButtonEvent>().map(|b| b.button());
                if let Some((x, y)) = position {
                    self.pressed(x, y, button == Some(gdk::BUTTON_PRIMARY));
                }
            }
            gdk::EventType::ButtonRelease => self.released(),
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    }

    fn moved(&self, x: f64, y: f64) {
        let mut s = self.imp().state.borrow_mut();
        s.pointer = Some((x, y));
        if let Some(start) = s.down {
            if !s.drawing && (x - start.0).hypot(y - start.1) >= INK_START_DISTANCE {
                let width = (self.height() as f64 * 0.0055).max(3.0);
                s.strokes.push(Stroke { points: vec![start], width, ended_at: None });
                s.drawing = true;
            }
            if s.drawing {
                let stroke = s.strokes.last_mut().expect("drawing stroke");
                let last = *stroke.points.last().expect("stroke has a start");
                if (x - last.0).hypot(y - last.1) >= INK_MIN_STEP {
                    stroke.points.push((x, y));
                }
            }
        }
        drop(s);
        self.queue_draw();
    }

    fn left(&self) {
        let mut s = self.imp().state.borrow_mut();
        if !s.held {
            s.pointer = None;
        }
        drop(s);
        self.queue_draw();
    }

    /// Every button is a tap: it never reaches the app underneath.
    fn pressed(&self, x: f64, y: f64, primary: bool) {
        let mut s = self.imp().state.borrow_mut();
        s.pointer = Some((x, y));
        s.press = 1.0;
        s.held = true;
        if primary {
            s.down = Some((x, y));
        }
        drop(s);
        crate::play_tap();
        self.queue_draw();
    }

    fn released(&self) {
        let mut s = self.imp().state.borrow_mut();
        s.held = false;
        s.down = None;
        if s.drawing {
            s.drawing = false;
            if let Some(stroke) = s.strokes.last_mut() {
                stroke.ended_at = Some(now());
            }
        }
    }

    fn tick(&self, frame_time: i64) {
        let width = self.width() as f64;
        let mut s = self.imp().state.borrow_mut();
        let dt = s
            .last_frame
            .map_or(0.0, |last| ((frame_time - last) as f64 / 1e6).clamp(0.0, 0.1));
        s.last_frame = Some(frame_time);
        // The original tuned its easing at 120 ticks a second; scale to the real frame rate.
        let ease = |rate: f64| 1.0 - (1.0 - rate).powf(dt * 120.0);
        let mut redraw = false;

        if let Some((x, _)) = s.pointer {
            let target = (x + GRIP_LEAN * width).clamp(0.15 * width, 1.05 * width);
            let gx = s.grip_x.map_or(target, |gx| gx + (target - gx) * ease(GRIP_FOLLOW));
            redraw |= s.grip_x.is_none_or(|old| (old - gx).abs() > 0.05);
            s.grip_x = Some(gx);
        }

        if !s.held && s.press > 0.0 {
            s.press -= s.press * ease(PRESS_DECAY);
            if s.press < 0.002 {
                s.press = 0.0;
            }
            redraw = true;
        }

        let now = now();
        let before = s.strokes.len();
        s.strokes.retain(|stroke| !stroke.faded(now));
        redraw |= s.strokes.len() != before
            || s.strokes.iter().any(|stroke| stroke.ended_at.is_some_and(|e| now - e >= INK_FADE_DELAY));

        drop(s);
        if redraw {
            self.queue_draw();
        }
    }
}

fn draw_ink(snapshot: &gtk4::Snapshot, s: &State, now: f64) {
    if s.strokes.is_empty() {
        return;
    }
    let shadow = gsk::Shadow::new(gdk::RGBA::new(0.0, 0.0, 0.0, 0.18), 1.0, 1.5, 3.0);
    snapshot.push_shadow(&[shadow]);
    for stroke in &s.strokes {
        let Some(&(x0, y0)) = stroke.points.first() else {
            continue;
        };
        // Smooth through the midpoints between samples, as TinyHand does.
        let path = gsk::PathBuilder::new();
        path.move_to(x0 as f32, y0 as f32);
        if stroke.points.len() == 1 {
            path.line_to(x0 as f32, y0 as f32);
        }
        for pair in stroke.points.windows(2) {
            let ((px, py), (x, y)) = (pair[0], pair[1]);
            path.quad_to(px as f32, py as f32, ((px + x) / 2.0) as f32, ((py + y) / 2.0) as f32);
        }
        if let Some(&(x, y)) = stroke.points.last().filter(|_| stroke.points.len() > 1) {
            path.line_to(x as f32, y as f32);
        }
        let style = gsk::Stroke::new(stroke.width as f32);
        style.set_line_cap(gsk::LineCap::Round);
        style.set_line_join(gsk::LineJoin::Round);

        let opacity = stroke.opacity(now);
        if opacity < 1.0 {
            snapshot.push_opacity(opacity);
        }
        snapshot.append_stroke(&path.to_path(), &style, &INK_COLOR);
        if opacity < 1.0 {
            snapshot.pop();
        }
    }
    snapshot.pop();
}

/// Draws the hand so its fingertip lands on `pointer`, with the rod running
/// down to a grip below the bottom of the screen.
fn draw_hand(
    snapshot: &gtk4::Snapshot,
    hand: &Hand,
    pointer: (f64, f64),
    grip_x: f64,
    press: f64,
    height: f64,
) {
    let img_w = hand.width as f64;
    let img_h = hand.height as f64;
    let grip = (grip_x, height * (1.0 + GRIP_DEPTH));

    let hand_len = HAND_LEN * height * (1.0 - 0.06 * press);
    // Image pixels to screen points for the hand part.
    let scale = hand_len / ((hand.wrist - hand.hand_top) as f64 * img_h);
    let hand_w = img_w * scale;
    let tip_len = (hand.wrist - hand.tip) as f64 * img_h * scale;
    let tip_off_x = (hand.tip_x - hand.rod_x) as f64 * hand_w;

    // In the rod's frame (origin at the grip, rod pointing to -y) the tip is at
    // (tip_off_x, -(rod_len + tip_len)). Stretch the rod and rotate so it reaches.
    let (dx, dy) = (pointer.0 - grip.0, pointer.1 - grip.1);
    let reach = dx.hypot(dy);
    let along = (reach * reach - tip_off_x * tip_off_x).max(0.0).sqrt();
    let rod_len = (along - tip_len).max(MIN_ROD * height);
    let rotation = dy.atan2(dx) - (-(rod_len + tip_len)).atan2(tip_off_x);

    let lift = LIFT * height * (1.0 - 0.8 * press);
    let shadow = gsk::Shadow::new(
        gdk::RGBA::new(0.0, 0.0, 0.0, (0.26 + 0.02 * press) as f32),
        (lift * 0.6) as f32,
        lift as f32,
        ((lift * 0.35).max(1.0) + 5.0 * press) as f32,
    );

    let left = -(hand.rod_x as f64) * hand_w;
    let wrist_px = hand.wrist as f64 * img_h;
    let image = graphene::Rect::new(0.0, 0.0, hand.width, hand.height);
    // Draw the image with `scale_y`, its wrist row at local y = -rod_len, clipped to `clip`.
    let part = |clip: graphene::Rect, scale_y: f64| {
        snapshot.push_clip(&clip);
        snapshot.save();
        snapshot.translate(&graphene::Point::new(left as f32, (-rod_len - wrist_px * scale_y) as f32));
        snapshot.scale(scale as f32, scale_y as f32);
        snapshot.append_scaled_texture(&hand.texture, gsk::ScalingFilter::Trilinear, &image);
        snapshot.restore();
        snapshot.pop();
    };

    snapshot.save();
    snapshot.push_shadow(&[shadow]);
    snapshot.translate(&graphene::Point::new(grip.0 as f32, grip.1 as f32));
    snapshot.rotate(rotation.to_degrees() as f32);
    part(
        graphene::Rect::new(left as f32, (-rod_len - hand_len) as f32, hand_w as f32, hand_len as f32),
        scale,
    );
    let rod_px = ((hand.rod_end - hand.wrist) as f64 * img_h).max(1.0);
    part(
        graphene::Rect::new(left as f32, -rod_len as f32, hand_w as f32, rod_len as f32),
        rod_len / rod_px,
    );
    snapshot.pop();
    snapshot.restore();
}
