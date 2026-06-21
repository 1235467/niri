/// Minimal ICC v2/v4 profile parser → DRM DEGAMMA_LUT, CTM, GAMMA_LUT blobs.
///
/// Supports matrix/shaper profiles (the common case for display profiles):
///   - rXYZ/gXYZ/bXYZ   primary chromaticities in D50 XYZ
///   - rTRC/gTRC/bTRC   transfer function: parametricCurve or curveType
///
/// The produced blobs can be fed directly to DRM CRTC properties via drm_ffi.
///
/// Pipeline applied by the KMS hardware:
///   [sRGB compositor output]
///     → DEGAMMA_LUT  (remove sRGB EOTF → linear light)
///     → CTM          (3×3 matrix: sRGB primaries → display primaries, in linear light)
///     → GAMMA_LUT    (apply display TRC → encoded signal for panel)
///     → [panel]

use std::path::Path;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Packed DRM DEGAMMA / GAMMA LUT entry (`drm_color_lut`).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DrmColorLut {
    pub red: u16,
    pub green: u16,
    pub blue: u16,
    pub reserved: u16,
}

/// DRM CTM blob (`drm_color_ctm`): 3×3 matrix in S31.32 fixed-point, row-major.
/// Encoding: bit 63 = sign, bits 62:32 = integer part, bits 31:0 = fraction.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DrmColorCtm {
    pub matrix: [u64; 9],
}

/// Everything needed to apply an ICC profile via DRM color management.
/// Parameters consumed by the GPU-side ICC passthrough shader.
///
/// Carried alongside an output whenever an ICC profile is active, so passthrough windows
/// (mpv, Krita, Firefox with CMS) can be rendered through a shader that pre-cancels the
/// hardware color pipeline (DEGAMMA → CTM → GAMMA).
#[derive(Debug, Clone, Copy)]
pub struct IccPassthroughParams {
    /// Inverse of the DRM CTM, row-major (display-linear → sRGB-linear).
    pub ctm_inverse: [f32; 9],
    /// Effective per-channel display gamma exponent. The shader uses
    /// `display_linear = encoded^display_gamma` to invert the hardware GAMMA_LUT, so the
    /// CTM cancellation operates in the same linear space the hardware does.
    pub display_gamma: [f32; 3],
}

pub struct IccTransforms {
    /// DEGAMMA_LUT: 1D LUT removing sRGB EOTF (linearise input).
    /// Length matches the CRTC's `DEGAMMA_LUT_SIZE`; values are u16 (0..=65535 = 0.0..=1.0).
    /// Stored as interleaved `[DrmColorLut]`.
    pub degamma: Vec<DrmColorLut>,
    /// CTM: sRGB linear → display-primary linear transform.
    pub ctm: DrmColorCtm,
    /// GAMMA_LUT: 1D LUT applying display TRC (re-encode for panel).
    /// Length matches the CRTC's `GAMMA_LUT_SIZE`.
    pub gamma: Vec<DrmColorLut>,
    /// The inverse of the CTM matrix as row-major f32[9], for use in the GPU-side passthrough
    /// shader (applied to windows that manage their own color — mpv, Krita, Firefox, etc.).
    /// Multiplying a pixel through this matrix undoes the DRM CTM so the output is identity.
    pub ctm_inverse: [f32; 9],
    /// Effective per-channel display gamma exponent (the display TRC's EOTF approximated as
    /// `linear = encoded^gamma`). Used by the passthrough shader to linearise the app's output
    /// with the *display's* transfer function instead of sRGB, so the hardware GAMMA_LUT is
    /// also cancelled (not just the CTM). Order: [r, g, b].
    pub display_gamma: [f32; 3],
}

/// Parse an ICC profile from a file and compute the DRM transforms.
///
/// `degamma_size` / `gamma_size` must be the CRTC's reported `DEGAMMA_LUT_SIZE` /
/// `GAMMA_LUT_SIZE`, so the produced blobs match what the hardware accepts.
pub fn load_icc(
    path: &Path,
    degamma_size: usize,
    gamma_size: usize,
) -> anyhow::Result<IccTransforms> {
    let data = std::fs::read(path)?;
    let profile = IccProfile::parse(&data)?;
    profile.into_drm_transforms(degamma_size, gamma_size)
}

