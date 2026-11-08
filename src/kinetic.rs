//! Kinetic scrolling: a flick on a touchpad keeps the page moving after the
//! fingers leave it, slowing down, as GTK and the browsers do. egui scrolls
//! only while the fingers move; the system reports when they lift (the
//! wheel event's `End` phase), and from there this module keeps feeding
//! decaying scroll events.
//!
//! Flicks in a row add up, as in the browsers: a new flick in the same
//! direction while the page still glides keeps that speed and adds its own,
//! so repeated flicks travel far instead of restarting from rest each time.

use egui::{Event, MouseWheelUnit, RawInput, TouchPhase, Vec2};

/// How fast the glide slows down: the speed falls to 1/e in this time.
const DECAY: f64 = 0.35;
/// Below this speed (points per second) a flick does not glide.
const MIN_SPEED: f32 = 120.0;
/// Only the movement of the last moment before the lift sets the speed.
const WINDOW: f64 = 0.1;
/// A flick that starts this soon after the glide it interrupts adds to it.
const BOOST_WINDOW: f64 = 0.5;
/// The fastest glide, in points per second: a few flicks in a row must not
/// jump whole chapters.
const MAX_SPEED: f32 = 12_000.0;

#[derive(Default)]
pub struct Kinetic {
    /// The latest finger movements: (time, delta).
    samples: Vec<(f64, Vec2)>,
    /// The glide's speed in points per second, and when it last moved.
    glide: Option<(Vec2, f64)>,
    /// The glide a new flick interrupted, and when: added to that flick's
    /// speed if it goes the same way.
    carried: Option<(Vec2, f64)>,
}

impl Kinetic {
    /// Looks at this frame's input and, while gliding, adds its scroll.
    pub fn feed(&mut self, ctx: &egui::Context, input: &mut RawInput) {
        let now = input.time.unwrap_or_else(|| ctx.input(|i| i.time));
        let mut lifted = false;
        let mut modifiers = egui::Modifiers::default();
        for event in &input.events {
            match event {
                Event::MouseWheel { unit: MouseWheelUnit::Point, delta, phase, modifiers: m } => {
                    modifiers = *m;
                    // Fingers back on the pad stop the glide; its speed is
                    // kept for a flick that follows.
                    if let Some((velocity, _)) = self.glide.take() {
                        self.carried = Some((velocity, now));
                    }
                    match phase {
                        TouchPhase::Start => self.samples.clear(),
                        TouchPhase::Move => self.samples.push((now, *delta)),
                        TouchPhase::End => lifted = true,
                        TouchPhase::Cancel => self.samples.clear(),
                    }
                }
                // A mouse wheel, a click or a key stops the glide.
                Event::MouseWheel { .. } | Event::PointerButton { pressed: true, .. } | Event::Key { pressed: true, .. } => {
                    self.glide = None;
                    self.carried = None;
                    self.samples.clear();
                }
                _ => {}
            }
        }
        self.samples.retain(|(t, _)| now - t <= WINDOW);
        if lifted {
            let carried = self.carried.take().filter(|(_, t)| now - t <= BOOST_WINDOW);
            if let Some(mut velocity) = velocity(&self.samples, now) {
                if let Some((before, _)) = carried.filter(|(v, _)| v.dot(velocity) > 0.0) {
                    velocity += before;
                }
                if velocity.length() > MAX_SPEED {
                    velocity = velocity.normalized() * MAX_SPEED;
                }
                self.glide = Some((velocity, now));
            }
            self.samples.clear();
        }
        let Some((velocity, last)) = self.glide else { return };
        let dt = (now - last).clamp(0.0, 0.1);
        let decay = (-dt / DECAY).exp() as f32;
        let velocity = velocity * decay;
        if velocity.length() < MIN_SPEED / 4.0 {
            self.glide = None;
            return;
        }
        // The distance covered since the last frame at the decaying speed.
        let step = velocity * (DECAY as f32) * (1.0 / decay - 1.0);
        input.events.push(Event::MouseWheel { unit: MouseWheelUnit::Point, delta: step, phase: TouchPhase::Move, modifiers });
        self.glide = Some((velocity, now));
        ctx.request_repaint();
    }
}

