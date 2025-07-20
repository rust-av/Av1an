use tracing::trace;

/// Maximum squared sum of normalized derivatives for PCHIP monotonicity
/// constraint. If alpha^2 + beta^2 > 9, the derivatives are scaled down to
/// preserve monotonicity.
const PCHIP_MAX_TAU_SQUARED: f64 = 9.0;

pub fn linear_interpolate(x: &[f64; 2], y: &[f64; 2], xi: f64) -> Option<f64> {
    // Check strictly increasing
    if x[1] <= x[0] {
        return None;
    }

    // Linear interpolation formula: y = y0 + (y1 - y0) * (xi - x0) / (x1 - x0)
    let t = (xi - x[0]) / (x[1] - x[0]);
    Some(t.mul_add(y[1] - y[0], y[0]))
}

pub fn natural_cubic_spline(x: &[f64], y: &[f64], xi: f64) -> Option<f64> {
    let n = x.len();
    if n < 3 || n != y.len() {
        return None;
    }

    // Noramally, no bounds check is needed - we're interpolating, not extrapolating
    // The target (xi) is a score value we're looking for, not restricted to input
    // range

    // Verify xi is within the observed range (it should be by algorithm design)
    if xi < x[0] || xi > x[n - 1] {
        trace!(
            "Natural cubic spline: unexpected extrapolation case - xi = {xi}, range = [{}, {}]",
            x[0],
            x[n - 1]
        );
        return None;
    }

    // Calculate intervals
    let mut h = vec![0.0; n - 1];
    for i in 0..n - 1 {
        h[i] = x[i + 1] - x[i];
        if h[i] <= 0.0 {
            trace!(
                "Natural cubic spline: x values not strictly increasing at index {i}: {prev} >= \
                 {next}",
                prev = x[i],
                next = x[i + 1]
            );
            return None; // x must be strictly increasing
        }
    }

    // Set up tridiagonal system for second derivatives
    let mut a = vec![0.0; n];
    let mut b = vec![2.0; n];
    let mut c = vec![0.0; n];
    let mut d = vec![0.0; n];

    // Natural boundary conditions: second derivative = 0 at endpoints
    b[0] = 1.0;
    b[n - 1] = 1.0;

    // Interior points
    for i in 1..n - 1 {
        a[i] = h[i - 1];
        b[i] = 2.0 * (h[i - 1] + h[i]);
        c[i] = h[i];
        d[i] = 3.0 * ((y[i + 1] - y[i]) / h[i] - (y[i] - y[i - 1]) / h[i - 1]);
    }

    // Solve tridiagonal system (Thomas algorithm)
    let mut m = vec![0.0; n];
    let mut l = vec![0.0; n];
    let mut z = vec![0.0; n];

    l[0] = b[0];
    if l[0] == 0.0 {
        trace!("Natural cubic spline: Singular matrix at first step");
        return None;
    }
    for i in 1..n {
        l[i] = b[i] - a[i] * c[i - 1] / l[i - 1];
        if l[i] == 0.0 {
            trace!("Natural cubic spline: Singular matrix at step {i}");
            return None;
        }
        z[i] = a[i].mul_add(-z[i - 1], d[i]) / l[i];
    }

    m[n - 1] = z[n - 1];
    for i in (0..n - 1).rev() {
        m[i] = z[i] - c[i] * m[i + 1] / l[i];
    }

    // Find the interval containing xi
    let mut k = 0;
    for i in 0..n - 1 {
        if xi >= x[i] && xi <= x[i + 1] {
            k = i;
            break;
        }
    }

    // Evaluate cubic polynomial
    let dx = xi - x[k];
    let h_k = h[k];

    let a_coeff = y[k];
    let b_coeff = (y[k + 1] - y[k]) / h_k - h_k * 2.0f64.mul_add(m[k], m[k + 1]) / 3.0;
    let c_coeff = m[k];
    let d_coeff = (m[k + 1] - m[k]) / (3.0 * h_k);

    // a_coeff + b_coeff * dx + c_coeff * dx * dx + d_coeff * dx * dx * dx
    Some(b_coeff.mul_add(dx, a_coeff) + c_coeff.mul_add(dx.powi(2), d_coeff * dx.powi(3)))
}

