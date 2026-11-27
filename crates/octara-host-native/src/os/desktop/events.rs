use super::gestures::TrackpadGestures;
use super::ime::WinitIme;
use super::{keys, wheel};
use crate::os::{FileDropPhase, PlatformEvent, PointerPhase};
use octara::runtime::WindowId;
use octara_model::geometry::Point;
use octara_model::input::{Button, InteractionEvent, Modifiers, PointerId, PointerKind};
use octara_model::theme::ColorScheme;
use winit::dpi::PhysicalPosition;
use winit::event::{MouseButton, Touch, TouchPhase, WindowEvent};
use winit::window::{Theme, Window};

pub(super) enum DesktopEvent {
    Platform(PlatformEvent),
    Accessibility(accesskit_winit::Event),
}

impl From<accesskit_winit::Event> for DesktopEvent {
    fn from(event: accesskit_winit::Event) -> Self {
        Self::Accessibility(event)
    }
}

#[derive(Default)]
pub(super) struct WindowInput {
    pub ime: WinitIme,
    gestures: TrackpadGestures,
    pointer: Option<Point>,
    modifiers: Modifiers,
}

impl WindowInput {
    pub(super) fn translate(&mut self, window: WindowId, native: &Window, event: WindowEvent) -> Option<PlatformEvent> {
        let next = match event {
            WindowEvent::CloseRequested => PlatformEvent::WindowCloseRequested(window),
            WindowEvent::Destroyed => PlatformEvent::WindowDestroyed(window),
            WindowEvent::ThemeChanged(theme) => theme_changed(theme),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => PlatformEvent::WindowResized(window),
            WindowEvent::Moved(_) => PlatformEvent::WindowMoved(window),
            WindowEvent::RedrawRequested => PlatformEvent::WindowRedrawRequested(window),
            WindowEvent::Focused(active) => PlatformEvent::WindowFocused { window, active },
            WindowEvent::ModifiersChanged(modifiers) => {
                let state = modifiers.state();
                self.modifiers.set(Modifiers::SHIFT, state.shift_key());
                self.modifiers.set(Modifiers::CTRL, state.control_key());
                self.modifiers.set(Modifiers::ALT, state.alt_key());
                self.modifiers.set(Modifiers::META, state.super_key());
                return None;
            }
            WindowEvent::CursorMoved { position, .. } => {
                let position = logical_position(position, native.scale_factor());
                self.pointer = Some(position);
                PlatformEvent::CursorMoved { window, position, modifiers: self.modifiers }
            }
            WindowEvent::CursorLeft { .. } => {
                self.pointer = None;
                PlatformEvent::CursorLeft(window)
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let position = self.pointer?;
                let button = match button {
                    MouseButton::Left => Button::Primary,
                    MouseButton::Right => Button::Secondary,
                    MouseButton::Middle => Button::Middle,
                    _ => return None,
                };
                let phase = if state.is_pressed() { PointerPhase::Started } else { PointerPhase::Ended };
                PlatformEvent::Pointer {
                    window,
                    phase,
                    position: Some(position),
                    id: PointerId(0),
                    kind: PointerKind::Mouse,
                    button,
                    modifiers: self.modifiers,
                }
            }
            WindowEvent::MouseWheel { delta, phase, .. } => PlatformEvent::Interaction {
                window,
                event: wheel::wheel_input(self.pointer?, self.modifiers, native.scale_factor(), delta, phase),
            },
            event @ (WindowEvent::PinchGesture { .. } | WindowEvent::RotationGesture { .. }) => PlatformEvent::Interaction {
                window,
                event: InteractionEvent::Gesture(self.gestures.update(&event, self.pointer, self.modifiers)?),
            },
            WindowEvent::Touch(Touch { phase, location, id, .. }) => PlatformEvent::Pointer {
                window,
                phase: match phase {
                    TouchPhase::Started => PointerPhase::Started,
                    TouchPhase::Moved => PointerPhase::Moved,
                    TouchPhase::Ended => PointerPhase::Ended,
                    TouchPhase::Cancelled => PointerPhase::Cancelled,
                },
                position: Some(logical_position(location, native.scale_factor())),
                id: PointerId(id.wrapping_add(1)),
                kind: PointerKind::Touch,
                button: Button::Primary,
                modifiers: self.modifiers,
            },
            WindowEvent::HoveredFile(path) => {
                PlatformEvent::FileDrop { window, phase: FileDropPhase::Entered, position: self.pointer, paths: vec![path] }
            }
            WindowEvent::HoveredFileCancelled => {
                PlatformEvent::FileDrop { window, phase: FileDropPhase::Left, position: self.pointer, paths: Vec::new() }
            }
            WindowEvent::DroppedFile(path) => {
                // Winit supplies one path per event. The shell combines paths at the end of the batch.
                PlatformEvent::FileDrop { window, phase: FileDropPhase::Dropped, position: self.pointer, paths: vec![path] }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state.is_pressed() => {
                PlatformEvent::Key { window, event: keys::key(&event, self.modifiers) }
            }
            WindowEvent::Ime(ime) => PlatformEvent::Editor { window, event: self.ime.event(&ime)? },
            _ => return None,
        };
        Some(next)
    }
}

fn logical_position(position: PhysicalPosition<f64>, scale: f64) -> Point {
    let position = position.to_logical::<f32>(scale);
    Point::new(position.x, position.y)
}

pub(super) fn color_scheme(theme: Theme) -> ColorScheme {
    match theme {
        Theme::Light => ColorScheme::Light,
        Theme::Dark => ColorScheme::Dark,
    }
}

pub(super) fn theme_changed(theme: Theme) -> PlatformEvent {
    PlatformEvent::SystemPreferencesChanged {
        color_scheme: Some(color_scheme(theme)),
        reduced_motion: None,
        text_scale: None,
        high_contrast: None,
    }
}
