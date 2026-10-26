//! Binary glTF in and out, for the one shape the pipeline produces: a single
//! textured triangle mesh (OpenMVS `TextureMesh --export-type glb`: positions,
//! `TEXCOORD_0`, `u32` indices, one base colour texture), plus on output a
//! separate water mesh with its own material.

use std::path::Path;

use shoreline_core::error::io_error;
use shoreline_core::{Error, Result};

use crate::mesh::Mesh;

/// An encoded image and its MIME type, as stored in a glTF buffer view.
#[derive(Debug, Clone, PartialEq)]
pub struct Texture {
    pub bytes: Vec<u8>,
    pub mime: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TexturedMesh {
    pub mesh: Mesh,
    pub texture: Texture,
}

/// The water surface: a planar polygon (counter-clockwise seen from +Y) and an
/// RGBA base colour, rendered with alpha blending.
#[derive(Debug, Clone, PartialEq)]
pub struct Water {
    pub outline: Vec<[f32; 3]>,
    pub color: [f32; 4],
}

fn glb_error(path: &Path, message: impl std::fmt::Display) -> Error {
    Error::Stage {
        stage: crate::NAME,
        message: format!("{}: {message}", path.display()),
    }
}

/// Read the first primitive of the first mesh, which must be triangles with
/// `TEXCOORD_0` and a base colour texture. An image given by URI is read
/// relative to the file.
pub fn read(path: &Path) -> Result<TexturedMesh> {
    let bytes = std::fs::read(path).map_err(io_error(path))?;
    let gltf = gltf::Gltf::from_slice(&bytes).map_err(|e| glb_error(path, e))?;
    let blob = gltf.blob.as_deref();
    let buffer = |b: gltf::Buffer<'_>| match b.source() {
        gltf::buffer::Source::Bin => blob,
        gltf::buffer::Source::Uri(_) => None,
    };
    let mesh = gltf
        .meshes()
        .next()
        .ok_or_else(|| glb_error(path, "no mesh"))?;
    let prim = mesh
        .primitives()
        .next()
        .ok_or_else(|| glb_error(path, "mesh has no primitive"))?;
    if prim.mode() != gltf::mesh::Mode::Triangles {
        return Err(glb_error(path, "primitive is not a triangle list"));
    }
    let reader = prim.reader(buffer);
    let positions: Vec<[f32; 3]> = reader
        .read_positions()
        .ok_or_else(|| glb_error(path, "no POSITION"))?
        .collect();
    let uvs: Vec<[f32; 2]> = reader
        .read_tex_coords(0)
        .ok_or_else(|| glb_error(path, "no TEXCOORD_0"))?
        .into_f32()
        .collect();
    let indices: Vec<u32> = reader
        .read_indices()
        .ok_or_else(|| glb_error(path, "no indices"))?
        .into_u32()
        .collect();
    let mesh = Mesh::new(positions, uvs, indices).map_err(|e| glb_error(path, e))?;

    let texture = prim
        .material()
        .pbr_metallic_roughness()
        .base_color_texture()
        .ok_or_else(|| glb_error(path, "material has no base colour texture"))?
        .texture()
        .source();
    let texture = match texture.source() {
        gltf::image::Source::View { view, mime_type } => {
            let data = buffer(view.buffer())
                .ok_or_else(|| glb_error(path, "texture view is not in the GLB blob"))?;
            Texture {
                bytes: data[view.offset()..view.offset() + view.length()].to_vec(),
                mime: mime_type.to_owned(),
            }
        }
        gltf::image::Source::Uri { uri, mime_type } => {
            if uri.contains(':') || uri.starts_with('/') {
                return Err(glb_error(path, format!("unsupported image URI {uri:?}")));
            }
            let file = path.parent().unwrap_or(Path::new(".")).join(uri);
            let mime = match mime_type {
                Some(m) => m.to_owned(),
                None => mime_from_extension(uri)
                    .ok_or_else(|| glb_error(path, format!("unknown image type {uri:?}")))?
                    .to_owned(),
            };
            Texture {
                bytes: std::fs::read(&file).map_err(io_error(&file))?,
                mime,
            }
        }
    };
    Ok(TexturedMesh { mesh, texture })
}

fn mime_from_extension(uri: &str) -> Option<&'static str> {
    let ext = uri.rsplit('.').next()?.to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        _ => None,
    }
}

/// Appends 4-byte-aligned buffer views to one binary chunk.
#[derive(Default)]
struct Bin {
    data: Vec<u8>,
    views: Vec<serde_json::Value>,
}