pub fn pchip_interpolate(x: &[f64; 4], y: &[f64; 4], xi: f64) -> Option<f64> {
    // Check strictly increasing
    for i in 0..3 {
        if x[i + 1] <= x[i] {
            return None;
        }
    }

    // Find interval containing xi
    let mut k = 0;
    for i in 0..3 {
        if xi >= x[i] && xi <= x[i + 1] {
            k = i;
            break;
        }
    }

    // Calculate slopes
    let s0 = (y[1] - y[0]) / (x[1] - x[0]);
    let s1 = (y[2] - y[1]) / (x[2] - x[1]);
    let s2 = (y[3] - y[2]) / (x[3] - x[2]);

    // Calculate derivatives using PCHIP method
    let mut d = [0.0; 4];

    // Endpoint derivatives
    d[0] = s0;
    d[3] = s2;

    // Interior derivatives (weighted harmonic mean)
    #[expect(clippy::needless_range_loop)]
    for i in 1..=2 {
        let (s_prev, s_next, h_prev, h_next) = if i == 1 {
            (s0, s1, x[1] - x[0], x[2] - x[1])
        } else {
            (s1, s2, x[2] - x[1], x[3] - x[2])
        };

        if s_prev * s_next <= 0.0 {
            d[i] = 0.0;
        } else {
            let w1 = 2.0f64.mul_add(h_next, h_prev);
            let w2 = 2.0f64.mul_add(h_prev, h_next);
            d[i] = (w1 + w2) / (w1 / s_prev + w2 / s_next);
        }
    }

    // Monotonicity constraint
    let slopes = [s0, s1, s2];
    for i in 0..3 {
        if slopes[i] == 0.0 {
            d[i] = 0.0;
            d[i + 1] = 0.0;
        } else {
            let alpha = d[i] / slopes[i];
            let beta = d[i + 1] / slopes[i];
            let tau = alpha.mul_add(alpha, beta * beta);

            if tau > PCHIP_MAX_TAU_SQUARED {
                let scale = 3.0 / tau.sqrt();
                d[i] = scale * alpha * slopes[i];
                d[i + 1] = scale * beta * slopes[i];
            }
        }
    }

    // Hermite cubic evaluation
    let h = x[k + 1] - x[k];
    let t = (xi - x[k]) / h;
    let t2 = t * t;
    let t3 = t2 * t;

    // (2.0 * t3 - 3.0 * t2 + 1.0) * y[k]
    // + (t3 - 2.0 * t2 + t) * h * d[k]
    // + (-2.0 * t3 + 3.0 * t2) * y[k + 1]
    // + (t3 - t2) * h * d[k + 1],
    Some(
        (2.0f64.mul_add(t3, -(3.0 * t2)) + 1.0)
            .mul_add(y[k], (2.0f64.mul_add(-t2, t3) + t) * h * d[k])
            + (-2.0f64).mul_add(t3, 3.0 * t2).mul_add(y[k + 1], (t3 - t2) * h * d[k + 1]),
    )
}

pub fn quadratic_interpolate(x: &[f64; 3], y: &[f64; 3], xi: f64) -> Option<f64> {
    // Check strictly increasing
    for i in 0..2 {
        if x[i + 1] <= x[i] {
            return None;
        }
    }

    // Verify xi is within the observed range
    if xi < x[0] || xi > x[2] {
        trace!(
            "Quadratic interpolation: unexpected extrapolation case - xi = {xi}, range = [{}, {}]",
            x[0],
            x[2]
        );
        return None;
    }

    // Lagrange interpolation formula for quadratic polynomial
    // L0 = (xi - x1)(xi - x2) / ((x0 - x1)(x0 - x2))
    // L1 = (xi - x0)(xi - x2) / ((x1 - x0)(x1 - x2))
    // L2 = (xi - x0)(xi - x1) / ((x2 - x0)(x2 - x1))
    // P(xi) = y0*L0 + y1*L1 + y2*L2

    let l0 = (xi - x[1]) * (xi - x[2]) / ((x[0] - x[1]) * (x[0] - x[2]));
    let l1 = (xi - x[0]) * (xi - x[2]) / ((x[1] - x[0]) * (x[1] - x[2]));
    let l2 = (xi - x[0]) * (xi - x[1]) / ((x[2] - x[0]) * (x[2] - x[1]));

    // y[0] * l0 + y[1] * l1 + y[2] * l2
    Some(y[2].mul_add(l2, y[0].mul_add(l0, y[1] * l1)))
}

