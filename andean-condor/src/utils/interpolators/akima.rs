use tracing::trace;

#[inline]
pub fn akima(x: &[f64; 4], y: &[f64; 4], xi: f64) -> Option<f64> {
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
            x[0], x[3]
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
        t[1] = f64::midpoint(m[0], m[1]);
    } else {
        // For 4 points, we approximate the weights
        let w1 = (m[1] - m[0]).abs();
        let w2 = (m[1] - m[0]).abs(); // Same weight for symmetry
        t[1] = w2.mul_add(m[0], w1 * m[1]) / (w1 + w2);
    }

    // For point 2: use differences m[1] and m[2]
    if (m[2] - m[1]).abs() < 1e-10 {
        t[2] = f64::midpoint(m[1], m[2]);
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
    use super::akima as interpolate_akima;
    #[test]
    fn akima_reproduces_knots_and_interpolates_between_observed_scores() {
        // Test with CRF/score data
        // CRF 5 (92.4354), CRF 15 (85.7452), CRF 25 (80.5088), CRF 35 (72.9709)
        let x = [72.9709, 80.5088, 85.7452, 92.4354]; // scores (ascending order)
        let y = [35.0, 25.0, 15.0, 5.0]; // CRFs

        // Test exact points
        assert!(
            (interpolate_akima(&x, &y, 72.9709).expect("result should exist") - 35.0).abs() < 1e-10
        );
        assert!(
            (interpolate_akima(&x, &y, 80.5088).expect("result should exist") - 25.0).abs() < 1e-10
        );
        assert!(
            (interpolate_akima(&x, &y, 85.7452).expect("result should exist") - 15.0).abs() < 1e-10
        );
        assert!(
            (interpolate_akima(&x, &y, 92.4354).expect("result should exist") - 5.0).abs() < 1e-10
        );

        // Test interpolation for score 89.0
        let result = interpolate_akima(&x, &y, 89.0);
        assert!(result.is_some());
        let crf = result.expect("result should exist");
        assert!(crf > 5.0 && crf < 15.0);

        // Test interpolation for score 76.0
        let result = interpolate_akima(&x, &y, 76.0);
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
        let result = interpolate_akima(&x2, &y2, 54.0);
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
        let result = interpolate_akima(&x3, &y3, 82.5);
        assert!(result.is_some());
    }

    #[test]
    fn akima_rejects_unsorted_knots_and_out_of_range_scores() {
        let x = [72.9709, 80.5088, 85.7452, 92.4354];
        let y = [35.0, 25.0, 15.0, 5.0];
        // Test with non-increasing x values (should return None)
        let x_bad = [72.9709, 88.0, 85.7452, 92.4354]; // Not properly ordered
        let y_bad = [35.0, 12.0, 15.0, 5.0];
        assert_eq!(interpolate_akima(&x_bad, &y_bad, 87.0), None);

        // Test extrapolation (should return None)
        assert_eq!(interpolate_akima(&x, &y, 70.0), None); // Below range
        assert_eq!(interpolate_akima(&x, &y, 95.0), None); // Above range
    }
}
