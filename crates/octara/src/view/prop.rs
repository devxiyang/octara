//! Fixed and tracked authoring values.

use octara_model::{
    geometry::Point,
    image::ImageSource,
    motion::TextReveal,
    native::TypedNativeProps,
    overlay::AnchorTarget,
    paint::PaintInput,
    semantics::{Checked, NumericRange},
    style::Transform3D,
    theme::{Color, ColorToken, ColorValue, Fill, ThemePatch},
};
use octara_runtime::{Binding, Field, ReadCx, ReadSignal, Signal};
use octara_tree::editor::EditorRichText;
use std::sync::Arc;

pub(crate) type ReadValue<T> = Box<dyn FnMut(&mut ReadCx<'_>) -> T>;

/// A fixed value or a value computed from tracked reads.
pub enum Prop<T> {
    Static(T),
    Dyn(ReadValue<T>),
}

impl<T: 'static> Prop<T> {
    pub fn map<U: 'static>(self, mut f: impl FnMut(T) -> U + 'static) -> Prop<U> {
        match self {
            Self::Static(value) => Prop::Static(f(value)),
            Self::Dyn(mut read) => bind(move |r| f(read(r))),
        }
    }
}

/// Builds a tracked property. The read context is available only during evaluation.
pub fn bind<T>(read: impl FnMut(&mut ReadCx<'_>) -> T + 'static) -> Prop<T> {
    Prop::Dyn(Box::new(read))
}

/// A value, signal, or tracked computation accepted by an authored property.
pub trait IntoProp<T> {
    fn into_prop(self) -> Prop<T>;
}

impl<T: 'static, U: Into<T> + 'static> IntoProp<T> for Prop<U> {
    fn into_prop(self) -> Prop<T> {
        self.map(Into::into)
    }
}

impl<T, U: Into<T>, F: FnMut(&mut ReadCx<'_>) -> U + 'static> IntoProp<T> for F {
    fn into_prop(mut self) -> Prop<T> {
        bind(move |r| self(r).into())
    }
}

impl<T, U: Clone + Into<T> + 'static> IntoProp<T> for Signal<U> {
    fn into_prop(self) -> Prop<T> {
        // Share the boxed reader with ReadSignal<U> for the same target type.
        self.read_only().into_prop()
    }
}

impl<T, U: Clone + Into<T> + 'static> IntoProp<T> for ReadSignal<U> {
    fn into_prop(self) -> Prop<T> {
        bind(move |r| r.get(self).into())
    }
}

impl<T, U: Clone + Into<T> + 'static> IntoProp<T> for Binding<U> {
    fn into_prop(self) -> Prop<T> {
        bind(move |read| read.get(self).into())
    }
}

impl<T, U: Clone + Into<T> + 'static> IntoProp<T> for Field<U> {
    fn into_prop(self) -> Prop<T> {
        bind(move |read| read.get(self).into())
    }
}

macro_rules! static_prop {
    ($($target:ty => [$($source:ty),* $(,)?]),* $(,)?) => {$ (
        $(
            impl IntoProp<$target> for $source {
                fn into_prop(self) -> Prop<$target> {
                    Prop::Static(self.into())
                }
            }
        )*
    )*};
}

pub(crate) use static_prop;

static_prop! {
    EditorRichText => [EditorRichText],
    ThemePatch => [ThemePatch],
    ImageSource => [ImageSource],
    bool => [bool],
    Option<bool> => [bool, Option<bool>],
    Option<Point> => [Point, Option<Point>],
    Option<TextReveal> => [TextReveal, Option<TextReveal>],
    Option<Transform3D> => [Transform3D, Option<Transform3D>],
    Option<AnchorTarget> => [AnchorTarget, Option<AnchorTarget>],
    u8 => [u8],
    Checked => [Checked, bool],
    f32 => [f32],
    u64 => [u64],
    String => [&str, &String, String],
    Arc<str> => [&str, String, Arc<str>],
    Option<String> => [String, Option<String>],
    ColorValue => [Color, ColorToken, ColorValue],
    Fill => [Color, ColorToken, ColorValue, Fill],
    NumericRange => [NumericRange],
    PaintInput => [PaintInput, [f32; 4]],
    serde_json::Value => [serde_json::Value],
    TypedNativeProps => [TypedNativeProps],
}

impl IntoProp<Option<String>> for &str {
    fn into_prop(self) -> Prop<Option<String>> {
        Prop::Static(Some(self.to_owned()))
    }
}