pub fn cubic_polynomial_interpolate(x: &[f64; 4], y: &[f64; 4], xi: f64) -> Option<f64> {
    // Check strictly increasing
    for i in 0..3 {
        if x[i + 1] <= x[i] {
            return None;
        }
    }

    // Verify xi is within the observed range
    if xi < x[0] || xi > x[3] {
        trace!(
            "Cubic polynomial interpolation: unexpected extrapolation case - xi = {xi}, range = \
             [{}, {}]",
            x[0],
            x[3]
        );
        return None;
    }

    // Lagrange interpolation formula for cubic polynomial
    // L0 = (xi - x1)(xi - x2)(xi - x3) / ((x0 - x1)(x0 - x2)(x0 - x3))
    // L1 = (xi - x0)(xi - x2)(xi - x3) / ((x1 - x0)(x1 - x2)(x1 - x3))
    // L2 = (xi - x0)(xi - x1)(xi - x3) / ((x2 - x0)(x2 - x1)(x2 - x3))
    // L3 = (xi - x0)(xi - x1)(xi - x2) / ((x3 - x0)(x3 - x1)(x3 - x2))
    // P(xi) = y0*L0 + y1*L1 + y2*L2 + y3*L3

    let l0 =
        (xi - x[1]) * (xi - x[2]) * (xi - x[3]) / ((x[0] - x[1]) * (x[0] - x[2]) * (x[0] - x[3]));
    let l1 =
        (xi - x[0]) * (xi - x[2]) * (xi - x[3]) / ((x[1] - x[0]) * (x[1] - x[2]) * (x[1] - x[3]));
    let l2 =
        (xi - x[0]) * (xi - x[1]) * (xi - x[3]) / ((x[2] - x[0]) * (x[2] - x[1]) * (x[2] - x[3]));
    let l3 =
        (xi - x[0]) * (xi - x[1]) * (xi - x[2]) / ((x[3] - x[0]) * (x[3] - x[1]) * (x[3] - x[2]));

    // y[0] * l0 + y[1] * l1 + y[2] * l2 + y[3] * l3
    Some(y[0].mul_add(l0, y[1] * l1) + y[2].mul_add(l2, y[3] * l3))
}

pub fn catmull_rom_interpolate(x: &[f64; 4], y: &[f64; 4], xi: f64) -> Option<f64> {
    // Check strictly increasing
    for i in 0..3 {
        if x[i + 1] <= x[i] {
            return None;
        }
    }

    // Find which segment contains xi (between points 1 and 2)
    // We use the inner two points for interpolation, with the outer points for
    // tangent calculation
    if xi < x[1] || xi > x[2] {
        trace!(
            "Catmull-Rom interpolation: xi = {xi} outside interpolation range [{}, {}]",
            x[1],
            x[2]
        );
        return None;
    }

    // Calculate the parameter t for the segment [x1, x2]
    let t = (xi - x[1]) / (x[2] - x[1]);

    // Catmull-Rom basis functions
    let t2 = t * t;
    let t3 = t2 * t;

    // Tension parameter (0.5 for standard Catmull-Rom)
    const TENSION: f64 = 0.5;

    // Calculate tangents at x[1] and x[2]
    // m1 = tension * (y2 - y0) / (x2 - x0)
    // m2 = tension * (y3 - y1) / (x3 - x1)
    let m1 = TENSION * (y[2] - y[0]) / (x[2] - x[0]);
    let m2 = TENSION * (y[3] - y[1]) / (x[3] - x[1]);

    // Hermite basis functions
    let h00 = 2.0f64.mul_add(t3, -(3.0 * t2)) + 1.0;
    let h10 = 2.0f64.mul_add(-t2, t3) + t;
    let h01 = (-2.0f64).mul_add(t3, 3.0 * t2);
    let h11 = t3 - t2;

    // Scale tangents by interval length
    let dx = x[2] - x[1];

    // Interpolate
    // h00 * y[1] + h10 * dx * m1 + h01 * y[2] + h11 * dx * m2
    Some((h11 * dx).mul_add(m2, h00.mul_add(y[1], h01.mul_add(y[2], h10 * dx * m1))))
}

