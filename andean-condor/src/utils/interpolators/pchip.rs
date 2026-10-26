/// Maximum squared sum of normalized derivatives for PCHIP monotonicity
/// constraint. If alpha^2 + beta^2 > 9, the derivatives are scaled down to
/// preserve monotonicity.
const PCHIP_MAX_TAU_SQUARED: f64 = 9.0;

/// Piecewise Cubic Hermite Interpolating Polynomial over any number of points.
///
/// The interior derivatives are weighted harmonic means of the neighbouring
/// secant slopes, and the monotonicity constraint scales them back wherever
/// they would otherwise make the interpolant overshoot. The result is a smooth
/// curve that never leaves the range the samples actually span, which is what
/// keeps a predicted quantizer inside the probed range.
#[inline]
pub fn pchip(x: &[f64], y: &[f64], xi: f64) -> Option<f64> {
    let n = x.len();
    if n < 2 || y.len() != n {
        return None;
    }

    // Check strictly increasing
    for i in 0..n - 1 {
        if x[i + 1] <= x[i] {
            return None;
        }
    }

    // Verify xi is within the observed range
    if xi < x[0] || xi > x[n - 1] {
        return None;
    }

    // Find interval containing xi
    let k = (0..n - 1).find(|&i| xi >= x[i] && xi <= x[i + 1]).unwrap_or(0);

    // Secant slope of every interval
    let s: Vec<f64> = (0..n - 1).map(|i| (y[i + 1] - y[i]) / (x[i + 1] - x[i])).collect();

    // Derivatives at every knot
    let mut d = vec![0.0; n];

    // Endpoint derivatives
    d[0] = s[0];
    d[n - 1] = s[n - 2];

    // Interior derivatives (weighted harmonic mean)
    for i in 1..n - 1 {
        let s_prev = s[i - 1];
        let s_next = s[i];
        if s_prev * s_next <= 0.0 {
            d[i] = 0.0;
        } else {
            let h_prev = x[i] - x[i - 1];
            let h_next = x[i + 1] - x[i];
            let w1 = 2.0f64.mul_add(h_next, h_prev);
            let w2 = 2.0f64.mul_add(h_prev, h_next);
            d[i] = (w1 + w2) / (w1 / s_prev + w2 / s_next);
        }
    }

    // Monotonicity constraint
    for i in 0..n - 1 {
        if s[i] == 0.0 {
            d[i] = 0.0;
            d[i + 1] = 0.0;
        } else {
            let alpha = d[i] / s[i];
            let beta = d[i + 1] / s[i];
            let tau = alpha.mul_add(alpha, beta * beta);

            if tau > PCHIP_MAX_TAU_SQUARED {
                let scale = 3.0 / tau.sqrt();
                d[i] = scale * alpha * s[i];
                d[i + 1] = scale * beta * s[i];
            }
        }
    }

    // Hermite cubic evaluation
    let h = x[k + 1] - x[k];
    let t = (xi - x[k]) / h;
    let t2 = t * t;
    let t3 = t2 * t;

    let h00 = 2.0f64.mul_add(t3, -3.0 * t2) + 1.0;
    let h10 = 2.0f64.mul_add(-t2, t3) + t;
    let h01 = (-2.0f64).mul_add(t3, 3.0 * t2);
    let h11 = t3 - t2;

    // h00 * y[k] + h10 * h * d[k] + h01 * y[k + 1] + h11 * h * d[k + 1]
    Some(h00.mul_add(
        y[k],
        (h10 * h).mul_add(d[k], (h11 * h).mul_add(d[k + 1], h01 * y[k + 1])),
    ))
}

#[cfg(test)]
mod tests {
    use super::pchip as interpolate_pchip;

    #[test]
    fn pchip() {
        // Test with monotonic data
        // CRF 5 (92.4354), CRF 15 (85.7452), CRF 25 (80.5088), CRF 35 (72.9709)
        let x = [72.9709, 80.5088, 85.7452, 92.4354]; // scores (ascending order)
        let y = [35.0, 25.0, 15.0, 5.0]; // CRFs

        // Test exact points
        assert!(
            (interpolate_pchip(&x, &y, 72.9709).expect("result should exist") - 35.0).abs() < 1e-10
        );
        assert!(
            (interpolate_pchip(&x, &y, 80.5088).expect("result should exist") - 25.0).abs() < 1e-10
        );
        assert!(
            (interpolate_pchip(&x, &y, 85.7452).expect("result should exist") - 15.0).abs() < 1e-10
        );
        assert!(
            (interpolate_pchip(&x, &y, 92.4354).expect("result should exist") - 5.0).abs() < 1e-10
        );

        // Test interpolation for score 89.0
        let result = interpolate_pchip(&x, &y, 89.0);
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
        let result = interpolate_pchip(&x2, &y2, 54.0);
        assert!(result.is_some());
        assert!(
            result.expect("result should exist") > 45.0
                && result.expect("result should exist") < 50.0
        );

        // Test with non-increasing x values (should return None)
        let x_bad = [72.9709, 88.0, 85.7452, 92.4354]; // Not properly ordered
        let y_bad = [35.0, 12.0, 15.0, 5.0];
        assert_eq!(interpolate_pchip(&x_bad, &y_bad, 87.0), None);

        // Test edge case with nearly flat region
        // CRF 63-66 have very similar scores
        let x_flat = [4.944567, 5.270722, 5.345044, 5.575547]; // scores (ascending order)
        let y_flat = [65.0, 66.0, 64.0, 63.0]; // CRFs
        let result = interpolate_pchip(&x_flat, &y_flat, 5.1);
        assert!(result.is_some());
        // Should handle the nearly flat region gracefully

        // Test with mismatched lengths (should return None)
        let x_short = [72.9709, 80.5088];
        let y_long = [35.0, 25.0, 15.0, 5.0];
        assert_eq!(interpolate_pchip(&x_short, &y_long, 76.0), None);

        // Test with too few points (should return None)
        assert_eq!(interpolate_pchip(&[72.9709], &[35.0], 72.9709), None);

        // Test extrapolation (should return None)
        assert_eq!(interpolate_pchip(&x, &y, 65.0), None); // Below range
        assert_eq!(interpolate_pchip(&x, &y, 95.0), None); // Above range
    }

    /// The generalized signature serves the 6th-and-later probes, which hand it
    /// more than four points.
    #[test]
    fn pchip_accepts_more_than_four_points() {
        // CRF 5 (92.4354), CRF 15 (85.7452), CRF 25 (80.5088), CRF 35 (72.9709),
        // CRF 45 (57.916622), CRF 55 (37.303120)
        let x = [37.303120, 57.916622, 72.9709, 80.5088, 85.7452, 92.4354];
        let y = [55.0, 45.0, 35.0, 25.0, 15.0, 5.0];

        // Every knot is reproduced exactly
        for (score, crf) in x.iter().zip(&y) {
            assert!(
                (interpolate_pchip(&x, &y, *score).expect("result should exist") - *crf).abs()
                    < 1e-10
            );
        }

        // Monotone data must not overshoot on any subinterval
        for step in 1..40 {
            let score = (x[5] - x[0]).mul_add(step as f64 / 40.0, x[0]);
            let crf = interpolate_pchip(&x, &y, score).expect("result should exist");
            assert!(
                (5.0..=55.0).contains(&crf),
                "score {score} predicted CRF {crf} outside the observed range"
            );
        }
    }
}
