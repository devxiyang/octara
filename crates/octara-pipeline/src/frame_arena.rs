//! Reusable, typed bump storage for frame-local values.

pub(crate) struct FrameArena<T> {
    buffers: [Vec<T>; 2],
    current: usize,
    offset: usize,
}

impl<T> Default for FrameArena<T> {
    fn default() -> Self {
        Self { buffers: Default::default(), current: 0, offset: 0 }
    }
}

impl<T: Copy> FrameArena<T> {
    pub(crate) fn push(&mut self, value: T) {
        let buffer = &mut self.buffers[self.current];
        if self.offset == buffer.len() {
            buffer.push(value);
        } else {
            buffer[self.offset] = value;
        }
        self.offset += 1;
    }

    pub(crate) fn used(&mut self) -> &mut [T] {
        &mut self.buffers[self.current][..self.offset]
    }

    pub(crate) fn finish(&mut self) {
        self.offset = 0;
        self.current ^= 1;
    }
}
