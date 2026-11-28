/// The top of the occupied area of one texture slice. The first step starts
/// at x = 0 and each step extends to the next or to the edge of the slice.
pub(super) struct Skyline {
    edge: u32,
    steps: Vec<[u32; 2]>,
}

impl Skyline {
    pub(super) fn new(edge: u32) -> Self {
        Self { edge, steps: vec![[0, 0]] }
    }

    pub(super) fn place(&mut self, [width, height]: [u32; 2], alignment: u32) -> Option<[u32; 2]> {
        if width == 0 || height == 0 || width > self.edge || height > self.edge {
            return None;
        }
        let [x, y] = self
            .steps
            .iter()
            .enumerate()
            .filter_map(|(index, &[start, _])| {
                let x = start.next_multiple_of(alignment);
                if x >= self.steps.get(index + 1).map_or(self.edge, |step| step[0]) || x > self.edge - width {
                    return None;
                }
                let y = self.steps[index..]
                    .iter()
                    .take_while(|step| step[0] < x + width)
                    .map(|step| step[1])
                    .max()?
                    .next_multiple_of(alignment);
                (y <= self.edge - height).then_some([x, y])
            })
            .min_by_key(|&[x, y]| (y, x))?;
        let first = self.split(x);
        let last = self.split(x + width);
        self.steps.splice(first..last, [[x, y + height]]);
        self.steps.dedup_by(|next, previous| next[1] == previous[1]);
        Some([x, y])
    }

    fn split(&mut self, x: u32) -> usize {
        match self.steps.binary_search_by_key(&x, |step| step[0]) {
            Ok(index) => index,
            Err(index) => {
                if x < self.edge {
                    self.steps.insert(index, [x, self.steps[index - 1][1]]);
                }
                index
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Skyline;

    #[test]
    fn fills_the_lowest_span_across_steps() {
        let mut packer = Skyline::new(8);
        assert_eq!(packer.place([2, 3], 1), Some([0, 0]));
        assert_eq!(packer.place([3, 1], 1), Some([2, 0]));
        assert_eq!(packer.place([4, 2], 1), Some([2, 1]));
        assert_eq!(packer.place([6, 5], 1), Some([0, 3]));
        assert_eq!(packer.place([2, 8], 1), Some([6, 0]));
        assert_eq!(packer.place([1, 1], 1), None);
    }

    #[test]
    fn alignment_and_failed_placement_preserve_neighbors() {
        let mut packer = Skyline::new(8);
        assert_eq!(packer.place([3, 3], 1), Some([0, 0]));
        assert_eq!(packer.place([5, 5], 4), None);
        assert_eq!(packer.place([4, 4], 4), Some([4, 0]));
        assert_eq!(packer.place([1, 4], 1), Some([3, 0]));
        assert_eq!(packer.place([4, 4], 4), Some([0, 4]));
        assert_eq!(packer.place([4, 4], 4), Some([4, 4]));
        assert_eq!(packer.place([9, 1], 1), None);
    }
}