pub fn akima_interpolate(x: &[f64; 4], y: &[f64; 4], xi: f64) -> Option<f64> {
    // Check strictly increasing
    for i in 0..3 {
        if x[i + 1] <= x[i] {
            return None;
        }
    }

    // Verify xi is within the observed range
    if xi < x[0] || xi > x[3] {
        trace!(
            "Akima interpolation: unexpected extrapolation case - xi = {xi}, range = [{}, {}]",
            x[0],
            x[3]
        );
        return None;
    }

    // Find the interval containing xi
    let mut k = 0;
    for i in 0..3 {
        if xi >= x[i] && xi <= x[i + 1] {
            k = i;
            break;
        }
    }

    // Calculate differences
    let mut m = [0.0; 3];
    for i in 0..3 {
        m[i] = (y[i + 1] - y[i]) / (x[i + 1] - x[i]);
    }

    // For 4 points, we need to estimate the slopes at the interior points
    // using a modified Akima method suitable for 4 points
    let mut t = [0.0; 4];

    // Endpoint slopes
    t[0] = m[0];
    t[3] = m[2];

    // Interior point slopes using Akima weights
    // For point 1: use differences m[0] and m[1]
    if (m[1] - m[0]).abs() < 1e-10 {
        t[1] = 0.5 * (m[0] + m[1]);
    } else {
        // For 4 points, we approximate the weights
        let w1 = (m[1] - m[0]).abs();
        let w2 = (m[1] - m[0]).abs(); // Same weight for symmetry
        t[1] = w2.mul_add(m[0], w1 * m[1]) / (w1 + w2);
    }

    // For point 2: use differences m[1] and m[2]
    if (m[2] - m[1]).abs() < 1e-10 {
        t[2] = 0.5 * (m[1] + m[2]);
    } else {
        let w1 = (m[2] - m[1]).abs();
        let w2 = (m[2] - m[1]).abs(); // Same weight for symmetry
        t[2] = w2.mul_add(m[1], w1 * m[2]) / (w1 + w2);
    }

    // Hermite cubic interpolation
    let h = x[k + 1] - x[k];
    let s = (xi - x[k]) / h;
    let s2 = s * s;
    let s3 = s2 * s;

    // Hermite basis functions
    let h00 = 2.0f64.mul_add(s3, -(3.0 * s2)) + 1.0;
    let h10 = 2.0f64.mul_add(-s2, s3) + s;
    let h01 = (-2.0f64).mul_add(s3, 3.0 * s2);
    let h11 = s3 - s2;

    // h00 * y[k] + h10 * h * t[k] + h01 * y[k + 1] + h11 * h * t[k + 1]
    Some(h00.mul_add(
        y[k],
        h10.mul_add(h * t[k], h01.mul_add(y[k + 1], h11 * h * t[k + 1])),
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        akima_interpolate as akima_interpolate_impl,
        catmull_rom_interpolate as catmull_rom_interpolate_impl,
        cubic_polynomial_interpolate as cubic_polynomial_interpolate_impl,
        linear_interpolate as linear_interpolate_impl,
        natural_cubic_spline as natural_cubic_spline_impl,
        pchip_interpolate as pchip_interpolate_impl,
        quadratic_interpolate as quadratic_interpolate_impl,
    };

    #[test]
    fn linear_interpolate() {
        // Test basic linear interpolation using real CRF/score data
        let x = [82.502861, 87.600777]; // scores (ascending order)
        let y = [20.0, 10.0]; // CRFs

        // Test exact points
        assert_eq!(linear_interpolate_impl(&x, &y, 82.502861), Some(20.0));
        assert_eq!(linear_interpolate_impl(&x, &y, 87.600777), Some(10.0));

        // Test midpoint - score 85.051819 should give CRF ~15
        assert!(
            (linear_interpolate_impl(&x, &y, 85.051819).expect("result should exist") - 15.0).abs()
                < 0.1
        );

        // Test interpolation for score 84.0
        let result = linear_interpolate_impl(&x, &y, 84.0);
        assert!(result.is_some());
        assert!(
            result.expect("result should exist") > 15.0
                && result.expect("result should exist") < 20.0
        );

        let x2 = [78.737953, 89.179634]; // scores (ascending order)
        let y2 = [15.0, 5.0]; // CRFs
        assert!(
            (linear_interpolate_impl(&x2, &y2, 83.958794).expect("result should exist") - 10.0)
                .abs()
                < 0.1
        );

        // Test non-increasing x values (should return None)
        let x_bad = [87.600777, 82.502861]; // Not ascending
        let y_bad = [10.0, 20.0];
        assert_eq!(linear_interpolate_impl(&x_bad, &y_bad, 85.0), None);

        // Test equal x values (should return None)
        let x_equal = [85.0, 85.0];
        assert_eq!(linear_interpolate_impl(&x_equal, &y, 85.0), None);
    }

    #[test]
    fn natural_cubic_spline() {
        // CRF 10 (84.872162), CRF 20 (78.517479), CRF 30 (72.812233)
        let x = vec![72.812233, 78.517479, 84.872162]; // scores (ascending order)
        let y = vec![30.0, 20.0, 10.0]; // CRFs

        // Test exact points
        assert!(
            (natural_cubic_spline_impl(&x, &y, 72.812233).expect("result should exist") - 30.0)
                .abs()
                < 1e-10
        );
        assert!(
            (natural_cubic_spline_impl(&x, &y, 78.517479).expect("result should exist") - 20.0)
                .abs()
                < 1e-10
        );
        assert!(
            (natural_cubic_spline_impl(&x, &y, 84.872162).expect("result should exist") - 10.0)
                .abs()
                < 1e-10
        );

        // Test interpolation for score 81.0
        let result = natural_cubic_spline_impl(&x, &y, 81.0);
        assert!(result.is_some());
        assert!(
            result.expect("result should exist") > 10.0
                && result.expect("result should exist") < 20.0
        );

        // CRF 15 (84.864449), CRF 25 (80.161186), CRF 35 (72.134048)
        let x2 = vec![72.134048, 80.161186, 84.864449]; // scores (ascending order)
        let y2 = vec![35.0, 25.0, 15.0]; // CRFs

        // Test interpolation for score 82.0
        let result = natural_cubic_spline_impl(&x2, &y2, 82.0);
        assert!(result.is_some());
        assert!(
            result.expect("result should exist") > 15.0
                && result.expect("result should exist") < 25.0
        );

        // CRF 20 (83.0155), CRF 30 (77.7812), CRF 40 (67.3447)
        let x3 = vec![67.3447, 77.7812, 83.0155]; // scores (ascending order)
        let y3 = vec![40.0, 30.0, 20.0]; // CRFs

        // Test interpolation for score 80.0
        let result = natural_cubic_spline_impl(&x3, &y3, 80.0);
        assert!(result.is_some());
        assert!(
            result.expect("result should exist") > 20.0
                && result.expect("result should exist") < 30.0
        );

        // Test with non-increasing x values (should return None)
        let x_bad = vec![84.872162, 78.517479, 80.0]; // Not properly ordered
        let y_bad = vec![10.0, 20.0, 25.0];
        assert_eq!(natural_cubic_spline_impl(&x_bad, &y_bad, 79.0), None);

        // Test with too few points (should return None)
        let x_short = vec![87.0715, 90.0064];
        let y_short = vec![20.0, 10.0];
        assert_eq!(natural_cubic_spline_impl(&x_short, &y_short, 88.0), None);

        // Test with mismatched lengths (should return None)
        let x_mismatch = vec![83.8005, 87.0715, 90.0064];
        let y_mismatch = vec![30.0, 20.0];
        assert_eq!(
            natural_cubic_spline_impl(&x_mismatch, &y_mismatch, 85.0),
            None
        );
    }

    #[test]
    fn pchip_interpolate() {
        // Test with monotonic data
        // CRF 5 (92.4354), CRF 15 (85.7452), CRF 25 (80.5088), CRF 35 (72.9709)
        let x = [72.9709, 80.5088, 85.7452, 92.4354]; // scores (ascending order)
        let y = [35.0, 25.0, 15.0, 5.0]; // CRFs

        // Test exact points
        assert!(
            (pchip_interpolate_impl(&x, &y, 72.9709).expect("result should exist") - 35.0).abs()
                < 1e-10
        );
        assert!(
            (pchip_interpolate_impl(&x, &y, 80.5088).expect("result should exist") - 25.0).abs()
                < 1e-10
        );
        assert!(
            (pchip_interpolate_impl(&x, &y, 85.7452).expect("result should exist") - 15.0).abs()
                < 1e-10
        );
        assert!(
            (pchip_interpolate_impl(&x, &y, 92.4354).expect("result should exist") - 5.0).abs()
                < 1e-10
        );

        // Test interpolation for score 89.0
        let result = pchip_interpolate_impl(&x, &y, 89.0);
        assert!(result.is_some());
        assert!(
            result.expect("result should exist") > 5.0
                && result.expect("result should exist") < 15.0
        );

        // Test with data that has varying slopes
        // CRF 40 (66.699707), CRF 45 (57.916622), CRF 50 (50.740498), CRF 55
        // (37.303120)
        let x2 = [37.303120, 50.740498, 57.916622, 66.699707]; // scores (ascending order)
        let y2 = [55.0, 50.0, 45.0, 40.0]; // CRFs

        // Should handle the steep changes in score
        let result = pchip_interpolate_impl(&x2, &y2, 54.0);
        assert!(result.is_some());
        assert!(
            result.expect("result should exist") > 45.0
                && result.expect("result should exist") < 50.0
        );

        // Test with non-increasing x values (should return None)
        let x_bad = [72.9709, 88.0, 85.7452, 92.4354]; // Not properly ordered
        let y_bad = [35.0, 12.0, 15.0, 5.0];
        assert_eq!(pchip_interpolate_impl(&x_bad, &y_bad, 87.0), None);

        // Test edge case with nearly flat region
        // CRF 63-66 have very similar scores
        let x_flat = [4.944567, 5.270722, 5.345044, 5.575547]; // scores (ascending order)
        let y_flat = [65.0, 66.0, 64.0, 63.0]; // CRFs
        let result = pchip_interpolate_impl(&x_flat, &y_flat, 5.1);
        assert!(result.is_some());
        // Should handle the nearly flat region gracefully
    }

    #[test]
    fn quadratic_interpolate() {
        // Test with CRF/score data
        // CRF 10 (84.872162), CRF 20 (78.517479), CRF 30 (72.812233)
        let x = [72.812233, 78.517479, 84.872162]; // scores (ascending order)
        let y = [30.0, 20.0, 10.0]; // CRFs

        // Test exact points
        assert!(
            (quadratic_interpolate_impl(&x, &y, 72.812233).expect("result should exist") - 30.0)
                .abs()
                < 1e-10
        );
        assert!(
            (quadratic_interpolate_impl(&x, &y, 78.517479).expect("result should exist") - 20.0)
                .abs()
                < 1e-10
        );
        assert!(
            (quadratic_interpolate_impl(&x, &y, 84.872162).expect("result should exist") - 10.0)
                .abs()
                < 1e-10
        );

        // Test interpolation for score 75.0
        let result = quadratic_interpolate_impl(&x, &y, 75.0);
        assert!(result.is_some());
        let crf = result.expect("result should exist");
        assert!(crf > 20.0 && crf < 30.0);
        // Should be closer to 25 than to 20 or 30
        assert!((crf - 25.0).abs() < 5.0);

        // Test interpolation for score 81.0
        let result = quadratic_interpolate_impl(&x, &y, 81.0);
        assert!(result.is_some());
        assert!(
            result.expect("result should exist") > 10.0
                && result.expect("result should exist") < 20.0
        );

        // Test with another set of CRF data
        // CRF 15 (84.864449), CRF 25 (80.161186), CRF 35 (72.134048)
        let x2 = [72.134048, 80.161186, 84.864449]; // scores (ascending order)
        let y2 = [35.0, 25.0, 15.0]; // CRFs

        // Test exact points
        assert!(
            (quadratic_interpolate_impl(&x2, &y2, 80.161186).expect("result should exist") - 25.0)
                .abs()
                < 1e-10
        );

        // Test interpolation for score 76.0
        let result = quadratic_interpolate_impl(&x2, &y2, 76.0);
        assert!(result.is_some());
        assert!(
            result.expect("result should exist") > 25.0
                && result.expect("result should exist") < 35.0
        );

        // Test with data that has varying slopes
        // CRF 20 (83.0155), CRF 30 (77.7812), CRF 40 (67.3447)
        let x3 = [67.3447, 77.7812, 83.0155]; // scores (ascending order)
        let y3 = [40.0, 30.0, 20.0]; // CRFs

        // Test interpolation for score 80.0
        let result = quadratic_interpolate_impl(&x3, &y3, 80.0);
        assert!(result.is_some());
        let crf = result.expect("result should exist");
        assert!(crf > 20.0 && crf < 30.0);

        // Test with non-increasing x values (should return None)
        let x_bad = [84.872162, 78.517479, 80.0]; // Not properly ordered
        let y_bad = [10.0, 20.0, 25.0];
        assert_eq!(quadratic_interpolate_impl(&x_bad, &y_bad, 79.0), None);

        // Test extrapolation (should return None)
        assert_eq!(quadratic_interpolate_impl(&x, &y, 65.0), None); // Below range
        assert_eq!(quadratic_interpolate_impl(&x, &y, 90.0), None); // Above range
    }

    #[test]
    fn cubic_polynomial_interpolate() {
        // Test with CRF/score data
        // CRF 5 (92.4354), CRF 15 (85.7452), CRF 25 (80.5088), CRF 35 (72.9709)
        let x = [72.9709, 80.5088, 85.7452, 92.4354]; // scores (ascending order)
        let y = [35.0, 25.0, 15.0, 5.0]; // CRFs

        // Test exact points
        assert!(
            (cubic_polynomial_interpolate_impl(&x, &y, 72.9709).expect("result should exist")
                - 35.0)
                .abs()
                < 1e-10
        );
        assert!(
            (cubic_polynomial_interpolate_impl(&x, &y, 80.5088).expect("result should exist")
                - 25.0)
                .abs()
                < 1e-10
        );
        assert!(
            (cubic_polynomial_interpolate_impl(&x, &y, 85.7452).expect("result should exist")
                - 15.0)
                .abs()
                < 1e-10
        );
        assert!(
            (cubic_polynomial_interpolate_impl(&x, &y, 92.4354).expect("result should exist")
                - 5.0)
                .abs()
                < 1e-10
        );

        // Test interpolation for score 89.0
        let result = cubic_polynomial_interpolate_impl(&x, &y, 89.0);
        assert!(result.is_some());
        let crf = result.expect("result should exist");
        assert!(crf > 5.0 && crf < 15.0);
        // Should be closer to 10 than to 5 or 15
        assert!((crf - 10.0).abs() < 5.0);

        // Test interpolation for score 76.0
        let result = cubic_polynomial_interpolate_impl(&x, &y, 76.0);
        assert!(result.is_some());
        assert!(
            result.expect("result should exist") > 25.0
                && result.expect("result should exist") < 35.0
        );

        // Test with another set of CRF data
        // CRF 40 (66.699707), CRF 45 (57.916622), CRF 50 (50.740498), CRF 55
        // (37.303120)
        let x2 = [37.303120, 50.740498, 57.916622, 66.699707]; // scores (ascending order)
        let y2 = [55.0, 50.0, 45.0, 40.0]; // CRFs

        // Test exact points
        assert!(
            (cubic_polynomial_interpolate_impl(&x2, &y2, 50.740498).expect("result should exist")
                - 50.0)
                .abs()
                < 1e-10
        );

        // Test interpolation for score 54.0
        let result = cubic_polynomial_interpolate_impl(&x2, &y2, 54.0);
        assert!(result.is_some());
        assert!(
            result.expect("result should exist") > 45.0
                && result.expect("result should exist") < 50.0
        );

        // Test with data that spans a wider range
        // CRF 10 (88.9), CRF 30 (75.5), CRF 50 (49.2), CRF 70 (5.8)
        let x3 = [5.8, 49.2, 75.5, 88.9]; // scores (ascending order)
        let y3 = [70.0, 50.0, 30.0, 10.0]; // CRFs

        // Test interpolation for score 60.0
        let result = cubic_polynomial_interpolate_impl(&x3, &y3, 60.0);
        assert!(result.is_some());
        assert!(
            result.expect("result should exist") > 30.0
                && result.expect("result should exist") < 50.0
        );

        // Test with non-increasing x values (should return None)
        let x_bad = [72.9709, 88.0, 85.7452, 92.4354]; // Not properly ordered
        let y_bad = [35.0, 12.0, 15.0, 5.0];
        assert_eq!(
            cubic_polynomial_interpolate_impl(&x_bad, &y_bad, 87.0),
            None
        );

        // Test extrapolation (should return None)
        assert_eq!(cubic_polynomial_interpolate_impl(&x, &y, 70.0), None); // Below range
        assert_eq!(cubic_polynomial_interpolate_impl(&x, &y, 95.0), None); // Above range
    }

    #[test]
    fn catmull_rom_interpolate() {
        // Test with CRF/score data
        // CRF 5 (92.4354), CRF 15 (85.7452), CRF 25 (80.5088), CRF 35 (72.9709)
        let x = [72.9709, 80.5088, 85.7452, 92.4354]; // scores (ascending order)
        let y = [35.0, 25.0, 15.0, 5.0]; // CRFs

        // Test exact points (at x[1] and x[2])
        assert!(
            (catmull_rom_interpolate_impl(&x, &y, 80.5088).expect("result should exist") - 25.0)
                .abs()
                < 1e-10
        );
        assert!(
            (catmull_rom_interpolate_impl(&x, &y, 85.7452).expect("result should exist") - 15.0)
                .abs()
                < 1e-10
        );

        // Test interpolation between x[1] and x[2]
        let result = catmull_rom_interpolate_impl(&x, &y, 83.0);
        assert!(result.is_some());
        let crf = result.expect("result should exist");
        assert!(crf > 15.0 && crf < 25.0);
        // Should be close to 20
        assert!((crf - 20.0).abs() < 2.0);

        // Test with another set of CRF data
        // CRF 40 (66.699707), CRF 45 (57.916622), CRF 50 (50.740498), CRF 55
        // (37.303120)
        let x2 = [37.303120, 50.740498, 57.916622, 66.699707]; // scores (ascending order)
        let y2 = [55.0, 50.0, 45.0, 40.0]; // CRFs

        // Test exact points
        assert!(
            (catmull_rom_interpolate_impl(&x2, &y2, 50.740498).expect("result should exist")
                - 50.0)
                .abs()
                < 1e-10
        );
        assert!(
            (catmull_rom_interpolate_impl(&x2, &y2, 57.916622).expect("result should exist")
                - 45.0)
                .abs()
                < 1e-10
        );

        // Test interpolation for score 54.0
        let result = catmull_rom_interpolate_impl(&x2, &y2, 54.0);
        assert!(result.is_some());
        assert!(
            result.expect("result should exist") > 45.0
                && result.expect("result should exist") < 50.0
        );

        // Test with data that has varying slopes
        // CRF 10 (88.9), CRF 30 (75.5), CRF 50 (49.2), CRF 70 (5.8)
        let x3 = [5.8, 49.2, 75.5, 88.9]; // scores (ascending order)
        let y3 = [70.0, 50.0, 30.0, 10.0]; // CRFs

        // Test interpolation between middle points
        let result = catmull_rom_interpolate_impl(&x3, &y3, 60.0);
        assert!(result.is_some());
        assert!(
            result.expect("result should exist") > 30.0
                && result.expect("result should exist") < 50.0
        );

        // Test with narrow CRF range
        // CRF 18 (82.5), CRF 20 (81.0), CRF 22 (79.5), CRF 24 (78.0)
        let x4 = [78.0, 79.5, 81.0, 82.5]; // scores (ascending order)
        let y4 = [24.0, 22.0, 20.0, 18.0]; // CRFs

        // Test interpolation for score 80.0
        let result = catmull_rom_interpolate_impl(&x4, &y4, 80.0);
        assert!(result.is_some());
        let crf = result.expect("result should exist");
        assert!(crf > 20.0 && crf < 22.0);

        // Test with non-increasing x values (should return None)
        let x_bad = [72.9709, 88.0, 85.7452, 92.4354]; // Not properly ordered
        let y_bad = [35.0, 12.0, 15.0, 5.0];
        assert_eq!(catmull_rom_interpolate_impl(&x_bad, &y_bad, 87.0), None);

        // Test outside interpolation range (should return None)
        // Note: Catmull-Rom only interpolates between x[1] and x[2]
        assert_eq!(catmull_rom_interpolate_impl(&x, &y, 75.0), None); // Before x[1]
        assert_eq!(catmull_rom_interpolate_impl(&x, &y, 90.0), None); // After x[2]
    }

    #[test]
    fn akima_interpolate() {
        // Test with CRF/score data
        // CRF 5 (92.4354), CRF 15 (85.7452), CRF 25 (80.5088), CRF 35 (72.9709)
        let x = [72.9709, 80.5088, 85.7452, 92.4354]; // scores (ascending order)
        let y = [35.0, 25.0, 15.0, 5.0]; // CRFs

        // Test exact points
        assert!(
            (akima_interpolate_impl(&x, &y, 72.9709).expect("result should exist") - 35.0).abs()
                < 1e-10
        );
        assert!(
            (akima_interpolate_impl(&x, &y, 80.5088).expect("result should exist") - 25.0).abs()
                < 1e-10
        );
        assert!(
            (akima_interpolate_impl(&x, &y, 85.7452).expect("result should exist") - 15.0).abs()
                < 1e-10
        );
        assert!(
            (akima_interpolate_impl(&x, &y, 92.4354).expect("result should exist") - 5.0).abs()
                < 1e-10
        );

        // Test interpolation for score 89.0
        let result = akima_interpolate_impl(&x, &y, 89.0);
        assert!(result.is_some());
        let crf = result.expect("result should exist");
        assert!(crf > 5.0 && crf < 15.0);

        // Test interpolation for score 76.0
        let result = akima_interpolate_impl(&x, &y, 76.0);
        assert!(result.is_some());
        assert!(
            result.expect("result should exist") > 25.0
                && result.expect("result should exist") < 35.0
        );

        // Test with another set of CRF data
        // CRF 40 (66.699707), CRF 45 (57.916622), CRF 50 (50.740498), CRF 55
        // (37.303120)
        let x2 = [37.303120, 50.740498, 57.916622, 66.699707]; // scores (ascending order)
        let y2 = [55.0, 50.0, 45.0, 40.0]; // CRFs

        // Test interpolation for score 54.0
        let result = akima_interpolate_impl(&x2, &y2, 54.0);
        assert!(result.is_some());
        assert!(
            result.expect("result should exist") > 45.0
                && result.expect("result should exist") < 50.0
        );

        // Test with data that has a flat region
        // CRF 20 (83.0), CRF 20 (82.0), CRF 22 (79.0), CRF 24 (76.0)
        let x3 = [76.0, 79.0, 82.0, 83.0]; // scores (ascending order)
        let y3 = [24.0, 22.0, 20.0, 20.0]; // CRFs (note the flat region at end)

        // Should handle the flat region gracefully
        let result = akima_interpolate_impl(&x3, &y3, 82.5);
        assert!(result.is_some());

        // Test with non-increasing x values (should return None)
        let x_bad = [72.9709, 88.0, 85.7452, 92.4354]; // Not properly ordered
        let y_bad = [35.0, 12.0, 15.0, 5.0];
        assert_eq!(akima_interpolate_impl(&x_bad, &y_bad, 87.0), None);

        // Test extrapolation (should return None)
        assert_eq!(akima_interpolate_impl(&x, &y, 70.0), None); // Below range
        assert_eq!(akima_interpolate_impl(&x, &y, 95.0), None); // Above range
    }
}
