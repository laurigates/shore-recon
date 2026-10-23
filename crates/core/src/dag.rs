//! Stage dependency graph.

use std::collections::{BTreeMap, BTreeSet};

use crate::error::{Error, Result};

/// Default pipeline: `(stage, dependencies)`.
pub const PIPELINE: &[(&str, &[&str])] = &[
    ("frames", &[]),
    ("masks", &["frames"]),
    ("sfm", &["frames", "masks"]),
    ("dense", &["sfm", "masks"]),
    ("splat", &["sfm", "masks"]),
    ("post", &["dense", "splat"]),
    ("export", &["post"]),
];

/// Topologically sort stages. Ties are broken by declaration order, so the
/// result is stable.
pub fn topo_order<'a>(stages: &[(&'a str, &[&'a str])]) -> Result<Vec<&'a str>> {
    let index: BTreeMap<&str, usize> = stages
        .iter()
        .enumerate()
        .map(|(i, (name, _))| (*name, i))
        .collect();
    if index.len() != stages.len() {
        return Err(Error::Dag("duplicate stage name".into()));
    }

    let mut indegree = vec![0usize; stages.len()];
    let mut dependents = vec![Vec::new(); stages.len()];
    for (i, (name, deps)) in stages.iter().enumerate() {
        for dep in *deps {
            let &d = index
                .get(dep)
                .ok_or_else(|| Error::Dag(format!("{name} depends on unknown stage {dep}")))?;
            indegree[i] += 1;
            dependents[d].push(i);
        }
    }

    let mut ready: BTreeSet<usize> = (0..stages.len()).filter(|&i| indegree[i] == 0).collect();
    let mut order = Vec::with_capacity(stages.len());
    while let Some(i) = ready.pop_first() {
        order.push(stages[i].0);
        for &j in &dependents[i] {
            indegree[j] -= 1;
            if indegree[j] == 0 {
                ready.insert(j);
            }
        }
    }
    if order.len() != stages.len() {
        return Err(Error::Dag("dependency cycle".into()));
    }
    Ok(order)
}

/// `target` and every stage that transitively depends on it.
pub fn dependents_of<'a>(stages: &[(&'a str, &[&'a str])], target: &str) -> BTreeSet<&'a str> {
    let mut out = BTreeSet::new();
    let mut frontier = vec![target];
    while let Some(cur) = frontier.pop() {
        for (name, deps) in stages {
            if deps.contains(&cur) && out.insert(*name) {
                frontier.push(name);
            }
        }
    }
    if let Some((name, _)) = stages.iter().find(|(n, _)| *n == target) {
        out.insert(*name);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_order() {
        assert_eq!(
            topo_order(PIPELINE).unwrap(),
            ["frames", "masks", "sfm", "dense", "splat", "post", "export"]
        );
    }

    #[test]
    fn rejects_cycle() {
        let g: &[(&str, &[&str])] = &[("a", &["b"]), ("b", &["a"])];
        assert!(matches!(topo_order(g), Err(Error::Dag(m)) if m.contains("cycle")));
    }

    #[test]
    fn rejects_unknown_dep() {
        let g: &[(&str, &[&str])] = &[("a", &["x"])];
        assert!(matches!(topo_order(g), Err(Error::Dag(m)) if m.contains("unknown")));
    }

    #[test]
    fn rejects_duplicate() {
        let g: &[(&str, &[&str])] = &[("a", &[]), ("a", &[])];
        assert!(topo_order(g).is_err());
    }

    #[test]
    fn mask_change_invalidates_downstream_only() {
        let affected: Vec<_> = dependents_of(PIPELINE, "masks").into_iter().collect();
        assert_eq!(
            affected,
            ["dense", "export", "masks", "post", "sfm", "splat"]
        );
        assert!(!dependents_of(PIPELINE, "dense").contains("splat"));
    }
}