/// The speed of the movement in `samples`, if it is a flick.
fn velocity(samples: &[(f64, Vec2)], now: f64) -> Option<Vec2> {
    let first = samples.first()?.0;
    let span = (now - first).max(1.0 / 60.0) as f32;
    let total = samples.iter().fold(Vec2::ZERO, |sum, (_, d)| sum + *d);
    let v = total / span;
    (samples.len() >= 2 && v.length() >= MIN_SPEED).then_some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wheel(delta: f32, phase: TouchPhase) -> Event {
        Event::MouseWheel { unit: MouseWheelUnit::Point, delta: egui::vec2(0.0, delta), phase, modifiers: Default::default() }
    }

    fn frame(k: &mut Kinetic, ctx: &egui::Context, time: f64, events: Vec<Event>) -> f32 {
        let mut input = RawInput { time: Some(time), events, ..Default::default() };
        let before = input.events.len();
        k.feed(ctx, &mut input);
        input.events[before..].iter().map(|e| if let Event::MouseWheel { delta, .. } = e { delta.y } else { 0.0 }).sum()
    }

    #[test]
    fn a_flick_glides_and_slows_down() {
        let ctx = egui::Context::default();
        let mut k = Kinetic::default();
        frame(&mut k, &ctx, 0.0, vec![wheel(0.0, TouchPhase::Start)]);
        for i in 1..=5 {
            frame(&mut k, &ctx, i as f64 / 60.0, vec![wheel(-20.0, TouchPhase::Move)]);
        }
        let mut t = 6.0 / 60.0;
        let first = frame(&mut k, &ctx, t, vec![wheel(0.0, TouchPhase::End)]);
        assert_eq!(first, 0.0, "the lift itself adds nothing");
        let mut steps = Vec::new();
        for _ in 0..200 {
            t += 1.0 / 60.0;
            steps.push(frame(&mut k, &ctx, t, vec![]));
        }
        assert!(steps[0] < -5.0, "keeps going the same way: {}", steps[0]);
        assert!(steps[0] < steps[10] && steps[10] < 0.0, "slowing down");
        assert_eq!(*steps.last().unwrap(), 0.0, "and stops");
        let total: f32 = steps.iter().sum();
        assert!(total < -100.0 && total > -1000.0, "a glide of a few hundred points: {total}");
    }

    /// One flick at `start`: fingers down, five quick moves, lift. Returns
    /// the time after it.
    fn flick(k: &mut Kinetic, ctx: &egui::Context, start: f64, dy: f32) -> f64 {
        frame(k, ctx, start, vec![wheel(0.0, TouchPhase::Start)]);
        for i in 1..=5 {
            frame(k, ctx, start + i as f64 / 60.0, vec![wheel(dy, TouchPhase::Move)]);
        }
        let end = start + 6.0 / 60.0;
        frame(k, ctx, end, vec![wheel(0.0, TouchPhase::End)]);
        end
    }

    /// How far the glide after a flick goes, frame by frame from `t`.
    fn glide(k: &mut Kinetic, ctx: &egui::Context, mut t: f64, frames: usize) -> (f32, f64) {
        let mut total = 0.0;
        for _ in 0..frames {
            t += 1.0 / 60.0;
            total += frame(k, ctx, t, vec![]);
        }
        (total, t)
    }

    #[test]
    fn flicks_in_a_row_add_up() {
        let ctx = egui::Context::default();
        let mut alone = Kinetic::default();
        let t = flick(&mut alone, &ctx, 0.0, -20.0);
        let (single, _) = glide(&mut alone, &ctx, t, 200);

        let mut k = Kinetic::default();
        let mut t = flick(&mut k, &ctx, 0.0, -20.0);
        t = glide(&mut k, &ctx, t, 6).1; // still gliding fast
        t = flick(&mut k, &ctx, t, -20.0);
        let (boosted, _) = glide(&mut k, &ctx, t, 200);
        assert!(boosted < single * 1.5, "the second flick goes further: {boosted} vs {single}");

        // The other way, it does not add: the second flick stands alone.
        let mut k = Kinetic::default();
        let mut t = flick(&mut k, &ctx, 0.0, -20.0);
        t = glide(&mut k, &ctx, t, 6).1;
        t = flick(&mut k, &ctx, t, 20.0);
        let (back, _) = glide(&mut k, &ctx, t, 200);
        assert!(back > 0.0 && back < -single * 1.2, "{back}");

        // Many in a row stay under the cap.
        let mut k = Kinetic::default();
        let mut t = 0.0;
        for _ in 0..10 {
            t = flick(&mut k, &ctx, t, -60.0);
            t = glide(&mut k, &ctx, t, 3).1;
        }
        let step = frame(&mut k, &ctx, t + 1.0 / 60.0, vec![]);
        assert!(-step <= MAX_SPEED / 60.0 + 1.0, "{step}");
    }

    #[test]
    fn a_slow_drag_or_a_click_does_not_glide() {
        let ctx = egui::Context::default();
        let mut k = Kinetic::default();
        frame(&mut k, &ctx, 0.0, vec![wheel(0.0, TouchPhase::Start)]);
        frame(&mut k, &ctx, 0.1, vec![wheel(-1.0, TouchPhase::Move)]);
        frame(&mut k, &ctx, 0.2, vec![wheel(-1.0, TouchPhase::Move)]);
        frame(&mut k, &ctx, 0.21, vec![wheel(0.0, TouchPhase::End)]);
        assert_eq!(frame(&mut k, &ctx, 0.23, vec![]), 0.0);

        for i in 1..=5 {
            frame(&mut k, &ctx, 1.0 + i as f64 / 60.0, vec![wheel(-20.0, TouchPhase::Move)]);
        }
        frame(&mut k, &ctx, 1.1, vec![wheel(0.0, TouchPhase::End)]);
        let click = Event::PointerButton { pos: egui::pos2(1.0, 1.0), button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() };
        frame(&mut k, &ctx, 1.12, vec![click]);
        assert_eq!(frame(&mut k, &ctx, 1.14, vec![]), 0.0, "the click stopped it");
    }
}
