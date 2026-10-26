//! Indexed triangle meshes with one texture coordinate per vertex, and the
//! cleanup operations of the post stage. Operations return new meshes.
//!
//! Texture seams duplicate a position with different UVs, so connectivity is
//! computed on positions welded by exact equality, not on vertex indices.

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// Triangle list.
    pub indices: Vec<u32>,
}

impl Mesh {
    pub fn new(
        positions: Vec<[f32; 3]>,
        uvs: Vec<[f32; 2]>,
        indices: Vec<u32>,
    ) -> Result<Self, String> {
        if positions.len() != uvs.len() {
            return Err(format!(
                "{} positions but {} texture coordinates",
                positions.len(),
                uvs.len()
            ));
        }
        if indices.len() % 3 != 0 {
            return Err(format!("{} indices is not a triangle list", indices.len()));
        }
        if let Some(i) = indices.iter().find(|&&i| i as usize >= positions.len()) {
            return Err(format!(
                "index {i} out of range ({} vertices)",
                positions.len()
            ));
        }
        Ok(Self {
            positions,
            uvs,
            indices,
        })
    }

    pub fn faces(&self) -> usize {
        self.indices.len() / 3
    }

    fn triangles(&self) -> impl Iterator<Item = [u32; 3]> + '_ {
        self.indices.chunks_exact(3).map(|t| [t[0], t[1], t[2]])
    }

    /// Apply `f` to every position.
    pub fn map_positions(&self, f: impl Fn([f32; 3]) -> [f32; 3]) -> Self {
        Self {
            positions: self.positions.iter().map(|&p| f(p)).collect(),
            ..self.clone()
        }
    }

    /// Keep the faces for which `keep` holds, then drop unreferenced vertices.
    pub fn filter_faces(&self, mut keep: impl FnMut(usize, [u32; 3]) -> bool) -> Self {
        let indices: Vec<u32> = self
            .triangles()
            .enumerate()
            .filter(|&(i, t)| keep(i, t))
            .flat_map(|(_, t)| t)
            .collect();
        self.with_indices(indices)
    }

    /// The mesh with another triangle list over the same vertices, compacted to
    /// the vertices it references (in first-use order).
    fn with_indices(&self, indices: Vec<u32>) -> Self {
        let mut remap = vec![u32::MAX; self.positions.len()];
        let mut positions = Vec::new();
        let mut uvs = Vec::new();
        let indices = indices
            .into_iter()
            .map(|i| {
                let slot = &mut remap[i as usize];
                if *slot == u32::MAX {
                    *slot = positions.len() as u32;
                    positions.push(self.positions[i as usize]);
                    uvs.push(self.uvs[i as usize]);
                }
                *slot
            })
            .collect();
        Self {
            positions,
            uvs,
            indices,
        }
    }

    /// Faces whose three vertices all have `y >= y0` (the plane `y = y0` is up
    /// to `+Y`): the rest is below the water plane.
    pub fn clip_below(&self, y0: f32) -> Self {
        let p = &self.positions;
        self.filter_faces(|_, t| t.iter().all(|&i| p[i as usize][1] >= y0))
    }

    /// Vertices with `y < y0`.
    pub fn vertices_below(&self, y0: f32) -> usize {
        self.positions.iter().filter(|p| p[1] < y0).count()
    }

    /// Connected component of each face, numbered from 0 in order of first
    /// face. Faces sharing a welded position are connected.
    pub fn components(&self) -> Vec<u32> {
        let mut weld: HashMap<[u32; 3], u32> = HashMap::new();
        let welded: Vec<u32> = self
            .positions
            .iter()
            .map(|p| {
                let key = p.map(f32::to_bits);
                let next = weld.len() as u32;
                *weld.entry(key).or_insert(next)
            })
            .collect();
        let mut parent: Vec<u32> = (0..weld.len() as u32).collect();
        fn find(parent: &mut [u32], mut x: u32) -> u32 {
            while parent[x as usize] != x {
                parent[x as usize] = parent[parent[x as usize] as usize];
                x = parent[x as usize];
            }
            x
        }
        for t in self.triangles() {
            let a = find(&mut parent, welded[t[0] as usize]);
            for &v in &t[1..] {
                let b = find(&mut parent, welded[v as usize]);
                if a != b {
                    parent[b as usize] = a;
                }
            }
        }
        let mut label: HashMap<u32, u32> = HashMap::new();
        self.triangles()
            .map(|t| {
                let root = find(&mut parent, welded[t[0] as usize]);
                let next = label.len() as u32;
                *label.entry(root).or_insert(next)
            })
            .collect()
    }

    /// Drop connected components with fewer than `min_fraction` of all faces.
    /// Returns the mesh and the number of components removed.
    pub fn remove_small_components(&self, min_fraction: f64) -> (Self, usize) {
        let labels = self.components();
        let mut sizes: HashMap<u32, usize> = HashMap::new();
        for &l in &labels {
            *sizes.entry(l).or_default() += 1;
        }
        let min = (min_fraction * self.faces() as f64).ceil() as usize;
        let removed = sizes.values().filter(|&&n| n < min).count();
        let mesh = self.filter_faces(|i, _| sizes[&labels[i]] >= min);
        (mesh, removed)
    }

    /// Simplify towards `target_faces` faces with meshoptimizer, which
    /// collapses edges onto existing vertices and keeps texture seams, so
    /// every face keeps UVs from one atlas chart. Returns the mesh and the
    /// relative error reached. Seams can stop it above the target: collapsing
    /// across them leaves faces whose corners sample different charts, which
    /// renders as speckles (ADR-0012).
    pub fn decimate(&self, target_faces: usize) -> (Self, f32) {
        if self.faces() <= target_faces {
            return (self.clone(), 0.0);
        }
        let bytes: Vec<u8> = self
            .positions
            .iter()
            .flatten()
            .flat_map(|x| x.to_le_bytes())
            .collect();
        let adapter = meshopt::VertexDataAdapter::new(&bytes, 12, 0)
            .expect("12-byte stride holds a position at offset 0");
        let mut error = 0.0;
        let indices = meshopt::simplify(
            &self.indices,
            &adapter,
            target_faces * 3,
            1.0,
            meshopt::SimplifyOptions::None,
            Some(&mut error),
        );
        (self.with_indices(indices), error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An `n` x `n` grid of quads in the XZ plane at height `y`, offset by `x0`,
    /// with UVs, as one component.
    fn grid(n: u32, x0: f32, y: f32) -> Mesh {
        let mut positions = Vec::new();
        let mut uvs = Vec::new();
        for j in 0..=n {
            for i in 0..=n {
                positions.push([x0 + i as f32, y, j as f32]);
                uvs.push([i as f32 / n as f32, j as f32 / n as f32]);
            }
        }
        let at = |i: u32, j: u32| j * (n + 1) + i;
        let mut indices = Vec::new();
        for j in 0..n {
            for i in 0..n {
                indices.extend([at(i, j), at(i, j + 1), at(i + 1, j)]);
                indices.extend([at(i + 1, j), at(i, j + 1), at(i + 1, j + 1)]);
            }
        }
        Mesh::new(positions, uvs, indices).unwrap()
    }

    fn concat(a: &Mesh, b: &Mesh) -> Mesh {
        let off = a.positions.len() as u32;
        Mesh::new(
            [a.positions.clone(), b.positions.clone()].concat(),
            [a.uvs.clone(), b.uvs.clone()].concat(),
            a.indices
                .iter()
                .copied()
                .chain(b.indices.iter().map(|i| i + off))
                .collect(),
        )
        .unwrap()
    }

    #[test]
    fn new_rejects_inconsistent_buffers() {
        assert!(Mesh::new(vec![[0.0; 3]], vec![], vec![]).is_err());
        assert!(Mesh::new(vec![[0.0; 3]], vec![[0.0; 2]], vec![0, 0]).is_err());
        assert!(Mesh::new(vec![[0.0; 3]], vec![[0.0; 2]], vec![0, 0, 1]).is_err());
    }

    #[test]
    fn clip_below_drops_faces_touching_the_lower_side_and_compacts() {
        // Two triangles: one at y = 1, one dipping to y = -1.
        let m = Mesh::new(
            vec![
                [0.0, 1.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 1.0],
                [1.0, -1.0, 1.0],
            ],
            vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]],
            vec![0, 1, 2, 1, 3, 2],
        )
        .unwrap();
        let c = m.clip_below(0.0);
        assert_eq!(c.faces(), 1);
        assert_eq!(c.positions.len(), 3);
        assert_eq!(c.uvs, [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]);
        assert_eq!(c.vertices_below(0.0), 0);
        assert_eq!(m.vertices_below(0.0), 1);
    }

    #[test]
    fn components_join_faces_across_duplicated_seam_vertices() {
        // Two triangles sharing an edge by position only (a texture seam).
        let m = Mesh::new(
            vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0],
                [1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0],
                [1.0, 0.0, 1.0],
            ],
            vec![[0.0; 2]; 6],
            vec![0, 1, 2, 3, 5, 4],
        )
        .unwrap();
        assert_eq!(m.components(), [0, 0]);
        let separate = concat(&grid(1, 0.0, 0.0), &grid(1, 5.0, 0.0));
        assert_eq!(separate.components(), [0, 0, 1, 1]);
    }

    #[test]
    fn remove_small_components_keeps_the_large_ones() {
        let big = grid(10, 0.0, 0.0); // 200 faces
        let small = grid(1, 20.0, 0.0); // 2 faces, 2/202 < 0.05
        let (m, removed) = concat(&big, &small).remove_small_components(0.05);
        assert_eq!(removed, 1);
        assert_eq!(m.faces(), 200);
        assert_eq!(m.positions.len(), big.positions.len());
    }

    #[test]
    fn decimate_reaches_the_target_on_a_flat_grid_and_keeps_uvs_valid() {
        let m = grid(40, 0.0, 0.0); // 3200 faces
        let (d, error) = m.decimate(400);
        assert!(d.faces() <= 400, "{} faces", d.faces());
        assert!(d.faces() > 0);
        assert!(error < 1e-3, "flat grid simplifies without error: {error}");
        // Every kept vertex is an original vertex with its original UV.
        for (p, uv) in d.positions.iter().zip(&d.uvs) {
            let i = m.positions.iter().position(|q| q == p).unwrap();
            assert_eq!(&m.uvs[i], uv);
        }
        assert_eq!(m.decimate(10_000).0, m);
    }

    #[test]
    fn decimate_never_collapses_across_seams() {
        // Every face its own UV chart: all vertices are seam duplicates, so
        // nothing can be removed without mixing charts within a face.
        let g = grid(20, 0.0, 0.0); // 800 faces
        let mut positions = Vec::new();
        let mut uvs = Vec::new();
        let mut indices = Vec::new();
        for (f, t) in g.indices.chunks_exact(3).enumerate() {
            for (k, &i) in t.iter().enumerate() {
                indices.push(positions.len() as u32);
                positions.push(g.positions[i as usize]);
                uvs.push([
                    (f % 40) as f32 / 40.0 + k as f32 * 0.01,
                    (f / 40) as f32 / 40.0,
                ]);
            }
        }
        let seamed = Mesh::new(positions, uvs, indices).unwrap();
        let (d, _) = seamed.decimate(200);
        assert!(d.faces() > 700, "{} faces", d.faces());
        // Every remaining face is an input face: positions and UVs together.
        let corner = |m: &Mesh, i: u32| {
            let (p, uv) = (m.positions[i as usize], m.uvs[i as usize]);
            [p[0], p[1], p[2], uv[0], uv[1]].map(f32::to_bits)
        };
        let faces = |m: &Mesh| -> std::collections::HashSet<[[u32; 5]; 3]> {
            m.indices
                .chunks_exact(3)
                .map(|t| [corner(m, t[0]), corner(m, t[1]), corner(m, t[2])])
                .collect()
        };
        assert!(faces(&d).is_subset(&faces(&seamed)));
    }
}
