use super::{Component, El, IntoView, Kind, View};

impl<V: IntoView> IntoView for (V,) {
    fn into_view(self) -> View {
        self.0.into_view()
    }

    #[inline]
    fn append_to<C>(self, parent: &mut El<C>) {
        self.0.append_to(parent);
    }

    fn with_component(self, component: Component) -> Self {
        (self.0.with_component(component),)
    }
}

macro_rules! tuple_view {
    ($($View:ident $index:tt),+) => {
        impl<$($View: IntoView),+> IntoView for ($($View,)+) {
            fn into_view(self) -> View {
                View { kind: Kind::Fragment(vec![$(self.$index.into_view(),)+]), component: None }
            }

            fn append_to<Content>(self, parent: &mut El<Content>) {
                $(self.$index.append_to(parent);)+
            }

            fn with_component(self, component: Component) -> Self {
                ($(self.$index.with_component(component),)+)
            }
        }
    };
}

tuple_view!(A 0, B 1);
tuple_view!(A 0, B 1, C 2);
tuple_view!(A 0, B 1, C 2, D 3);
tuple_view!(A 0, B 1, C 2, D 3, E 4);
tuple_view!(A 0, B 1, C 2, D 3, E 4, F 5);
tuple_view!(A 0, B 1, C 2, D 3, E 4, F 5, G 6);
tuple_view!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7);
tuple_view!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8);
tuple_view!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8, J 9);
tuple_view!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8, J 9, K 10);
tuple_view!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8, J 9, K 10, L 11);
