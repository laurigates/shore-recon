//! COLMAP sparse model reader, binary format (ADR-0010).
//!
//! Layouts follow COLMAP 4.2.1 `src/colmap/scene/reconstruction_io_binary.cc`:
//! little-endian, each file a `u64` count followed by records. `rigs.bin` and
//! `frames.bin` are not read.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use shoreline_core::{Error, Result};

/// `kInvalidPoint3DId`: a 2D point without a 3D point.
const INVALID_POINT3D: u64 = u64::MAX;

#[derive(Debug, Clone, PartialEq)]
pub struct Camera {
    /// COLMAP `CameraModelId` (2 = `SIMPLE_RADIAL`).
    pub model: i32,
    pub width: u64,
    pub height: u64,
    pub params: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    /// `cam_from_world` rotation as `[w, x, y, z]`.
    pub rotation: [f64; 4],
    /// `cam_from_world` translation.
    pub translation: [f64; 3],
    pub camera_id: u32,
    pub name: String,
    pub points2d: Vec<Point2D>,
}

impl Image {
    /// 2D points that observe a 3D point (COLMAP `Image::NumPoints3D`).
    pub fn num_observations(&self) -> usize {
        self.points2d.iter().filter(|p| p.point3d.is_some()).count()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Point2D {
    pub xy: [f64; 2],
    pub point3d: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Point3D {
    pub xyz: [f64; 3],
    pub rgb: [u8; 3],
    /// Mean reprojection error in pixels; `None` when COLMAP left it unset (-1).
    pub error: Option<f64>,
    /// Observations as `(image id, index into that image's points2d)`.
    pub track: Vec<(u32, u32)>,
}

/// A sparse reconstruction. A model on disk holds only registered images.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Model {
    pub cameras: BTreeMap<u32, Camera>,
    pub images: BTreeMap<u32, Image>,
    pub points3d: BTreeMap<u64, Point3D>,
}

/// Parameter count of each COLMAP camera model, indexed by model id
/// (`src/colmap/sensor/models.h`).
const NUM_PARAMS: [usize; 18] = [3, 4, 4, 5, 8, 8, 12, 5, 4, 5, 12, 16, 4, 5, 3, 4, 6, 2];

impl Model {
    /// Read `cameras.bin`, `images.bin` and `points3D.bin` from `dir` (a
    /// `sparse/<n>` directory), and check that the references between them
    /// resolve.
    pub fn read_binary(dir: &Path) -> Result<Self> {
        let model = Self {
            cameras: read_file(&dir.join("cameras.bin"), read_cameras)?,
            images: read_file(&dir.join("images.bin"), read_images)?,
            points3d: read_file(&dir.join("points3D.bin"), read_points3d)?,
        };
        model.check(dir)?;
        Ok(model)
    }

    fn check(&self, dir: &Path) -> Result<()> {
        let bad = |what: String| Err(model_error(dir, what));
        for (id, image) in &self.images {
            if !self.cameras.contains_key(&image.camera_id) {
                return bad(format!(
                    "image {id} names missing camera {}",
                    image.camera_id
                ));
            }
            let missing = image
                .points2d
                .iter()
                .filter_map(|p| p.point3d)
                .find(|p| !self.points3d.contains_key(p));
            if let Some(p) = missing {
                return bad(format!("image {id} names missing 3D point {p}"));
            }
        }
        for (id, point) in &self.points3d {
            for &(image, idx) in &point.track {
                let observed = self
                    .images
                    .get(&image)
                    .and_then(|i| i.points2d.get(idx as usize));
                if observed.and_then(|p| p.point3d) != Some(*id) {
                    return bad(format!(
                        "3D point {id} lists observation ({image}, {idx}) that does not point back"
                    ));
                }
            }
        }
        Ok(())
    }
}

fn model_error(path: &Path, message: String) -> Error {
    Error::Stage {
        stage: crate::NAME,
        message: format!("COLMAP model {}: {message}", path.display()),
    }
}

fn read_file<T>(path: &Path, parse: fn(&mut Reader) -> Result<T>) -> Result<T> {
    let buf = std::fs::read(path).map_err(|source| Error::Io {
        path: path.to_owned(),
        source,
    })?;
    let mut reader = Reader {
        path: path.to_owned(),
        buf: &buf,
        pos: 0,
    };
    let value = parse(&mut reader)?;
    if reader.pos != buf.len() {
        return Err(reader.error(format!("{} trailing bytes", buf.len() - reader.pos)));
    }
    Ok(value)
}

struct Reader<'a> {
    path: PathBuf,
    buf: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn error(&self, what: String) -> Error {
        model_error(&self.path, format!("{what} at byte {}", self.pos))
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N]> {
        let bytes = self
            .buf
            .get(self.pos..self.pos + N)
            .ok_or_else(|| self.error("truncated".to_owned()))?;
        self.pos += N;
        Ok(bytes.try_into().expect("slice of length N"))
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take::<1>()?[0])
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take()?))
    }
    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take()?))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take()?))
    }
    fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_le_bytes(self.take()?))
    }
    fn f64s<const N: usize>(&mut self) -> Result<[f64; N]> {
        let mut out = [0.0; N];
        for v in &mut out {
            *v = self.f64()?;
        }
        Ok(out)
    }

    /// A record count, bounded by the bytes left so a corrupt count cannot
    /// trigger a huge allocation.
    fn count(&mut self, min_record: usize) -> Result<usize> {
        let n = self.u64()?;
        let left = (self.buf.len() - self.pos) / min_record;
        match usize::try_from(n) {
            Ok(n) if n <= left => Ok(n),
            _ => Err(self.error(format!("truncated: count {n} exceeds the remaining data"))),
        }
    }

    fn cstr(&mut self) -> Result<String> {
        let rest = &self.buf[self.pos..];
        let len = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| self.error("unterminated name".to_owned()))?;
        let name = std::str::from_utf8(&rest[..len])
            .map_err(|_| self.error("name is not UTF-8".to_owned()))?
            .to_owned();
        self.pos += len + 1;
        Ok(name)
    }
}