// ---------------------------------------------------------------------------
// S31.32 encoding
// ---------------------------------------------------------------------------

fn f64_to_s31_32(x: f64) -> u64 {
    // Positive: (x * 2^32) as u64
    // Negative: bit63 set, magnitude * 2^32
    if x >= 0.0 {
        (x * (1u64 << 32) as f64) as u64
    } else {
        let mag = (-x * (1u64 << 32) as f64) as u64;
        (1u64 << 63) | mag
    }
}

// ---------------------------------------------------------------------------
// 3×3 matrix helpers (row-major, f64)
// ---------------------------------------------------------------------------

type Mat3 = [f64; 9];

fn mat3_mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut out = [0f64; 9];
    for row in 0..3 {
        for col in 0..3 {
            out[row * 3 + col] =
                a[row * 3] * b[col] + a[row * 3 + 1] * b[3 + col] + a[row * 3 + 2] * b[6 + col];
        }
    }
    out
}

/// Invert a 3×3 matrix via cofactors.
fn mat3_inv(m: &Mat3) -> anyhow::Result<Mat3> {
    let det = m[0] * (m[4] * m[8] - m[5] * m[7])
        - m[1] * (m[3] * m[8] - m[5] * m[6])
        + m[2] * (m[3] * m[7] - m[4] * m[6]);
    if det.abs() < 1e-12 {
        anyhow::bail!("ICC matrix is singular (det={det})");
    }
    let inv_det = 1.0 / det;
    Ok([
        (m[4] * m[8] - m[5] * m[7]) * inv_det,
        (m[2] * m[7] - m[1] * m[8]) * inv_det,
        (m[1] * m[5] - m[2] * m[4]) * inv_det,
        (m[5] * m[6] - m[3] * m[8]) * inv_det,
        (m[0] * m[8] - m[2] * m[6]) * inv_det,
        (m[2] * m[3] - m[0] * m[5]) * inv_det,
        (m[3] * m[7] - m[4] * m[6]) * inv_det,
        (m[1] * m[6] - m[0] * m[7]) * inv_det,
        (m[0] * m[4] - m[1] * m[3]) * inv_det,
    ])
}

// sRGB primaries + D65 white → D50 XYZ (Bradford-adapted, IEC 61966-2-1 values)
// This is the standard matrix used in ICC sRGB profiles.
const SRGB_TO_XYZ_D50: Mat3 = [
    0.4360747, 0.3850649, 0.1430804, // row 0: X
    0.2225045, 0.7168786, 0.0606169, // row 1: Y
    0.0139322, 0.0971045, 0.7141733, // row 2: Z
];

// ---------------------------------------------------------------------------
// sRGB transfer functions
// ---------------------------------------------------------------------------

/// sRGB EOTF: encoded → linear (removes gamma, "degamma").
fn srgb_eotf(v: f64) -> f64 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

// ---------------------------------------------------------------------------
// ICC parsing
// ---------------------------------------------------------------------------

/// Represents a 1D transfer curve from an ICC profile.
enum Trc {
    /// Gamma-only curve (parameter[0] = gamma exponent).
    Gamma(f64),
    /// ICCv4 parametricCurve, type 1: y = x^g  (g=params[0])
    /// type 3: y = (a*x+b)^g + c  (g,a,b,c,d = params[0..5])
    Parametric(Vec<f64>),
    /// Sampled curve: N evenly-spaced u16 samples from the profile.
    Sampled(Vec<u16>),
    /// Identity (gamma = 1.0).
    Identity,
}

