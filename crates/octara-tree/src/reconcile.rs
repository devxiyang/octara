//! Shared ordering for retained tree and host children.

/// Selects one longest increasing subsequence of retained ranks in O(n log n).
/// Newly attached children are represented by `None` and always need placement.
pub fn stationary_indices(ranks: impl IntoIterator<Item = Option<usize>>) -> Vec<bool> {
    let ranks = ranks.into_iter();
    let mut predecessors = Vec::with_capacity(ranks.size_hint().0);
    let mut tails: Vec<(usize, usize)> = Vec::new();
    for (index, rank) in ranks.enumerate() {
        predecessors.push(None);
        let Some(rank) = rank else { continue };
        let length =
            if tails.last().is_none_or(|&(tail, _)| tail < rank) { tails.len() } else { tails.partition_point(|&(tail, _)| tail < rank) };
        predecessors[index] = length.checked_sub(1).map(|previous| tails[previous].1);
        if length == tails.len() {
            tails.push((rank, index));
        } else {
            tails[length] = (rank, index);
        }
    }
    let mut stationary = vec![false; predecessors.len()];
    let mut current = tails.last().map(|&(_, index)| index);
    while let Some(index) = current {
        stationary[index] = true;
        current = predecessors[index];
    }
    stationary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_moves_one_child_and_inserts_new_children() {
        let ranks = [Some(1), Some(2), None, Some(3), Some(0)];
        assert_eq!(stationary_indices(ranks), [true, true, false, true, false]);
    }

    #[test]
    fn retains_the_longest_subsequence_for_every_small_permutation() {
        fn verify(ranks: &mut [usize], start: usize) {
            if start < ranks.len() {
                for swap in start..ranks.len() {
                    ranks.swap(start, swap);
                    verify(ranks, start + 1);
                    ranks.swap(start, swap);
                }
                return;
            }

            let mut lengths = vec![1; ranks.len()];
            for (index, rank) in ranks.iter().enumerate() {
                for before in 0..index {
                    if ranks[before] < *rank {
                        lengths[index] = lengths[index].max(lengths[before] + 1);
                    }
                }
            }
            let retained: Vec<_> = ranks
                .iter()
                .zip(stationary_indices(ranks.iter().copied().map(Some)))
                .filter_map(|(&rank, stationary)| stationary.then_some(rank))
                .collect();
            assert!(retained.is_sorted());
            assert_eq!(retained.len(), lengths.into_iter().max().unwrap_or(0));
        }

        verify(&mut [0, 1, 2, 3, 4, 5, 6], 0);
    }
}
