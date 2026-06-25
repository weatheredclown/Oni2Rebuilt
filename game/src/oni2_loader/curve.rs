/*
 * oni2_loader/curve.rs — NURBS curve evaluation.
 *
 * NurbsCurve: cubic B-spline for smooth path evaluation.
 * Control points come from layout.paths; knot vector uses multiple end knots.
 * evaluate(t) → Vec3 used by CurveFollower and scripted camera paths.
 */
use bevy::math::{Vec3, Vec2};

/// Cubic B-spline (NURBS) curve for smooth path evaluation.
/// Control points from layout.paths, knot vector generated with multiple end knots.
pub struct NurbsCurve {
    points: Vec<Vec3>,
    kt: Vec<f32>, // knot vector t-values
    nkv: usize,   // length of knot vector
}

impl NurbsCurve {
    /// Build a NURBS curve from control points with multiple end knots (open curve).
    pub fn new(points: Vec<Vec3>) -> Self {
        assert!(points.len() >= 4, "NURBS curves need >= 4 control points");

        let n_points = points.len();
        let n_knots = n_points - 2; // distinct knots (excludes multiplicity)
        let nkv = n_knots + 6;

        let mut kt = vec![0.0f32; nkv];

        // Multiple end knots (open curve):
        // First 3 entries = 0, last 3 = max, interior = 0,1,2,...
        let mut kvi = 0;
        for _ in 0..3 {
            kt[kvi] = 0.0;
            kvi += 1;
        }
        let mut kvj = 0u32;
        for _ in 0..n_knots {
            kt[kvi] = kvj as f32;
            kvi += 1;
            kvj += 1;
        }
        for _ in 0..3 {
            kt[kvi] = (kvj - 1) as f32;
            kvi += 1;
        }

        NurbsCurve { points, kt, nkv }
    }

    /// Evaluate a point on the curve at parameter t ∈ [0, 1].
    pub fn get_curve_point(&self, mut t: f32) -> Vec3 {
        t = t.clamp(0.0, 1.0);
        if t >= 1.0 {
            t = 0.9999;
        }

        // Scale t from [0,1] to curve segment range
        t *= self.nkv as f32 - 6.0 - 1.0;

        // Find span index i where Kt[i] <= t < Kt[i+1]
        let mut i = 3usize;
        while i + 1 < self.nkv && !(self.kt[i] <= t && t < self.kt[i + 1]) {
            i += 1;
        }

        // Evaluate 4 cubic B-spline basis functions
        let b0 = self.blend(i.wrapping_sub(3), t);
        let b1 = self.blend(i.wrapping_sub(2), t);
        let b2 = self.blend(i.wrapping_sub(1), t);
        let b3 = self.blend(i, t);

        self.points[i - 3] * b0
            + self.points[i - 2] * b1
            + self.points[i - 1] * b2
            + self.points[i] * b3
    }

    /// Find the point on the curve closest to `test_pos` in XZ space.
    /// Returns (closest_point_3d, parameter_t).
    pub fn nearest_point_xz(&self, test_pos: Vec3) -> (Vec3, f32) {
        let n_samples = 100;
        let mut best_pt = Vec3::ZERO;
        let mut best_t = 0.0;
        let mut min_dist_xz = f32::MAX;

        // Sample the curve at regular intervals
        let mut samples = Vec::with_capacity(n_samples + 1);
        for i in 0..=n_samples {
            let t = i as f32 / n_samples as f32;
            samples.push((t, self.get_curve_point(t)));
        }

        // Check distance to each segment
        for i in 0..n_samples {
            let (t1, p1) = samples[i];
            let (t2, p2) = samples[i + 1];

            // Project test_pos onto segment p1->p2 in XZ
            let p1_xz = Vec2::new(p1.x, p1.z);
            let p2_xz = Vec2::new(p2.x, p2.z);
            let test_xz = Vec2::new(test_pos.x, test_pos.z);

            let v = p2_xz - p1_xz;
            let w = test_xz - p1_xz;
            let d = v.length_squared();

            let pct = if d > 1e-6 {
                (w.dot(v) / d).clamp(0.0, 1.0)
            } else {
                0.0
            };

            // Interpolated 3D position
            let segment_pt = p1.lerp(p2, pct);
            let dist_xz = Vec2::new(segment_pt.x, segment_pt.z).distance(test_xz);

            if dist_xz < min_dist_xz {
                min_dist_xz = dist_xz;
                best_pt = segment_pt;
                best_t = t1 + pct * (t2 - t1);
            }
        }

        (best_pt, best_t)
    }

