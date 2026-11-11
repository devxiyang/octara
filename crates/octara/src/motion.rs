//! Motion: springs and the tracks that move a property toward its target.
//!
//! A track's value is its target plus one closed-form segment, evaluated at the frame's time:
//! nothing is integrated, so any step size and any frame rate give the same value (Rune). A new
//! target samples the current position and velocity and starts a new segment from there, so
//! motion stays continuous when interrupted. Each segment knows when it comes to rest from the
//! start, so when a track stops does not depend on when frames are sampled (Exact's
//! frame-cadence bug).

use std::f64::consts::PI;

/// A spring, by its perceptual duration and bounce (as SwiftUI's `Spring(duration:bounce:)` and
/// Rune): ω = 2π / duration; ζ = 1 − bounce for bounce ≥ 0, 1 / (1 + bounce) below. Bounce 0 is
/// critically damped (no overshoot); up to 1 springs past the target.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Spring {
    /// Seconds.
    pub duration: f32,
    /// −1 (sluggish) … 0 (no overshoot) … 1 (keeps bouncing).
    pub bounce: f32,
}

impl Spring {
    pub const fn new(duration: f32, bounce: f32) -> Spring {
        assert!(duration > 0.0, "a spring's duration is positive");
        assert!(bounce > -1.0 && bounce < 1.0, "a spring's bounce is in (-1, 1)");
        Spring { duration, bounce }
    }
    /// Quick and direct: hover, press, toggles (0.25 s).
    pub const SNAP: Spring = Spring::new(0.25, 0.0);
    /// Things coming to rest: values, selection (0.3 s).
    pub const SETTLE: Spring = Spring::new(0.3, 0.0);
    /// Layout moving: rows, panels, sizes (0.4 s).
    pub const REFLOW: Spring = Spring::new(0.4, 0.0);
    /// Moving across the screen: sheets, slides (0.32 s).
    pub const GLIDE: Spring = Spring::new(0.32, 0.0);
    /// A little overshoot: badges, popovers appearing (0.25 s, bounce 0.15).
    pub const POP: Spring = Spring::new(0.25, 0.15);
    /// Shapes changing (0.3 s, bounce 0.16).
    pub const MORPH: Spring = Spring::new(0.3, 0.16);
    /// The named springs above, by name (for tools and docs).
    pub const NAMED: [(&'static str, Spring); 6] = [
        ("SNAP", Spring::SNAP),
        ("SETTLE", Spring::SETTLE),
        ("REFLOW", Spring::REFLOW),
        ("GLIDE", Spring::GLIDE),
        ("POP", Spring::POP),
        ("MORPH", Spring::MORPH),
    ];

    fn omega(&self) -> f64 {
        2.0 * PI / self.duration as f64
    }
    fn zeta(&self) -> f64 {
        let b = self.bounce as f64;
        if b >= 0.0 { 1.0 - b } else { 1.0 / (1.0 + b) }
    }
}

/// How a property moves to a new target.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Curve {
    Spring(Spring),
    /// An ease-in-out over `duration` seconds that ignores the current velocity (Reduce Motion's
    /// fades).
    Fade(f32),
    /// No motion: the value jumps.
    Instant,
}

impl From<Spring> for Curve {
    fn from(s: Spring) -> Curve {
        Curve::Spring(s)
    }
}

/// The fade Reduce Motion turns opacity and color changes into (Rune: 0.18 s).
pub const REDUCED_FADE: Curve = Curve::Fade(0.18);

/// What kind of quantity a property is: it decides when motion counts as at rest, and what
/// Reduce Motion does with it (Rune's `PropClass`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PropClass {
    /// Positions and sizes (logical pixels).
    Spatial,
    /// Opacity (0–1).
    Opacity,
    /// Color channels (0–1).
    Color,
}

impl PropClass {
    /// Below this distance (and this distance per second scaled by ω) a value is at rest.
    pub fn epsilon(self) -> f64 {
        match self {
            PropClass::Spatial => 1.0 / 128.0,
            PropClass::Opacity | PropClass::Color => 1.0 / 1024.0,
        }
    }
    /// The curve Reduce Motion uses instead of `curve`: no spatial motion; fades for the rest.
    pub fn reduced(self, curve: Curve) -> Curve {
        match (self, curve) {
            (_, Curve::Instant) => Curve::Instant,
            (PropClass::Spatial, _) => Curve::Instant,
            (PropClass::Opacity | PropClass::Color, _) => REDUCED_FADE,
        }
    }
}

/// One closed-form piece of motion: at `t0` the value was `x0` away from the target, moving at
/// `v0`; it comes to rest by `end`.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Segment {
    t0: f64,
    x0: f64,
    v0: f64,
    curve: Curve,
    end: f64,
}