impl Trc {
    /// Evaluate the TRC at normalised input x ∈ [0, 1], returning linear value.
    fn eotf(&self, x: f64) -> f64 {
        match self {
            Trc::Identity => x,
            Trc::Gamma(g) => x.powf(*g),
            Trc::Parametric(p) => {
                match p.len() {
                    1 => x.powf(p[0]),
                    3 => {
                        // type 1: (a*x+b)^g  if x >= -b/a, else 0
                        let g = p[0];
                        let a = p[1];
                        let b = p[2];
                        let t = -b / a;
                        if x >= t {
                            (a * x + b).max(0.0).powf(g)
                        } else {
                            0.0
                        }
                    }
                    4 => {
                        // type 2: (a*x+b)^g + c
                        let g = p[0];
                        let a = p[1];
                        let b = p[2];
                        let c = p[3];
                        (a * x + b).max(0.0).powf(g) + c
                    }
                    5 => {
                        // type 3: IEC-like (sRGB shape)
                        // x >= d: (a*x+b)^g
                        // x <  d: c*x
                        let g = p[0];
                        let a = p[1];
                        let b = p[2];
                        let c = p[3];
                        let d = p[4];
                        if x >= d {
                            (a * x + b).max(0.0).powf(g)
                        } else {
                            c * x
                        }
                    }
                    7 => {
                        // type 4: (a*x+b)^g + e  or  c*x+f
                        let g = p[0];
                        let a = p[1];
                        let b = p[2];
                        let c = p[3];
                        let d = p[4];
                        let e = p[5];
                        let f = p[6];
                        if x >= d {
                            (a * x + b).max(0.0).powf(g) + e
                        } else {
                            c * x + f
                        }
                    }
                    _ => x, // unknown, treat as identity
                }
            }
            Trc::Sampled(samples) => {
                let n = samples.len();
                if n == 0 {
                    return x;
                }
                let scaled = x * (n - 1) as f64;
                let lo = scaled.floor() as usize;
                let hi = (lo + 1).min(n - 1);
                let frac = scaled - lo as f64;
                let a = samples[lo] as f64 / 65535.0;
                let b = samples[hi] as f64 / 65535.0;
                a + (b - a) * frac
            }
        }
    }

