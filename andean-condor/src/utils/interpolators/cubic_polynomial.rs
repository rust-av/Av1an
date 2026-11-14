use tracing::trace;

#[inline]
pub fn cubic_polynomial(x: &[f64; 4], y: &[f64; 4], xi: f64) -> Option<f64> {
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
            x[0], x[3]
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

#[cfg(test)]
mod tests {
    use super::cubic_polynomial as interpolate_cubic_polynomial;

    #[test]
    fn cubic_polynomial_reproduces_knots_and_interpolates_observed_scores() {
        // Test with CRF/score data
        // CRF 5 (92.4354), CRF 15 (85.7452), CRF 25 (80.5088), CRF 35 (72.9709)
        let x = [72.9709, 80.5088, 85.7452, 92.4354]; // scores (ascending order)
        let y = [35.0, 25.0, 15.0, 5.0]; // CRFs

        // Test exact points
        assert!(
            (interpolate_cubic_polynomial(&x, &y, 72.9709).expect("result should exist") - 35.0)
                .abs()
                < 1e-10
        );
        assert!(
            (interpolate_cubic_polynomial(&x, &y, 80.5088).expect("result should exist") - 25.0)
                .abs()
                < 1e-10
        );
        assert!(
            (interpolate_cubic_polynomial(&x, &y, 85.7452).expect("result should exist") - 15.0)
                .abs()
                < 1e-10
        );
        assert!(
            (interpolate_cubic_polynomial(&x, &y, 92.4354).expect("result should exist") - 5.0)
                .abs()
                < 1e-10
        );

        // Test interpolation for score 89.0
        let result = interpolate_cubic_polynomial(&x, &y, 89.0);
        assert!(result.is_some());
        let crf = result.expect("result should exist");
        assert!(crf > 5.0 && crf < 15.0);
        // Should be closer to 10 than to 5 or 15
        assert!((crf - 10.0).abs() < 5.0);

        // Test interpolation for score 76.0
        let result = interpolate_cubic_polynomial(&x, &y, 76.0);
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
            (interpolate_cubic_polynomial(&x2, &y2, 50.740498).expect("result should exist")
                - 50.0)
                .abs()
                < 1e-10
        );

        // Test interpolation for score 54.0
        let result = interpolate_cubic_polynomial(&x2, &y2, 54.0);
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
        let result = interpolate_cubic_polynomial(&x3, &y3, 60.0);
        assert!(result.is_some());
        assert!(
            result.expect("result should exist") > 30.0
                && result.expect("result should exist") < 50.0
        );
    }

    #[test]
    fn cubic_polynomial_rejects_unsorted_knots_and_out_of_range_scores() {
        let x = [72.9709, 80.5088, 85.7452, 92.4354];
        let y = [35.0, 25.0, 15.0, 5.0];
        // Test with non-increasing x values (should return None)
        let x_bad = [72.9709, 88.0, 85.7452, 92.4354]; // Not properly ordered
        let y_bad = [35.0, 12.0, 15.0, 5.0];
        assert_eq!(interpolate_cubic_polynomial(&x_bad, &y_bad, 87.0), None);

        // Test extrapolation (should return None)
        assert_eq!(interpolate_cubic_polynomial(&x, &y, 70.0), None); // Below range
        assert_eq!(interpolate_cubic_polynomial(&x, &y, 95.0), None); // Above range
    }
}