fn insert<K: Ord + std::fmt::Display, V>(
    r: &Reader,
    map: &mut BTreeMap<K, V>,
    id: K,
    value: V,
) -> Result<()> {
    let message = format!("duplicate id {id}");
    match map.insert(id, value) {
        Some(_) => Err(r.error(message)),
        None => Ok(()),
    }
}

fn read_cameras(r: &mut Reader) -> Result<BTreeMap<u32, Camera>> {
    let mut cameras = BTreeMap::new();
    for _ in 0..r.count(24)? {
        let id = r.u32()?;
        let model = r.i32()?;
        let num_params = usize::try_from(model)
            .ok()
            .and_then(|m| NUM_PARAMS.get(m))
            .ok_or_else(|| r.error(format!("unknown camera model {model}")))?;
        let (width, height) = (r.u64()?, r.u64()?);
        let params = (0..*num_params).map(|_| r.f64()).collect::<Result<_>>()?;
        let camera = Camera {
            model,
            width,
            height,
            params,
        };
        insert(r, &mut cameras, id, camera)?;
    }
    Ok(cameras)
}

fn read_images(r: &mut Reader) -> Result<BTreeMap<u32, Image>> {
    let mut images = BTreeMap::new();
    for _ in 0..r.count(73)? {
        let id = r.u32()?;
        let rotation = r.f64s()?;
        let translation = r.f64s()?;
        let camera_id = r.u32()?;
        let name = r.cstr()?;
        let points2d = (0..r.count(24)?)
            .map(|_| {
                let xy = r.f64s()?;
                let p = r.u64()?;
                Ok(Point2D {
                    xy,
                    point3d: (p != INVALID_POINT3D).then_some(p),
                })
            })
            .collect::<Result<_>>()?;
        let image = Image {
            rotation,
            translation,
            camera_id,
            name,
            points2d,
        };
        insert(r, &mut images, id, image)?;
    }
    Ok(images)
}

