use tracing::trace;

#[inline]
pub fn quadratic(x: &[f64; 3], y: &[f64; 3], xi: f64) -> Option<f64> {
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
            x[0], x[2]
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

#[cfg(test)]
mod tests {
    use super::quadratic as interpolate_quadratic;

    #[test]
    fn quadratic_interpolation_reproduces_knots_and_intermediate_crfs() {
        // Test with CRF/score data
        // CRF 10 (84.872162), CRF 20 (78.517479), CRF 30 (72.812233)
        let x = [72.812233, 78.517479, 84.872162]; // scores (ascending order)
        let y = [30.0, 20.0, 10.0]; // CRFs

        // Test exact points
        assert!(
            (interpolate_quadratic(&x, &y, 72.812233).expect("result should exist") - 30.0).abs()
                < 1e-10
        );
        assert!(
            (interpolate_quadratic(&x, &y, 78.517479).expect("result should exist") - 20.0).abs()
                < 1e-10
        );
        assert!(
            (interpolate_quadratic(&x, &y, 84.872162).expect("result should exist") - 10.0).abs()
                < 1e-10
        );

        // Test interpolation for score 75.0
        let result = interpolate_quadratic(&x, &y, 75.0);
        assert!(result.is_some());
        let crf = result.expect("result should exist");
        assert!(crf > 20.0 && crf < 30.0);
        // Should be closer to 25 than to 20 or 30
        assert!((crf - 25.0).abs() < 5.0);

        // Test interpolation for score 81.0
        let result = interpolate_quadratic(&x, &y, 81.0);
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
            (interpolate_quadratic(&x2, &y2, 80.161186).expect("result should exist") - 25.0).abs()
                < 1e-10
        );

        // Test interpolation for score 76.0
        let result = interpolate_quadratic(&x2, &y2, 76.0);
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
        let result = interpolate_quadratic(&x3, &y3, 80.0);
        assert!(result.is_some());
        let crf = result.expect("result should exist");
        assert!(crf > 20.0 && crf < 30.0);
    }

    #[test]
    fn quadratic_interpolation_rejects_unsorted_or_out_of_range_scores() {
        let x = [72.812233, 78.517479, 84.872162];
        let y = [30.0, 20.0, 10.0];
        // Test with non-increasing x values (should return None)
        let x_bad = [84.872162, 78.517479, 80.0]; // Not properly ordered
        let y_bad = [10.0, 20.0, 25.0];
        assert_eq!(interpolate_quadratic(&x_bad, &y_bad, 79.0), None);

        // Test extrapolation (should return None)
        assert_eq!(interpolate_quadratic(&x, &y, 65.0), None); // Below range
        assert_eq!(interpolate_quadratic(&x, &y, 90.0), None); // Above range
    }
}