impl Segment {
    /// The offset from the target and the velocity `tau` seconds into the segment.
    fn at(&self, tau: f64) -> (f64, f64) {
        let (x0, v0) = (self.x0, self.v0);
        match self.curve {
            Curve::Instant => (0.0, 0.0),
            Curve::Fade(d) => {
                let d = d as f64;
                if tau >= d {
                    return (0.0, 0.0);
                }
                let u = tau / d;
                let (e, de) = if u < 0.5 {
                    (4.0 * u * u * u, 12.0 * u * u)
                } else {
                    (1.0 - (2.0 - 2.0 * u).powi(3) / 2.0, 3.0 * (2.0 - 2.0 * u).powi(2))
                };
                (x0 * (1.0 - e), -x0 * de / d)
            }
            Curve::Spring(s) => {
                let (w, z) = (s.omega(), s.zeta());
                if (z - 1.0).abs() < 1e-9 {
                    let c = v0 + w * x0;
                    let e = (-w * tau).exp();
                    ((x0 + c * tau) * e, (c - w * (x0 + c * tau)) * e)
                } else if z < 1.0 {
                    let wd = w * (1.0 - z * z).sqrt();
                    let b = (v0 + z * w * x0) / wd;
                    let e = (-z * w * tau).exp();
                    let (sin, cos) = (wd * tau).sin_cos();
                    let x = e * (x0 * cos + b * sin);
                    let v = e * (-z * w * (x0 * cos + b * sin) + wd * (-x0 * sin + b * cos));
                    (x, v)
                } else {
                    let r = (z * z - 1.0).sqrt();
                    let (r1, r2) = (-w * (z - r), -w * (z + r));
                    let c2 = (v0 - r1 * x0) / (r2 - r1);
                    let c1 = x0 - c2;
                    let (e1, e2) = ((r1 * tau).exp(), (r2 * tau).exp());
                    (c1 * e1 + c2 * e2, r1 * c1 * e1 + r2 * c2 * e2)
                }
            }
        }
    }

    /// An upper bound of how far from rest the motion can be at `tau`: the larger of the offset
    /// and the velocity over ω.
    fn envelope(&self, tau: f64) -> f64 {
        let (x0, v0) = (self.x0, self.v0);
        let Curve::Spring(s) = self.curve else { return 0.0 };
        let (w, z) = (s.omega(), s.zeta());
        if (z - 1.0).abs() < 1e-9 {
            let c = (v0 + w * x0).abs();
            (x0.abs() + c * (tau + 1.0 / w)) * (-w * tau).exp()
        } else if z < 1.0 {
            let wd = w * (1.0 - z * z).sqrt();
            let a = (x0 * x0 + ((v0 + z * w * x0) / wd).powi(2)).sqrt();
            a * (1.0 + (z * w + wd) / w) * (-z * w * tau).exp()
        } else {
            let r = (z * z - 1.0).sqrt();
            let (r1, r2) = (-w * (z - r), -w * (z + r));
            let c2 = (v0 - r1 * x0) / (r2 - r1);
            let c1 = x0 - c2;
            c1.abs() * (1.0 + r1.abs() / w) * (r1 * tau).exp() + c2.abs() * (1.0 + r2.abs() / w) * (r2 * tau).exp()
        }
    }

    /// When the motion is within `eps` of rest for good (seconds after `t0`).
    fn rest(&self, eps: f64) -> f64 {
        match self.curve {
            Curve::Instant => 0.0,
            Curve::Fade(d) => d as f64,
            Curve::Spring(_) => {
                if self.envelope(0.0) < eps {
                    return 0.0;
                }
                // The envelope may rise before it falls: double until it is below `eps`, then
                // bisect between the last two probes (deterministic, a few dozen evaluations).
                let mut hi = 1.0 / 64.0;
                while self.envelope(hi) >= eps && hi < 3600.0 {
                    hi *= 2.0;
                }
                let mut lo = hi / 2.0;
                for _ in 0..40 {
                    let mid = (lo + hi) / 2.0;
                    if self.envelope(mid) >= eps { lo = mid } else { hi = mid }
                }
                hi
            }
        }
    }
}

/// One animated number: a target and the motion toward it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Track {
    target: f64,
    segment: Option<Segment>,
}

impl Track {
    /// A track at rest at `value`.
    pub fn new(value: f64) -> Track {
        Track { target: value, segment: None }
    }

    pub fn target(&self) -> f64 {
        self.target
    }

    /// The value and velocity at time `t` (seconds).
    pub fn sample(&self, t: f64) -> (f64, f64) {
        match &self.segment {
            Some(s) if t < s.end => {
                let (x, v) = s.at((t - s.t0).max(0.0));
                (self.target + x, v)
            }
            _ => (self.target, 0.0),
        }
    }

    pub fn value(&self, t: f64) -> f64 {
        self.sample(t).0
    }

    /// Moves toward `target` from time `t` with `curve`, starting from where (and how fast) the
    /// track is at `t`. An unchanged target changes nothing.
    pub fn retarget(&mut self, target: f64, t: f64, curve: Curve, class: PropClass) {
        if target == self.target {
            return;
        }
        let (x, v) = self.sample(t);
        self.target = target;
        let mut s = Segment { t0: t, x0: x - target, v0: v, curve, end: t };
        if matches!(curve, Curve::Instant) {
            self.segment = None;
            return;
        }
        s.end = t + s.rest(class.epsilon());
        self.segment = (s.end > t).then_some(s);
    }

