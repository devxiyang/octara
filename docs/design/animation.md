# Animation

Based on [animation research](../research/animation.md). Confirmed: elements declare `motion`, subtrees inherit; inside a motion subtree, elements
fade in and out by default. Animation is never state: the view gives target values, and the kernel's tracks move between targets (Rune); views and
bindings do not rerun because of animation.

## Curves

- `Spring::new(duration, bounce)`: the same mapping as SwiftUI `Spring(duration:bounce:)` and Rune: ω = 2π / duration,
  ζ = 1 − bounce when bounce ≥ 0, ζ = 1 / (1 + bounce) when bounce < 0.
- Named presets (Rune's values): `SNAP` (0.25 s), `SETTLE` (0.3 s), `REFLOW` (0.4 s), `GLIDE` (0.32 s), `POP` (0.25 s, bounce 0.15),
  `MORPH` (0.3 s, bounce 0.16).
- Fade (`Fade { duration }`, ease-in-out) is used only for opacity and color under reduced motion.

## Tracks

- One track per property: `value = target + segment(t)`, where the segment is the closed-form spring solution (underdamped, critical, overdamped),
  evaluated directly at frame times, not integrated; the result is the same for any step size and any frame rate.
- New target: sample (x, v) at the current time and start a new segment from it as the initial value (position and velocity are continuous). An
  unchanged target does not move; the first assignment does not animate.
- The rest time (the upper bound of the envelope) is computed when the segment is built; once past it, the value lands on the target and leaves the
  active set. So "when to stop" is independent of frame rate (avoiding Exact's bug).
- Colors animate per component in premultiplied OKLab (Rune); positional values use logical pixels.
- Property classes (`PropClass`): spatial, opacity, color. The class decides the rest threshold (spatial 1/128 px, opacity and color 1/1024) and the
  handling under reduced motion.

## Declaration

- `.motion(spring)`: on this element and its subtree, every property whose style (including theme and state overrides) changes is animated;
  `motion_none()` turns it off (inherited by the subtree).
- Per-class overrides: `.color_motion(spring)` (color), `.opacity_motion(spring)`, `.layout_motion(spring)` (position and size).
- The component library sets defaults for buttons, switches, checkboxes and dropdowns (color changes on hover and press).

## Transactions

- `instant(|| ..)`: changes caused by writes inside are not animated (Rune `tx.instant`).
- `settle_motion()`: all running animations settle immediately, and exiting nodes unmount immediately (Rune `tx.settle`).
- Direct manipulation is automatically instant: typing in a text field, dragging.

## Reduced motion

- Enforced by the runtime per class (Rune): spatial changes become instant; opacity and color become a 0.18 s fade. `App::set_reduced_motion`; the
  shell reads the system setting.

## Time and frames

- The kernel holds the presentation time (the app clock), and the display list is sampled at that time; with active tracks, `next_deadline` returns
  the next frame (about 16.7 ms), otherwise there is no wake-up.
- `run_until_idle` runs animations to completion; tests can `advance` to any time and take a screenshot.
- Evaluated on the main thread; track storage does not depend on the renderer (it can move to the render thread later).

## Implemented

Steps 1–5 are all implemented (see TODO).

## Steps

1. Curves and tracks (this module);
2. Property animation: color, opacity, shadow, state overrides, transactions, reduced motion, component library defaults;
3. Layout: animate position and size per edge; painting, hit testing and accessibility use the presented geometry;
4. Appearing and disappearing: fade in and out by default in motion subtrees; exiting nodes leave layout, are inert, cannot be found by test id, and
   unmount after fading out;
5. Scrolling: `scroll_to` slides by spring, and a repeated request along the way keeps the velocity.