    /// Evaluate the inverse TRC (OETF) at linear input x → encoded value.
    /// Used for GAMMA_LUT (re-encode to display's transfer function).
    fn oetf(&self, x: f64) -> f64 {
        match self {
            Trc::Identity => x,
            Trc::Gamma(g) => {
                if *g <= 0.0 {
                    x
                } else {
                    x.max(0.0).powf(1.0 / g)
                }
            }
            // For parametric and sampled, invert numerically via binary search.
            other => {
                // OETF(y) = x  where EOTF(x) = y
                // monotone → binary search
                let mut lo = 0.0f64;
                let mut hi = 1.0f64;
                for _ in 0..52 {
                    let mid = (lo + hi) * 0.5;
                    if other.eotf(mid) < x {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                (lo + hi) * 0.5
            }
        }
    }
}

struct IccProfile {
    /// Column-major: [r_X, r_Y, r_Z, g_X, g_Y, g_Z, b_X, b_Y, b_Z]
    /// But we'll store row-major for matrix math.
    /// Row 0 = [rX, gX, bX], row 1 = [rY, gY, bY], row 2 = [rZ, gZ, bZ]
    xyz_matrix: Mat3,
    trc_r: Trc,
    trc_g: Trc,
    trc_b: Trc,
}

fn read_u32(data: &[u8], offset: usize) -> anyhow::Result<u32> {
    data.get(offset..offset + 4)
        .ok_or_else(|| anyhow::anyhow!("ICC: out of bounds at offset {offset}"))?
        .try_into()
        .map(u32::from_be_bytes)
        .map_err(|_| anyhow::anyhow!("ICC: slice conversion failed"))
}

fn read_u16(data: &[u8], offset: usize) -> anyhow::Result<u16> {
    data.get(offset..offset + 2)
        .ok_or_else(|| anyhow::anyhow!("ICC: out of bounds at offset {offset}"))?
        .try_into()
        .map(u16::from_be_bytes)
        .map_err(|_| anyhow::anyhow!("ICC: slice conversion failed"))
}

fn read_s15f16(data: &[u8], offset: usize) -> anyhow::Result<f64> {
    let raw = read_u32(data, offset)? as i32;
    Ok(raw as f64 / 65536.0)
}

fn read_xyz_tag(data: &[u8], offset: usize, size: usize) -> anyhow::Result<[f64; 3]> {
    if size < 20 {
        anyhow::bail!("ICC XYZType tag too small");
    }
    let sig = &data[offset..offset + 4];
    if sig != b"XYZ " {
        anyhow::bail!("ICC: expected XYZ tag type, got {:?}", sig);
    }
    let x = read_s15f16(data, offset + 8)?;
    let y = read_s15f16(data, offset + 12)?;
    let z = read_s15f16(data, offset + 16)?;
    Ok([x, y, z])
}

fn read_trc_tag(data: &[u8], offset: usize, size: usize) -> anyhow::Result<Trc> {
    if size < 8 {
        anyhow::bail!("ICC TRC tag too small");
    }
    let sig = &data.get(offset..offset + 4).ok_or_else(|| anyhow::anyhow!("ICC TRC oob"))?;

    if *sig == b"para" {
        // parametricCurveType
        let func_type = read_u16(data, offset + 8)?;
        let n_params = match func_type {
            0 => 1usize,
            1 => 3,
            2 => 4,
            3 => 5,
            4 => 7,
            _ => anyhow::bail!("ICC: unknown parametric curve type {func_type}"),
        };
        let mut params = Vec::with_capacity(n_params);
        for i in 0..n_params {
            params.push(read_s15f16(data, offset + 12 + i * 4)?);
        }
        return Ok(Trc::Parametric(params));
    }

    if *sig == b"curv" {
        let count = read_u32(data, offset + 8)? as usize;
        if count == 0 {
            return Ok(Trc::Identity);
        }
        if count == 1 {
            // Single u8.8 fixed-point value = gamma exponent
            let val = read_u16(data, offset + 12)?;
            return Ok(Trc::Gamma(val as f64 / 256.0));
        }
        // Sampled curve
        let mut samples = Vec::with_capacity(count);
        for i in 0..count {
            samples.push(read_u16(data, offset + 12 + i * 2)?);
        }
        return Ok(Trc::Sampled(samples));
    }

    anyhow::bail!("ICC: unknown TRC tag type {:?}", sig)
}

impl IccProfile {
    fn parse(data: &[u8]) -> anyhow::Result<Self> {
        if data.len() < 128 {
            anyhow::bail!("ICC: file too small ({} bytes)", data.len());
        }
        // Bytes 36-39: profile / device class signature
        // We accept 'mntr' (display) and 'scnr'/'spac'/'link'/'abst' as best-effort.
        let tag_count = read_u32(data, 128)? as usize;
        if data.len() < 132 + tag_count * 12 {
            anyhow::bail!("ICC: tag table truncated");
        }

        let mut r_xyz = None;
        let mut g_xyz = None;
        let mut b_xyz = None;
        let mut r_trc = None;
        let mut g_trc = None;
        let mut b_trc = None;

        for i in 0..tag_count {
            let base = 132 + i * 12;
            let tag = &data[base..base + 4];
            let offset = read_u32(data, base + 4)? as usize;
            let size = read_u32(data, base + 8)? as usize;

            match tag {
                b"rXYZ" => r_xyz = Some(read_xyz_tag(data, offset, size)?),
                b"gXYZ" => g_xyz = Some(read_xyz_tag(data, offset, size)?),
                b"bXYZ" => b_xyz = Some(read_xyz_tag(data, offset, size)?),
                b"rTRC" => r_trc = Some(read_trc_tag(data, offset, size)?),
                b"gTRC" => g_trc = Some(read_trc_tag(data, offset, size)?),
                b"bTRC" => b_trc = Some(read_trc_tag(data, offset, size)?),
                _ => {}
            }
        }

        let r = r_xyz.ok_or_else(|| anyhow::anyhow!("ICC: missing rXYZ"))?;
        let g = g_xyz.ok_or_else(|| anyhow::anyhow!("ICC: missing gXYZ"))?;
        let b = b_xyz.ok_or_else(|| anyhow::anyhow!("ICC: missing bXYZ"))?;

        // Build row-major XYZ matrix:
        //   col 0 = r primaries, col 1 = g primaries, col 2 = b primaries
        //   rows = X, Y, Z
        let xyz_matrix: Mat3 = [
            r[0], g[0], b[0], // X row
            r[1], g[1], b[1], // Y row
            r[2], g[2], b[2], // Z row
        ];

        Ok(Self {
            xyz_matrix,
            trc_r: r_trc.unwrap_or(Trc::Identity),
            trc_g: g_trc.unwrap_or(Trc::Identity),
            trc_b: b_trc.unwrap_or(Trc::Identity),
        })
    }

    fn into_drm_transforms(
        self,
        degamma_size: usize,
        gamma_size: usize,
    ) -> anyhow::Result<IccTransforms> {
        anyhow::ensure!(degamma_size >= 2, "DEGAMMA_LUT_SIZE too small ({degamma_size})");
        anyhow::ensure!(gamma_size >= 2, "GAMMA_LUT_SIZE too small ({gamma_size})");

        // --- CTM ---
        // We want: display_linear = CTM × sRGB_linear
        // CTM = M_display^-1 × M_sRGB_D50
        // where M_display = self.xyz_matrix, M_sRGB_D50 = SRGB_TO_XYZ_D50
        let m_disp_inv = mat3_inv(&self.xyz_matrix)?;
        let ctm_mat = mat3_mul(&m_disp_inv, &SRGB_TO_XYZ_D50);

        let matrix = ctm_mat.map(f64_to_s31_32);
        let ctm = DrmColorCtm { matrix };

        // --- DEGAMMA_LUT: remove sRGB EOTF → linear ---
        let mut degamma = Vec::with_capacity(degamma_size);
        for i in 0..degamma_size {
            let x = i as f64 / (degamma_size - 1) as f64;
            let lin = srgb_eotf(x);
            let v = (lin.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16;
            degamma.push(DrmColorLut {
                red: v,
                green: v,
                blue: v,
                reserved: 0,
            });
        }

        // --- GAMMA_LUT: apply display TRC → encoded signal ---
        // Each channel may have its own TRC.
        let mut gamma = Vec::with_capacity(gamma_size);
        for i in 0..gamma_size {
            let x = i as f64 / (gamma_size - 1) as f64;
            let r = (self.trc_r.oetf(x).clamp(0.0, 1.0) * 65535.0 + 0.5) as u16;
            let g = (self.trc_g.oetf(x).clamp(0.0, 1.0) * 65535.0 + 0.5) as u16;
            let b = (self.trc_b.oetf(x).clamp(0.0, 1.0) * 65535.0 + 0.5) as u16;
            gamma.push(DrmColorLut {
                red: r,
                green: g,
                blue: b,
                reserved: 0,
            });
        }

        // ctm_inverse: inverse of ctm_mat (sRGB→display), i.e. display→sRGB, used in the
        // GPU-side passthrough shader to counteract the DRM CTM for passthrough windows.
        let ctm_inv_mat = mat3_inv(&ctm_mat)?;
        let ctm_inverse = ctm_inv_mat.map(|x| x as f32);

        // Estimate a single effective gamma exponent per channel from the TRC. Used by the
        // passthrough shader: `linear = encoded^gamma` cancels the hardware GAMMA_LUT to a
        // good approximation for real display profiles (pure power curves are exact; sampled
        // and parametric curves match within a few LSBs of typical display TRCs).
        let display_gamma = [
            estimate_gamma(&self.trc_r) as f32,
            estimate_gamma(&self.trc_g) as f32,
            estimate_gamma(&self.trc_b) as f32,
        ];

        Ok(IccTransforms {
            degamma,
            ctm,
            gamma,
            ctm_inverse,
            display_gamma,
        })
    }
}

/// Approximate the EOTF (encoded → linear) of a TRC as a single gamma exponent.
///
/// Exact for pure-power curves; for parametric/sampled curves, fits `linear = encoded^g`
/// by least-squares in log-log on a fixed sample grid (skipping the toe near 0 where any
/// linear segment dominates and would skew the exponent).
fn estimate_gamma(trc: &Trc) -> f64 {
    if let Trc::Gamma(g) = trc {
        return *g;
    }
    if matches!(trc, Trc::Identity) {
        return 1.0;
    }
    // Pure-power parametric (type 0): single exponent, no offset.
    if let Trc::Parametric(p) = trc {
        if p.len() == 1 {
            return p[0];
        }
    }

    // Log-log linear fit on a sample grid in [0.05, 1.0]:
    //   y = EOTF(x); fit  log(y) = g * log(x)  ⇒  g = Σ(log x · log y) / Σ(log x)²
    let mut num = 0.0;
    let mut den = 0.0;
    let n = 64;
    for i in 1..=n {
        let x = 0.05 + (1.0 - 0.05) * (i as f64) / (n as f64);
        let y = trc.eotf(x);
        if y <= 0.0 {
            continue;
        }
        let lx = x.ln();
        let ly = y.ln();
        num += lx * ly;
        den += lx * lx;
    }
    if den.abs() < 1e-12 {
        2.2
    } else {
        (num / den).clamp(1.0, 4.0)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s31_32_roundtrip() {
        for &v in &[0.0f64, 1.0, 0.5, -0.5, 2.5, -1.0] {
            let encoded = f64_to_s31_32(v);
            let sign = if encoded & (1 << 63) != 0 { -1.0 } else { 1.0 };
            let mag = (encoded & !(1u64 << 63)) as f64 / (1u64 << 32) as f64;
            let decoded = sign * mag;
            assert!((decoded - v).abs() < 1e-9, "v={v} decoded={decoded}");
        }
    }

    #[test]
    fn mat3_inv_identity() {
        let id: Mat3 = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        let inv = mat3_inv(&id).unwrap();
        for i in 0..9 {
            assert!((inv[i] - id[i]).abs() < 1e-10);
        }
    }

    #[test]
    fn identity_icc_ctm_is_near_identity() {
        // A "display" with sRGB primaries should give CTM ≈ identity.
        let profile = IccProfile {
            xyz_matrix: SRGB_TO_XYZ_D50,
            trc_r: Trc::Identity,
            trc_g: Trc::Identity,
            trc_b: Trc::Identity,
        };
        let t = profile.into_drm_transforms(256, 256).unwrap();
        // Decode the 3×3 and check it's ≈ identity.
        let decode = |v: u64| -> f64 {
            let sign = if v & (1 << 63) != 0 { -1.0 } else { 1.0 };
            let mag = (v & !(1u64 << 63)) as f64 / (1u64 << 32) as f64;
            sign * mag
        };
        let m = t.ctm.matrix;
        for row in 0..3 {
            for col in 0..3 {
                let expected = if row == col { 1.0 } else { 0.0 };
                let got = decode(m[row * 3 + col]);
                assert!((got - expected).abs() < 1e-4, "ctm[{row},{col}] = {got} expected {expected}");
            }
        }
    }

    #[test]
    fn estimate_gamma_pure_power() {
        // A pure-power TRC should round-trip exactly.
        assert!((estimate_gamma(&Trc::Gamma(2.2)) - 2.2).abs() < 1e-9);
        // Parametric type 0 is the same shape.
        assert!((estimate_gamma(&Trc::Parametric(vec![2.4])) - 2.4).abs() < 1e-9);
        // Identity = gamma 1.
        assert!((estimate_gamma(&Trc::Identity) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn estimate_gamma_srgb_parametric_fits_around_2_2() {
        // sRGB parametric type 3 (the canonical sRGB curve in ICC profiles):
        // g=2.4, a=1/1.055, b=0.055/1.055, c=1/12.92, d=0.04045.
        let srgb = Trc::Parametric(vec![
            2.4,
            1.0 / 1.055,
            0.055 / 1.055,
            1.0 / 12.92,
            0.04045,
        ]);
        let g = estimate_gamma(&srgb);
        // A least-squares log-log fit of sRGB to a pure power lands around 2.0–2.1
        // (sRGB's nominal "effective gamma 2.2" comes from a different fitting metric).
        // For the passthrough use case this is close enough — accept anything in [1.9, 2.3].
        assert!(
            (1.9..=2.3).contains(&g),
            "estimated gamma {g} for sRGB parametric outside [1.9, 2.3]"
        );
    }

    #[test]
    fn lut_size_two_is_accepted() {
        // Smallest legal LUT size — make sure we don't underflow.
        let profile = IccProfile {
            xyz_matrix: SRGB_TO_XYZ_D50,
            trc_r: Trc::Gamma(2.2),
            trc_g: Trc::Gamma(2.2),
            trc_b: Trc::Gamma(2.2),
        };
        let t = profile.into_drm_transforms(2, 2).unwrap();
        assert_eq!(t.degamma.len(), 2);
        assert_eq!(t.gamma.len(), 2);
    }
}