    /// Cox-de Boor recursive B-spline basis function of order 4 (degree 3).
    fn blend(&self, i: usize, t: f32) -> f32 {
        let kt = &self.kt;

        // Order 1 (degree 0) basis functions
        let b01 = if kt[i] <= t && t < kt[i + 1] {
            1.0
        } else {
            0.0
        };
        let b11 = if kt[i + 1] <= t && t < kt[i + 2] {
            1.0
        } else {
            0.0
        };
        let b21 = if kt[i + 2] <= t && t < kt[i + 3] {
            1.0
        } else {
            0.0
        };
        let b31 = if kt[i + 3] <= t && t < kt[i + 4] {
            1.0
        } else {
            0.0
        };

        // Order 2
        let b22 = {
            let d1 = kt[i + 3] - kt[i + 2];
            let d2 = kt[i + 4] - kt[i + 3];
            match (d1 != 0.0, d2 != 0.0) {
                (true, true) => ((t - kt[i + 2]) / d1) * b21 + ((kt[i + 4] - t) / d2) * b31,
                (true, false) => ((t - kt[i + 2]) / d1) * b21,
                (false, true) => ((kt[i + 4] - t) / d2) * b31,
                _ => 0.0,
            }
        };
        let b12 = {
            let d1 = kt[i + 2] - kt[i + 1];
            let d2 = kt[i + 3] - kt[i + 2];
            match (d1 != 0.0, d2 != 0.0) {
                (true, true) => ((t - kt[i + 1]) / d1) * b11 + ((kt[i + 3] - t) / d2) * b21,
                (true, false) => ((t - kt[i + 1]) / d1) * b11,
                (false, true) => ((kt[i + 3] - t) / d2) * b21,
                _ => 0.0,
            }
        };
        let b02 = {
            let d1 = kt[i + 1] - kt[i];
            let d2 = kt[i + 2] - kt[i + 1];
            match (d1 != 0.0, d2 != 0.0) {
                (true, true) => ((t - kt[i]) / d1) * b01 + ((kt[i + 2] - t) / d2) * b11,
                (true, false) => ((t - kt[i]) / d1) * b01,
                (false, true) => ((kt[i + 2] - t) / d2) * b11,
                _ => 0.0,
            }
        };

        // Order 3
        let b13 = {
            let d1 = kt[i + 3] - kt[i + 1];
            let d2 = kt[i + 4] - kt[i + 2];
            match (d1 != 0.0, d2 != 0.0) {
                (true, true) => ((t - kt[i + 1]) / d1) * b12 + ((kt[i + 4] - t) / d2) * b22,
                (true, false) => ((t - kt[i + 1]) / d1) * b12,
                (false, true) => ((kt[i + 4] - t) / d2) * b22,
                _ => 0.0,
            }
        };
        let b03 = {
            let d1 = kt[i + 2] - kt[i];
            let d2 = kt[i + 3] - kt[i + 1];
            match (d1 != 0.0, d2 != 0.0) {
                (true, true) => ((t - kt[i]) / d1) * b02 + ((kt[i + 3] - t) / d2) * b12,
                (true, false) => ((t - kt[i]) / d1) * b02,
                (false, true) => ((kt[i + 3] - t) / d2) * b12,
                _ => 0.0,
            }
        };

        // Order 4 (cubic)
        let d1 = kt[i + 3] - kt[i];
        let d2 = kt[i + 4] - kt[i + 1];
        match (d1 != 0.0, d2 != 0.0) {
            (true, true) => ((t - kt[i]) / d1) * b03 + ((kt[i + 4] - t) / d2) * b13,
            (true, false) => ((t - kt[i]) / d1) * b03,
            (false, true) => ((kt[i + 4] - t) / d2) * b13,
            _ => 0.0,
        }
    }
}