fn read_points3d(r: &mut Reader) -> Result<BTreeMap<u64, Point3D>> {
    let mut points = BTreeMap::new();
    for _ in 0..r.count(51)? {
        let id = r.u64()?;
        let xyz = r.f64s()?;
        let rgb = [r.u8()?, r.u8()?, r.u8()?];
        let error = r.f64()?;
        let track = (0..r.count(8)?)
            .map(|_| Ok((r.u32()?, r.u32()?)))
            .collect::<Result<_>>()?;
        let point = Point3D {
            xyz,
            rgb,
            error: (error != -1.0).then_some(error),
            track,
        };
        insert(r, &mut points, id, point)?;
    }
    Ok(points)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Writes `model` in COLMAP's binary layout, as `WriteReconstructionBinary` does.
    pub(crate) fn write_binary(model: &Model, dir: &Path) {
        let mut cameras = (model.cameras.len() as u64).to_le_bytes().to_vec();
        for (id, c) in &model.cameras {
            cameras.extend(id.to_le_bytes());
            cameras.extend(c.model.to_le_bytes());
            cameras.extend(c.width.to_le_bytes());
            cameras.extend(c.height.to_le_bytes());
            c.params
                .iter()
                .for_each(|p| cameras.extend(p.to_le_bytes()));
        }
        let mut images = (model.images.len() as u64).to_le_bytes().to_vec();
        for (id, i) in &model.images {
            images.extend(id.to_le_bytes());
            i.rotation
                .iter()
                .chain(&i.translation)
                .for_each(|v| images.extend(v.to_le_bytes()));
            images.extend(i.camera_id.to_le_bytes());
            images.extend(i.name.as_bytes());
            images.push(0);
            images.extend((i.points2d.len() as u64).to_le_bytes());
            for p in &i.points2d {
                p.xy.iter().for_each(|v| images.extend(v.to_le_bytes()));
                images.extend(p.point3d.unwrap_or(INVALID_POINT3D).to_le_bytes());
            }
        }
        let mut points = (model.points3d.len() as u64).to_le_bytes().to_vec();
        for (id, p) in &model.points3d {
            points.extend(id.to_le_bytes());
            p.xyz.iter().for_each(|v| points.extend(v.to_le_bytes()));
            points.extend(p.rgb);
            points.extend(p.error.unwrap_or(-1.0).to_le_bytes());
            points.extend((p.track.len() as u64).to_le_bytes());
            for (image, idx) in &p.track {
                points.extend(image.to_le_bytes());
                points.extend(idx.to_le_bytes());
            }
        }
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("cameras.bin"), cameras).unwrap();
        std::fs::write(dir.join("images.bin"), images).unwrap();
        std::fs::write(dir.join("points3D.bin"), points).unwrap();
    }

    /// Two images of one camera; 3D points 1 and 2 seen by both, point 3 by
    /// image 1 only. Image 2 also has an unmatched keypoint.
    pub(crate) fn sample() -> Model {
        let pt = |x: f64, p: Option<u64>| Point2D {
            xy: [x, x + 0.5],
            point3d: p,
        };
        let image = |name: &str, points2d| Image {
            rotation: [1.0, 0.0, 0.0, 0.0],
            translation: [0.0, 0.0, 1.0],
            camera_id: 1,
            name: name.into(),
            points2d,
        };
        let point = |error, track| Point3D {
            xyz: [1.0, 2.0, 3.0],
            rgb: [10, 20, 30],
            error,
            track,
        };
        Model {
            cameras: [(
                1,
                Camera {
                    model: 2,
                    width: 3200,
                    height: 1800,
                    params: vec![2080.5, 1600.0, 900.0, -0.03],
                },
            )]
            .into(),
            images: [
                (
                    1,
                    image(
                        "f_0001.jpg",
                        vec![pt(1.0, Some(1)), pt(2.0, Some(2)), pt(3.0, Some(3))],
                    ),
                ),
                (
                    2,
                    image(
                        "f_0002.jpg",
                        vec![pt(4.0, Some(1)), pt(5.0, None), pt(6.0, Some(2))],
                    ),
                ),
            ]
            .into(),
            points3d: [
                (1, point(Some(0.5), vec![(1, 0), (2, 0)])),
                (2, point(Some(1.0), vec![(1, 1), (2, 2)])),
                (3, point(None, vec![(1, 2)])),
            ]
            .into(),
        }
    }

    fn model_err(result: Result<Model>) -> String {
        match result {
            Err(Error::Stage { message, .. }) => message,
            other => panic!("expected a model error, got {other:?}"),
        }
    }

    #[test]
    fn binary_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        write_binary(&sample(), dir.path());
        assert_eq!(Model::read_binary(dir.path()).unwrap(), sample());
    }

    #[test]
    fn observations_skip_unmatched_keypoints() {
        let model = sample();
        assert_eq!(model.images[&1].num_observations(), 3);
        assert_eq!(model.images[&2].num_observations(), 2);
    }

    #[test]
    fn truncated_or_padded_files_are_errors_naming_the_file() {
        let dir = tempfile::tempdir().unwrap();
        write_binary(&sample(), dir.path());
        let images = dir.path().join("images.bin");
        let bytes = std::fs::read(&images).unwrap();

        std::fs::write(&images, &bytes[..bytes.len() - 3]).unwrap();
        let err = model_err(Model::read_binary(dir.path()));
        assert!(
            err.contains("images.bin") && err.contains("truncated"),
            "{err}"
        );

        std::fs::write(&images, [&bytes[..], &[0]].concat()).unwrap();
        let err = model_err(Model::read_binary(dir.path()));
        assert!(err.contains("1 trailing bytes"), "{err}");
    }

    #[test]
    fn huge_count_is_an_error_not_an_allocation() {
        let dir = tempfile::tempdir().unwrap();
        write_binary(&sample(), dir.path());
        let points = dir.path().join("points3D.bin");
        let mut bytes = std::fs::read(&points).unwrap();
        bytes[..8].copy_from_slice(&u64::MAX.to_le_bytes());
        std::fs::write(&points, bytes).unwrap();
        let err = model_err(Model::read_binary(dir.path()));
        assert!(err.contains("exceeds the remaining data"), "{err}");
    }

    #[test]
    fn unknown_camera_model_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut model = sample();
        model.cameras.get_mut(&1).unwrap().model = 18;
        write_binary(&model, dir.path());
        let err = model_err(Model::read_binary(dir.path()));
        assert!(err.contains("unknown camera model 18"), "{err}");
    }

    #[test]
    fn dangling_references_are_errors() {
        type Break = fn(&mut Model);
        let dir = tempfile::tempdir().unwrap();
        let cases: [(Break, &str); 3] = [
            (
                |m| m.images.get_mut(&1).unwrap().camera_id = 9,
                "missing camera 9",
            ),
            (
                |m| m.images.get_mut(&2).unwrap().points2d[1].point3d = Some(7),
                "missing 3D point 7",
            ),
            (
                |m| m.points3d.get_mut(&3).unwrap().track.push((2, 1)),
                "(2, 1)",
            ),
        ];
        for (break_it, needle) in cases {
            let mut model = sample();
            break_it(&mut model);
            write_binary(&model, dir.path());
            let err = model_err(Model::read_binary(dir.path()));
            assert!(err.contains(needle), "{needle}: {err}");
        }
    }

    #[test]
    fn missing_file_is_an_io_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            Model::read_binary(dir.path()),
            Err(Error::Io { path, .. }) if path.ends_with("cameras.bin")
        ));
    }
}
