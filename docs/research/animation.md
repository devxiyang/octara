# Research: Animation and Transitions

2026-11. Compares Rune (source: `rune-motion`, `el/motion.rs`, `el/visuals.rs`, `research/06`), GPUI (Zed `main` 985bbf4:
`elements/animation.rs`, `spring.rs`, `crates/ui`), Expo's Exact (the `motion/` crate and docs) and SwiftUI, Compose, Flutter,
Motion (compiled from memory of the official docs).

## Model

| | Rune | GPUI | Exact |
| --- | --- | --- | --- |
| Curves | Spring-first: `Spring::new(duration, bounce)`, the same mapping as SwiftUI's `Spring(duration:bounce:)` (`rune-motion/src/spring.rs:6-20`); also Tween, Decay, Keyframes, Instant; named tiers SNAP / SETTLE / REFLOW / POP ..., each with a shorter exit tier | `Animation { duration, easing }` with state stored by id; plus a separate `spring.rs` (stiffness / damping / mass, analytic stepping) | CSS `transition` / `animation`, plus `-exact-spring(stiffness, damping, mass)`; default curve is CSS `ease` |
| Computation | Analytic: each track is "target + several closed-form segments", evaluated directly at frame time without integration; its own `exp` / `sin_cos` keep goldens consistent | Analytic spring stepping; duration animations computed from `now - start` | Closed form, with time supplied by the host, giving the same result at any step size |
| Interruption | Samples the current (x, v) to start a new segment, velocity continuous (C¹); unchanged targets are left alone; the first assignment does not animate | Springs keep velocity and can be retargeted; duration animations cannot be retargeted (only restarted with a new id) | Retargets from the current curve, springs inherit velocity; color (Oklab) velocity resets to zero |
| Stopping | Each segment precomputes an upper bound on its rest time, after which it lands at (target, 0); thresholds per property class | `is_settled(position and velocity)`, snaps when settled | `|x|<1e-3 and |v|<1e-3` is judged at the sample time, so the result depends on frame rate (`issues/20261110-spring-done-depends-on-frame-cadence.md`) |

## Declaration

| | Rune | GPUI | Exact |
| --- | --- | --- | --- |
| Implicit | Yes: the runtime compares targets and animates on change, provided an ancestor gave `El::motion(spring)` (inherited by the subtree); each property can be overridden separately (`opacity_motion` ...) | No: only `with_animation(id, ..)` re-runs a closure every frame; hover and press do not animate | Yes: a `transition` line on the node |
| Explicit | Transaction `Tx`: `instant` (direct manipulation, such as typing and dragging, is automatically instant), `settle` (everything settles), `motion_curve`; subtree `instant_changes()` | — | None |
| Animatable properties | Color (premultiplied Oklab), opacity, shadow (offset / blur / spread follow the layout spring, color follows the color spring), gradient stops, `offset` / `scale` / `rotate`, hover (separate enter / exit curves), press scale | Whatever the closure writes (the view is rebuilt every frame) | Translate, scale, rotate, opacity, color, shadow, numeric height, font weight, etc.; width and `top` are rejected in favor of layout transitions |
| Property class | `PropClass` (spatial, scale, rotation, opacity, color, shape, appearance ...) decides the rest threshold and the handling under reduced motion (`props.rs:19-67`) | — | — |

## Layout, appearance and disappearance

- **Rune**: layout runs only once, on the target state; changed nodes are animated with springs on each of the four edges (not scale-style FLIP: "avoid scaling things that have text");
  background, border and shadow are drawn by the in-flight rectangle. Width leads (`width_leads`), and text is laid out once at the landing width (`hold_landing_width`).
  Appear / disappear: by default every element fades in and out (0.2 s / 0.15 s); a removed keyed node enters "exiting", leaves layout and stops responding, and is unmounted only after opacity reaches zero;
  returning with the same key revives it from its current state (generation guard); first mount does not animate; rows remounted by virtual list scrolling do not replay.
- **GPUI**: no layout animation and no exit animation; the component library has only a few enter animations and spinners.
- **Exact**: `-exact-exit-animation` (the node leaves layout immediately and becomes inert, siblings move by layout transition); `-exact-layout-transition` animates only the box itself,
  with children jumping to the end; a known bug: ghost nodes that are exiting still trigger resize actions.
- **Native**: SwiftUI `.transition` + `matchedGeometryEffect`; Compose `AnimatedVisibility`, `animateContentSize`, `animateItem`;
  Motion `AnimatePresence` + `layout` (FLIP).

## Threads and frames

- **Rune**: PLAN says the render thread owns animation; in practice it is evaluated per frame time on the main thread, and view functions are never re-run for animation (README:2905-2911);
  with animations the display link rises to 120 Hz, and when idle there are zero frames and zero commits.
- **GPUI**: animating elements request the next frame every frame, and stop requesting once done; zero cost when idle.
- **Exact**: the host supplies frames and the motion crate evaluates; on Web it lowers to CSS / WAAPI.

## Reduced motion and determinism

- **Rune**: enforced by the runtime, mapped by property class: spatial, scale and rotation become instant, opacity and color become a 0.18 s fade; the user's own dragging and inertia are not reduced.
  Manual clock, `seek`; the "motion gate" records each element's rectangle and opacity frame by frame on the virtual clock and reports jumps, reversals, jitter and flicker.
- **GPUI**: a global `reduce_motion`, but each component checks it itself (advisory); tests use `advance_clock`, `simulate_next_frame`.
- **Exact**: turned off manually by the app; `clock settle` runs animations to completion; `screenshot over 600 every 50` captures a contact sheet.

## Conclusions

1. **Spring-first, `Spring::new(duration, bounce)`** (the same mapping in Rune and SwiftUI), with named tiers; duration + easing only for fades and fixed-duration indicators.
2. **Analytic tracks**: value = target + closed-form segments, evaluated directly on the app clock; on retargeting, sample (x, v) and start a new segment; compute the rest time when building a segment (avoiding Exact's "settling depends on frame rate").
3. **Animation lives in the kernel property layer**: style resolution gives the target and the track animates toward it, without re-running views. Octara's fine-grained signals already achieve "one write, one property",
   which puts tracks right below signals (Rune built rebuild-style views precisely because it has no signals).
4. **Implicit + transaction override**: once a node (and its subtree) declares how to animate, changes in style and state (hover, press, focus) all animate; a transaction can make it instant or settle everything;
   direct manipulation (typing, dragging) is automatically instant.
5. **Property classes** decide the rest threshold and the handling under reduced motion, so authors cannot miss it.
6. **Layout animation per edge** (Rune), not scale-style FLIP; hit testing and accessibility use the in-flight geometry.
7. **Appearance and disappearance** are framework capabilities: a keyed removal enters an exit state (leaves layout, inert) and unmounts only when the animation ends; same-key revival; first mount does not animate.
8. **Evaluate at frame time on the main thread**, with track storage independent of the renderer to leave room for a render thread; zero frames when idle; `next_deadline` reports the next frame while animating,
   `run_until_idle` runs until everything settles; tests can take a screenshot at any moment.