impl Bin {
    fn push(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        while self.data.len() % 4 != 0 {
            self.data.push(0);
        }
        let mut view = serde_json::json!({
            "buffer": 0,
            "byteOffset": self.data.len(),
            "byteLength": bytes.len(),
        });
        if let Some(t) = target {
            view["target"] = t.into();
        }
        self.data.extend_from_slice(bytes);
        self.views.push(view);
        self.views.len() - 1
    }
}

const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;
const FLOAT: u32 = 5126;
const UNSIGNED_INT: u32 = 5125;

fn f32_bytes<const N: usize>(v: &[[f32; N]]) -> Vec<u8> {
    v.iter().flatten().flat_map(|x| x.to_le_bytes()).collect()
}

fn u32_bytes(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn bounds(points: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for p in points {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    (lo, hi)
}

/// Encode a GLB: node `mesh` (unlit, textured, double-sided) and, if given,
/// node `water` (a fan-triangulated polygon, alpha-blended PBR material). The
/// texture is embedded.
pub fn encode(model: &TexturedMesh, water: Option<&Water>) -> Vec<u8> {
    use serde_json::json;
    let mut bin = Bin::default();
    let mut accessors = Vec::new();
    let mesh = &model.mesh;

    let (lo, hi) = bounds(&mesh.positions);
    let v = bin.push(&f32_bytes(&mesh.positions), Some(ARRAY_BUFFER));
    accessors.push(
        json!({"bufferView": v, "componentType": FLOAT, "count": mesh.positions.len(),
                          "type": "VEC3", "min": lo, "max": hi}),
    );
    let v = bin.push(&f32_bytes(&mesh.uvs), Some(ARRAY_BUFFER));
    accessors.push(
        json!({"bufferView": v, "componentType": FLOAT, "count": mesh.uvs.len(), "type": "VEC2"}),
    );
    let v = bin.push(&u32_bytes(&mesh.indices), Some(ELEMENT_ARRAY_BUFFER));
    accessors.push(json!({"bufferView": v, "componentType": UNSIGNED_INT, "count": mesh.indices.len(), "type": "SCALAR"}));
    let image_view = bin.push(&model.texture.bytes, None);

    let mut meshes = vec![json!({"name": "mesh", "primitives": [
        {"attributes": {"POSITION": 0, "TEXCOORD_0": 1}, "indices": 2, "material": 0, "mode": 4}]})];
    let mut materials = vec![json!({
        "name": "surface",
        "doubleSided": true,
        "pbrMetallicRoughness": {"baseColorTexture": {"index": 0}, "metallicFactor": 0.0, "roughnessFactor": 1.0},
        "extensions": {"KHR_materials_unlit": {}},
    })];
    let mut nodes = vec![json!({"name": "mesh", "mesh": 0})];

    if let Some(w) = water {
        let n = w.outline.len() as u32;
        let indices: Vec<u32> = (1..n.saturating_sub(1))
            .flat_map(|i| [0, i, i + 1])
            .collect();
        let normals = vec![[0.0f32, 1.0, 0.0]; w.outline.len()];
        let (lo, hi) = bounds(&w.outline);
        let base = accessors.len();
        let v = bin.push(&f32_bytes(&w.outline), Some(ARRAY_BUFFER));
        accessors.push(
            json!({"bufferView": v, "componentType": FLOAT, "count": w.outline.len(),
                              "type": "VEC3", "min": lo, "max": hi}),
        );
        let v = bin.push(&f32_bytes(&normals), Some(ARRAY_BUFFER));
        accessors.push(json!({"bufferView": v, "componentType": FLOAT, "count": normals.len(), "type": "VEC3"}));
        let v = bin.push(&u32_bytes(&indices), Some(ELEMENT_ARRAY_BUFFER));
        accessors.push(json!({"bufferView": v, "componentType": UNSIGNED_INT, "count": indices.len(), "type": "SCALAR"}));
        meshes.push(json!({"name": "water", "primitives": [
            {"attributes": {"POSITION": base, "NORMAL": base + 1}, "indices": base + 2, "material": 1, "mode": 4}]}));
        materials.push(json!({
            "name": "water",
            "doubleSided": true,
            "alphaMode": "BLEND",
            "pbrMetallicRoughness": {"baseColorFactor": w.color, "metallicFactor": 0.0, "roughnessFactor": 0.15},
        }));
        nodes.push(json!({"name": "water", "mesh": 1}));
    }

    let doc = json!({
        "asset": {"version": "2.0", "generator": "shoreline-recon"},
        "extensionsUsed": ["KHR_materials_unlit"],
        "scene": 0,
        "scenes": [{"name": "scene", "nodes": (0..nodes.len()).collect::<Vec<_>>()}],
        "nodes": nodes,
        "meshes": meshes,
        "materials": materials,
        "textures": [{"sampler": 0, "source": 0}],
        "samplers": [{"magFilter": 9729, "minFilter": 9987, "wrapS": 33071, "wrapT": 33071}],
        "images": [{"bufferView": image_view, "mimeType": model.texture.mime}],
        "accessors": accessors,
        "bufferViews": bin.views,
        "buffers": [{"byteLength": bin.data.len()}],
    });

    let mut json = serde_json::to_vec(&doc).expect("glTF JSON serialises");
    while json.len() % 4 != 0 {
        json.push(b' ');
    }
    while bin.data.len() % 4 != 0 {
        bin.data.push(0);
    }
    let total = 12 + 8 + json.len() + 8 + bin.data.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json);
    out.extend_from_slice(&(bin.data.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin.data);
    out
}

pub fn write(path: &Path, model: &TexturedMesh, water: Option<&Water>) -> Result<()> {
    std::fs::write(path, encode(model, water)).map_err(io_error(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad() -> TexturedMesh {
        let mesh = Mesh::new(
            vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 0.0, 1.0],
                [0.0, 0.0, 1.0],
            ],
            vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            vec![0, 1, 2, 0, 2, 3],
        )
        .unwrap();
        TexturedMesh {
            mesh,
            texture: Texture {
                bytes: vec![1, 2, 3, 4, 5],
                mime: "image/png".into(),
            },
        }
    }

    #[test]
    fn encode_then_read_round_trips_the_mesh_and_embedded_texture() {
        let model = quad();
        let water = Water {
            outline: vec![
                [-1.0, 0.0, 2.0],
                [2.0, 0.0, 2.0],
                [2.0, 0.0, -1.0],
                [-1.0, 0.0, -1.0],
            ],
            color: [0.1, 0.2, 0.3, 0.7],
        };
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("m.glb");
        write(&path, &model, Some(&water)).unwrap();
        let back = read(&path).unwrap();
        assert_eq!(back, model);

        // The water node is separate, with its own blended material.
        let gltf = gltf::Gltf::open(&path).unwrap();
        let names: Vec<_> = gltf
            .document
            .as_json()
            .nodes
            .iter()
            .map(|n| n.mesh.unwrap().value())
            .collect();
        assert_eq!(names, [0, 1]);
        let water_mat = gltf.materials().nth(1).unwrap();
        assert_eq!(water_mat.alpha_mode(), gltf::material::AlphaMode::Blend);
        let w = gltf.meshes().nth(1).unwrap().primitives().next().unwrap();
        let blob = gltf.blob.as_deref();
        let idx: Vec<u32> = w
            .reader(|_| blob)
            .read_indices()
            .unwrap()
            .into_u32()
            .collect();
        assert_eq!(idx, [0, 1, 2, 0, 2, 3]);
    }

    #[test]
    fn read_resolves_an_image_uri_next_to_the_file() {
        // Rewrite the embedded image as a URI, as OpenMVS writes it.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("m.glb");
        let glb = encode(&quad(), None);
        let json_len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        let mut doc: serde_json::Value = serde_json::from_slice(&glb[20..20 + json_len]).unwrap();
        doc["images"] = serde_json::json!([{"uri": "tex_0.png"}]);
        let mut json = serde_json::to_vec(&doc).unwrap();
        while json.len() % 4 != 0 {
            json.push(b' ');
        }
        let bin = &glb[20 + json_len..];
        let mut out = b"glTF".to_vec();
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&((12 + 8 + json.len() + bin.len()) as u32).to_le_bytes());
        out.extend_from_slice(&(json.len() as u32).to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(&json);
        out.extend_from_slice(bin);
        std::fs::write(&path, out).unwrap();
        std::fs::write(tmp.path().join("tex_0.png"), b"PNGDATA").unwrap();

        let back = read(&path).unwrap();
        assert_eq!(back.texture.bytes, b"PNGDATA");
        assert_eq!(back.texture.mime, "image/png");
        assert_eq!(back.mesh, quad().mesh);
    }
}
