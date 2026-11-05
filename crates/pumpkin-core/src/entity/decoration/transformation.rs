//! Vanilla `com.mojang.math.Transformation.EXTENDED_CODEC`, including the matrix form that
//! `MatrixUtil.svdDecompose` splits into the parts display entities sync.
//!
//! Matrices are column-major like JOML: `m[c][r]` is JOML's `mCR`.
#![expect(
    clippy::imprecise_flops,
    clippy::manual_midpoint,
    reason = "the float math follows JOML and `MatrixUtil` step by step"
)]

use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};

type Matrix3 = [[f32; 3]; 3];
/// `x, y, z, w`, like JOML's `Quaternionf`.
type Quaternion = [f32; 4];

const IDENTITY_QUATERNION: Quaternion = [0.0, 0.0, 0.0, 1.0];

/// The decomposed form display entities store and sync.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transformation {
    pub translation: [f32; 3],
    pub left_rotation: Quaternion,
    pub scale: [f32; 3],
    pub right_rotation: Quaternion,
}

impl Transformation {
    pub const IDENTITY: Self = Self {
        translation: [0.0; 3],
        left_rotation: IDENTITY_QUATERNION,
        scale: [1.0; 3],
        right_rotation: IDENTITY_QUATERNION,
    };

    /// Vanilla `EXTENDED_CODEC`: the four-part compound, or else a 16-float row-major matrix.
    /// `None` when neither form parses.
    #[must_use]
    pub fn from_nbt(tag: &NbtTag) -> Option<Self> {
        if let NbtTag::Compound(compound) = tag {
            return Self::from_parts(compound);
        }
        let values: [f32; 16] = floats(tag)?.try_into().ok()?;
        Some(Self::from_matrix(&values))
    }

    /// Vanilla `Transformation.CODEC`: every part is required.
    fn from_parts(compound: &NbtCompound) -> Option<Self> {
        let vector =
            |name: &str| -> Option<[f32; 3]> { floats(compound.get(name)?)?.try_into().ok() };
        Some(Self {
            translation: vector("translation")?,
            left_rotation: quaternion(compound.get("left_rotation")?)?,
            scale: vector("scale")?,
            right_rotation: quaternion(compound.get("right_rotation")?)?,
        })
    }

    /// `ExtraCodecs.MATRIX4F` followed by `Transformation.ensureDecomposed`.
    fn from_matrix(values: &[f32; 16]) -> Self {
        // `setRowColumn(i >> 2, i & 3, value)`.
        let at = |row: usize, column: usize| values[row * 4 + column];
        // `MatrixUtil.svdDecompose(Matrix4fc, ...)`.
        let scale_factor = 1.0 / at(3, 3);
        let mut input: Matrix3 = [[0.0; 3]; 3];
        for (c, column) in input.iter_mut().enumerate() {
            for (r, value) in column.iter_mut().enumerate() {
                *value = at(r, c) * scale_factor;
            }
        }
        let (left_rotation, scale, right_rotation) = svd_decompose(input);
        Self {
            translation: [
                at(0, 3) * scale_factor,
                at(1, 3) * scale_factor,
                at(2, 3) * scale_factor,
            ],
            left_rotation,
            scale,
            right_rotation,
        }
    }
}

/// The numbers of a list or array tag, read like `Codec.FLOAT.listOf()`.
fn floats(tag: &NbtTag) -> Option<Vec<f32>> {
    match tag {
        NbtTag::List(list) => list.iter().map(NbtTag::as_numeric_float).collect(),
        NbtTag::ByteArray(values) => Some(values.iter().map(|&v| f32::from(v)).collect()),
        NbtTag::IntArray(values) => Some(values.iter().map(|&v| v as f32).collect()),
        NbtTag::LongArray(values) => Some(values.iter().map(|&v| v as f32).collect()),
        _ => None,
    }
}

/// `ExtraCodecs.QUATERNIONF`: four components (normalized), or an `angle` and `axis`.
fn quaternion(tag: &NbtTag) -> Option<Quaternion> {
    if let NbtTag::Compound(compound) = tag {
        let angle = compound.get("angle")?.as_numeric_float()?;
        let axis: [f32; 3] = floats(compound.get("axis")?)?.try_into().ok()?;
        // `Quaternionf(AxisAngle4f)`; JOML stores the angle modulo 2 pi first.
        let two_pi = std::f32::consts::TAU;
        let angle = if angle < 0.0 {
            two_pi + angle % two_pi
        } else {
            angle
        } % two_pi;
        let half = angle * 0.5;
        let sin = half.sin();
        let cos = cos_from_sin(sin, half);
        return Some([axis[0] * sin, axis[1] * sin, axis[2] * sin, cos]);
    }
    let q: Quaternion = floats(tag)?.try_into().ok()?;
    Some(normalize(q))
}

