//! Load-time proof that a map's node arrays are trees. The loaders run these
//! once so the hot walkers (`LeafWalk`, the clip map kernels, the vis and
//! partition walks) can follow child links with no visited set and no step
//! bound: a file with a cycle, a shared child or a deep chain is refused.

use anyhow::{Result, ensure};

/// The deepest node a tree may hold. Stock trees stay far shallower; a
/// deeper one is hostile, and the recursive kernels would overflow on it.
pub const MAX_DEPTH: usize = 256;

/// A BSP tree from node 0 whose `children(n)` are node indices, or
/// `-1 - leaf` for a leaf below `leafs`. Every node the walk reaches is
/// entered once and lies under [`MAX_DEPTH`]; unreached nodes are not read.
pub fn check_bsp(nodes: usize, leafs: usize, children: impl Fn(usize) -> [i32; 2]) -> Result<()> {
    if nodes == 0 {
        return Ok(());
    }
    let mut seen = vec![false; nodes];
    seen[0] = true;
    let mut stack = vec![(0usize, 0usize)];
    while let Some((n, depth)) = stack.pop() {
        ensure!(depth < MAX_DEPTH, "node {n} lies {depth} deep");
        for c in children(n) {
            if c < 0 {
                let leaf = -(i64::from(c) + 1);
                ensure!(
                    (leaf as u64) < leafs as u64,
                    "node {n}: leaf {leaf} of {leafs}"
                );
                continue;
            }
            let c = c as usize;
            ensure!(c < nodes, "node {n}: child {c} of {nodes}");
            ensure!(!seen[c], "node {c} is reached twice");
            seen[c] = true;
            stack.push((c, depth + 1));
        }
    }
    Ok(())
}

/// A forest of `nodes` whose `children(n)` lie past `n`: each child is in
/// the array, has one parent, and lies under [`MAX_DEPTH`]. One pass in
/// index order, since a parent always comes before its children.
pub fn check_forest<I>(nodes: usize, children: impl Fn(usize) -> I) -> Result<()>
where
    I: IntoIterator<Item = i64>,
{
    let mut depth = vec![0usize; nodes];
    let mut has_parent = vec![false; nodes];
    for n in 0..nodes {
        for c in children(n) {
            ensure!(
                c > n as i64 && c < nodes as i64,
                "node {n}: child {c} of {nodes} is not past it"
            );
            let c = c as usize;
            ensure!(!has_parent[c], "node {c} has two parents");
            has_parent[c] = true;
            depth[c] = depth[n] + 1;
            ensure!(depth[c] < MAX_DEPTH, "node {c} lies {} deep", depth[c]);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bsp(children: &[[i32; 2]], leafs: usize) -> Result<()> {
        check_bsp(children.len(), leafs, |n| children[n])
    }

    #[test]
    fn bsp_tree_passes() {
        assert!(bsp(&[[1, 2], [-1, -2], [-3, -1]], 3).is_ok());
    }

    #[test]
    fn bsp_cycle_shared_and_range_fail() {
        assert!(bsp(&[[0, -1]], 1).is_err());
        assert!(bsp(&[[1, 2], [2, -1], [-1, -1]], 1).is_err());
        assert!(bsp(&[[1, -1]], 1).is_err());
        assert!(bsp(&[[-1, -2]], 1).is_err());
    }

    #[test]
    fn bsp_chain_past_max_depth_fails() {
        let chain = |n: usize| -> Vec<[i32; 2]> {
            (0..n)
                .map(|i| [if i + 1 < n { i as i32 + 1 } else { -1 }, -1])
                .collect()
        };
        assert!(bsp(&chain(MAX_DEPTH), 1).is_ok());
        assert!(bsp(&chain(MAX_DEPTH + 1), 1).is_err());
    }

    fn forest(runs: &[(i64, i64)]) -> Result<()> {
        check_forest(runs.len(), |n| {
            let (first, count) = runs[n];
            first..first + count
        })
    }

    #[test]
    fn forest_passes_and_shared_or_backward_fails() {
        assert!(forest(&[(1, 2), (3, 1), (0, 0), (0, 0)]).is_ok());
        // node i lists i + 1 and i + 2: the Fibonacci DAG
        assert!(forest(&[(1, 2), (2, 2), (3, 1), (0, 0)]).is_err());
        assert!(forest(&[(0, 1)]).is_err());
        assert!(forest(&[(1, 5), (0, 0)]).is_err());
        assert!(forest(&[(-1, 1)]).is_err());
    }

    #[test]
    fn forest_chain_past_max_depth_fails() {
        let chain = |n: usize| -> Vec<(i64, i64)> {
            (0..n as i64)
                .map(|i| if i + 1 < n as i64 { (i + 1, 1) } else { (0, 0) })
                .collect()
        };
        assert!(forest(&chain(MAX_DEPTH)).is_ok());
        assert!(forest(&chain(MAX_DEPTH + 1)).is_err());
    }
}