    /// Jumps to `value`: no motion.
    pub fn set(&mut self, value: f64) {
        *self = Track::new(value);
    }

    /// When the track comes to rest, if it is moving at `t`.
    pub fn rest_at(&self, t: f64) -> Option<f64> {
        self.segment.filter(|s| s.end > t).map(|s| s.end)
    }

    pub fn moving(&self, t: f64) -> bool {
        self.rest_at(t).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moved(curve: Curve) -> Track {
        let mut t = Track::new(0.0);
        t.retarget(100.0, 0.0, curve, PropClass::Spatial);
        t
    }

    #[test]
    fn springs_start_where_they_were_and_end_at_the_target() {
        for spring in [Spring::SNAP, Spring::REFLOW, Spring::POP, Spring::new(0.4, -0.5)] {
            let t = moved(Curve::Spring(spring));
            assert_eq!(t.sample(0.0), (0.0, 0.0));
            let end = t.rest_at(0.0).unwrap();
            assert!(end > spring.duration as f64 * 0.3 && end < spring.duration as f64 * 6.0, "{spring:?}: rest at {end}");
            assert_eq!(t.sample(end), (100.0, 0.0));
            // Within the threshold from a little before the end on.
            let (x, _) = t.sample(end * 0.999);
            assert!((x - 100.0).abs() < 1.0, "{spring:?}: {x}");
        }
    }

    #[test]
    fn bounce_overshoots_and_zero_bounce_does_not() {
        let max = |t: &Track| (0..2000).map(|i| t.value(i as f64 / 1000.0)).fold(f64::MIN, f64::max);
        assert!(max(&moved(Curve::Spring(Spring::new(0.3, 0.3)))) > 101.0);
        assert!(max(&moved(Curve::Spring(Spring::SETTLE))) <= 100.0 + 1e-9);
    }

    #[test]
    fn retargeting_keeps_position_and_velocity() {
        let mut t = moved(Curve::Spring(Spring::SNAP));
        let (x, v) = t.sample(0.05);
        t.retarget(0.0, 0.05, Curve::Spring(Spring::SNAP), PropClass::Spatial);
        let (x2, v2) = t.sample(0.05);
        assert!((x - x2).abs() < 1e-9 && (v - v2).abs() < 1e-9);
        assert_eq!(t.target(), 0.0);
        assert_eq!(t.value(10.0), 0.0);
    }

    #[test]
    fn sampling_does_not_depend_on_the_frame_rate() {
        let t = moved(Curve::Spring(Spring::POP));
        // The value at 0.1 s is the value at 0.1 s, however it is reached: the track is a function.
        assert_eq!(t.value(0.1), t.value(0.1));
        let end = t.rest_at(0.0).unwrap();
        for hz in [24.0, 60.0, 120.0, 240.0] {
            let frames = (end * hz).ceil() as usize + 1;
            let last = t.value(frames as f64 / hz);
            assert_eq!(last, 100.0, "at rest after the end at {hz} Hz");
        }
    }

    #[test]
    fn velocity_is_the_derivative() {
        for spring in [Spring::SNAP, Spring::POP, Spring::new(0.5, -0.4)] {
            let t = moved(Curve::Spring(spring));
            for ms in [10.0, 40.0, 90.0] {
                let s = ms / 1000.0;
                let h = 1e-6;
                let numeric = (t.value(s + h) - t.value(s - h)) / (2.0 * h);
                let (_, v) = t.sample(s);
                assert!((numeric - v).abs() < 1e-3 * v.abs().max(1.0), "{spring:?} at {ms} ms: {numeric} vs {v}");
            }
        }
    }

    #[test]
    fn fades_instants_and_unchanged_targets() {
        let t = moved(Curve::Fade(0.2));
        assert!((t.value(0.1) - 50.0).abs() < 1e-3, "halfway at half the time");
        assert!((t.rest_at(0.0).unwrap() - 0.2).abs() < 1e-6);
        let t = moved(Curve::Instant);
        assert_eq!((t.value(0.0), t.moving(0.0)), (100.0, false));
        let mut t = moved(Curve::Spring(Spring::SNAP));
        let before = t;
        t.retarget(100.0, 0.1, Curve::Spring(Spring::GLIDE), PropClass::Spatial);
        assert_eq!(t, before, "the same target changes nothing");
    }

    #[test]
    fn reduce_motion_maps_by_class() {
        let c = Curve::Spring(Spring::SNAP);
        assert_eq!(PropClass::Spatial.reduced(c), Curve::Instant);
        assert_eq!(PropClass::Opacity.reduced(c), REDUCED_FADE);
        assert_eq!(PropClass::Color.reduced(Curve::Instant), Curve::Instant);
    }
}