/// JOML `Math.cosFromSin`.
fn cos_from_sin(sin: f32, angle: f32) -> f32 {
    let cos = (1.0 - sin * sin).sqrt();
    let a = angle + std::f32::consts::FRAC_PI_2;
    let two_pi = std::f32::consts::TAU;
    let mut b = a - ((a / two_pi) as i32) as f32 * two_pi;
    if b < 0.0 {
        b += two_pi;
    }
    if b >= std::f32::consts::PI { -cos } else { cos }
}

fn normalize(q: Quaternion) -> Quaternion {
    let inv = 1.0 / (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    [q[0] * inv, q[1] * inv, q[2] * inv, q[3] * inv]
}

/// JOML `Quaternionf.mul`: `a * b`.
fn quaternion_mul(a: Quaternion, b: Quaternion) -> Quaternion {
    let [x, y, z, w] = a;
    let [qx, qy, qz, qw] = b;
    [
        w * qx + (x * qw + (y * qz + -z * qy)),
        w * qy + (-x * qz + (y * qw + z * qx)),
        w * qz + (x * qy + (-y * qx + z * qw)),
        w * qw + (-x * qx + (-y * qy + -z * qz)),
    ]
}

/// JOML `Matrix3f.mul`: `a * b`.
fn mul(a: &Matrix3, b: &Matrix3) -> Matrix3 {
    let mut out = [[0.0; 3]; 3];
    for (c, column) in out.iter_mut().enumerate() {
        for (r, value) in column.iter_mut().enumerate() {
            *value = a[0][r] * b[c][0] + (a[1][r] * b[c][1] + a[2][r] * b[c][2]);
        }
    }
    out
}

fn transpose(m: &Matrix3) -> Matrix3 {
    let mut out = [[0.0; 3]; 3];
    for (c, column) in out.iter_mut().enumerate() {
        for (r, value) in column.iter_mut().enumerate() {
            *value = m[r][c];
        }
    }
    out
}

/// The rotation matrix JOML's `Matrix3f.rotate(Quaternionfc)` multiplies by.
fn rotation_matrix(q: Quaternion) -> Matrix3 {
    let [x, y, z, w] = q;
    let (w2, x2, y2, z2) = (w * w, x * x, y * y, z * z);
    let (zw, xy, xz, yw, yz, xw) = (z * w, x * y, x * z, y * w, y * z, x * w);
    let (dzw, dxy, dxz, dyw, dyz, dxw) = (zw + zw, xy + xy, xz + xz, yw + yw, yz + yz, xw + xw);
    [
        [w2 + x2 - z2 - y2, dxy + dzw, dxz - dyw],
        [dxy - dzw, y2 - z2 + w2 - x2, dyz + dxw],
        [dyw + dxz, dyz - dxw, z2 - y2 - x2 + w2],
    ]
}

/// `GivensParameters`: the half-angle sine and cosine of a rotation.
#[derive(Clone, Copy)]
struct Givens {
    sin_half: f32,
    cos_half: f32,
}

#[derive(Clone, Copy)]
enum Axis {
    X,
    Y,
    Z,
}

impl Givens {
    fn from_unnormalized(sin_half: f32, cos_half: f32) -> Self {
        let w = 1.0 / (sin_half * sin_half + cos_half * cos_half).sqrt();
        Self {
            sin_half: w * sin_half,
            cos_half: w * cos_half,
        }
    }

    /// `MatrixUtil.PI_4`.
    fn pi_4() -> Self {
        let half = std::f32::consts::FRAC_PI_4 / 2.0;
        let sin = half.sin();
        Self {
            sin_half: sin,
            cos_half: cos_from_sin(sin, half),
        }
    }

    const fn inverse(self) -> Self {
        Self {
            sin_half: -self.sin_half,
            cos_half: self.cos_half,
        }
    }

    const fn quaternion(self, axis: Axis) -> Quaternion {
        match axis {
            Axis::X => [self.sin_half, 0.0, 0.0, self.cos_half],
            Axis::Y => [0.0, self.sin_half, 0.0, self.cos_half],
            Axis::Z => [0.0, 0.0, self.sin_half, self.cos_half],
        }
    }

    fn matrix(self, axis: Axis) -> Matrix3 {
        let c = self.cos_half * self.cos_half - self.sin_half * self.sin_half;
        let s = 2.0 * self.sin_half * self.cos_half;
        match axis {
            Axis::X => [[1.0, 0.0, 0.0], [0.0, c, s], [0.0, -s, c]],
            Axis::Y => [[c, 0.0, -s], [0.0, 1.0, 0.0], [s, 0.0, c]],
            Axis::Z => [[c, s, 0.0], [-s, c, 0.0], [0.0, 0.0, 1.0]],
        }
    }
}

/// `MatrixUtil.approxGivensQuat`.
fn approx_givens_quat(a11: f32, a12: f32, a22: f32) -> Givens {
    // `3.0F + 2.0F * Math.sqrt(2.0F)`.
    let g = 3.0 + 2.0 * std::f32::consts::SQRT_2;
    let ch = 2.0 * (a11 - a22);
    if g * a12 * a12 < ch * ch {
        Givens::from_unnormalized(a12, ch)
    } else {
        Givens::pi_4()
    }
}

/// `MatrixUtil.qrGivensQuat`.
fn qr_givens_quat(a1: f32, a2: f32) -> Givens {
    let p = f64::from(a1).hypot(f64::from(a2)) as f32;
    let mut sh = if p > 1.0E-6 { a2 } else { 0.0 };
    let mut ch = a1.abs() + p.max(1.0E-6);
    if a1 < 0.0 {
        std::mem::swap(&mut sh, &mut ch);
    }
    Givens::from_unnormalized(sh, ch)
}

/// `MatrixUtil.similarityTransform`: `qᵀ a q`.
fn similarity_transform(a: &Matrix3, q: &Matrix3) -> Matrix3 {
    mul(&transpose(q), &mul(a, q))
}

/// `MatrixUtil.stepJacobi`.
fn step_jacobi(m: &mut Matrix3, output: &mut Quaternion) {
    if m[0][1] * m[0][1] + m[1][0] * m[1][0] > 1.0E-6 {
        let p = approx_givens_quat(m[0][0], 0.5 * (m[0][1] + m[1][0]), m[1][1]);
        *output = quaternion_mul(*output, p.quaternion(Axis::Z));
        *m = similarity_transform(m, &p.matrix(Axis::Z));
    }
    if m[0][2] * m[0][2] + m[2][0] * m[2][0] > 1.0E-6 {
        let p = approx_givens_quat(m[0][0], 0.5 * (m[0][2] + m[2][0]), m[2][2]).inverse();
        *output = quaternion_mul(*output, p.quaternion(Axis::Y));
        *m = similarity_transform(m, &p.matrix(Axis::Y));
    }
    if m[1][2] * m[1][2] + m[2][1] * m[2][1] > 1.0E-6 {
        let p = approx_givens_quat(m[1][1], 0.5 * (m[1][2] + m[2][1]), m[2][2]);
        *output = quaternion_mul(*output, p.quaternion(Axis::X));
        *m = similarity_transform(m, &p.matrix(Axis::X));
    }
}

/// `MatrixUtil.svdDecompose(Matrix3f, ...)`: `(u, s, v)` with `input = u * diag(s) * v`.
fn svd_decompose(input: Matrix3) -> (Quaternion, [f32; 3], Quaternion) {
    // `eigenvalueJacobi(inputᵀ input, 5, v)`.
    let mut scratch = mul(&transpose(&input), &input);
    let mut v = IDENTITY_QUATERNION;
    for _ in 0..5 {
        step_jacobi(&mut scratch, &mut v);
    }
    v = normalize(v);
    let zero_column_0 = f64::from(scratch[0][0]) < 1.0E-6;
    let zero_column_1 = f64::from(scratch[1][1]) < 1.0E-6;

    let u012s = mul(&input, &rotation_matrix(v));
    let mut u = IDENTITY_QUATERNION;
    let p = if zero_column_0 {
        qr_givens_quat(u012s[1][1], -u012s[1][0])
    } else {
        qr_givens_quat(u012s[0][0], u012s[0][1])
    };
    u = quaternion_mul(u, p.quaternion(Axis::Z));
    let u12s = mul(&transpose(&p.matrix(Axis::Z)), &u012s);

    let p = if zero_column_0 {
        qr_givens_quat(u12s[2][2], -u12s[2][0])
    } else {
        qr_givens_quat(u12s[0][0], u12s[0][2])
    }
    .inverse();
    u = quaternion_mul(u, p.quaternion(Axis::Y));
    let u2s = mul(&transpose(&p.matrix(Axis::Y)), &u12s);

    let p = if zero_column_1 {
        qr_givens_quat(u2s[2][2], -u2s[2][1])
    } else {
        qr_givens_quat(u2s[1][1], u2s[1][2])
    };
    u = quaternion_mul(u, p.quaternion(Axis::X));
    let sm = mul(&transpose(&p.matrix(Axis::X)), &u2s);

    let scale = [sm[0][0], sm[1][1], sm[2][2]];
    // `v.conjugate()`.
    let v = [-v[0], -v[1], -v[2], v[3]];
    (u, scale, v)
}

#[cfg(test)]
mod tests {
    use super::{Matrix3, Quaternion, Transformation, mul, rotation_matrix};
    use pumpkin_nbt::tag::NbtTag;

    fn compose(t: &Transformation) -> Matrix3 {
        let scale = [
            [t.scale[0], 0.0, 0.0],
            [0.0, t.scale[1], 0.0],
            [0.0, 0.0, t.scale[2]],
        ];
        mul(
            &mul(&rotation_matrix(t.left_rotation), &scale),
            &rotation_matrix(t.right_rotation),
        )
    }

    fn matrix_tag(values: [f32; 16]) -> NbtTag {
        NbtTag::List(values.iter().map(|&v| NbtTag::Float(v)).collect())
    }

    /// A rotation around Y, a non-uniform scale and another rotation around X, as a `BDEngine`
    /// export would write them, decompose back into the same matrix.
    #[test]
    fn matrix_form_decomposes_into_the_same_transform() {
        let half = 0.3f32;
        let left: Quaternion = [0.0, half.sin(), 0.0, half.cos()];
        let right: Quaternion = [0.7f32.sin(), 0.0, 0.0, 0.7f32.cos()];
        let expected = Transformation {
            translation: [1.0, 2.0, 3.0],
            left_rotation: left,
            scale: [2.0, 0.5, 1.5],
            right_rotation: right,
        };
        let m = compose(&expected);
        // Row-major, translation in the last column.
        let mut values = [0.0; 16];
        for r in 0..3 {
            for c in 0..3 {
                values[r * 4 + c] = m[c][r];
            }
            values[r * 4 + 3] = expected.translation[r];
        }
        values[15] = 1.0;
        let decomposed = Transformation::from_nbt(&matrix_tag(values)).unwrap();
        assert_eq!(decomposed.translation, expected.translation);
        let recomposed = compose(&decomposed);
        for c in 0..3 {
            for r in 0..3 {
                assert!(
                    (recomposed[c][r] - m[c][r]).abs() < 1.0E-4,
                    "{recomposed:?} != {m:?}"
                );
            }
        }
    }

    #[test]
    fn decomposed_form_needs_every_part() {
        let mut compound = pumpkin_nbt::compound::NbtCompound::new();
        let floats = |v: &[f32]| NbtTag::List(v.iter().map(|&f| NbtTag::Float(f)).collect());
        compound.put("scale", floats(&[2.0, 2.0, 2.0]));
        assert_eq!(
            Transformation::from_nbt(&NbtTag::Compound(compound.clone())),
            None
        );
        compound.put("translation", floats(&[0.0, 1.0, 0.0]));
        compound.put("left_rotation", floats(&[0.0, 0.0, 0.0, 2.0]));
        let mut axis_angle = pumpkin_nbt::compound::NbtCompound::new();
        axis_angle.put_float("angle", std::f32::consts::PI);
        axis_angle.put("axis", floats(&[0.0, 1.0, 0.0]));
        compound.put("right_rotation", NbtTag::Compound(axis_angle));
        let t = Transformation::from_nbt(&NbtTag::Compound(compound)).unwrap();
        assert_eq!(t.left_rotation, [0.0, 0.0, 0.0, 1.0]);
        assert!((t.right_rotation[1] - 1.0).abs() < 1.0E-6 && t.right_rotation[3].abs() < 1.0E-6);
    }
}
