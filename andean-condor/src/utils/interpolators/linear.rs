#[inline]
pub fn linear(x: &[f64; 2], y: &[f64; 2], xi: f64) -> Option<f64> {
    // Check strictly increasing
    if x[1] <= x[0] {
        return None;
    }

    // Linear interpolation formula: y = y0 + (y1 - y0) * (xi - x0) / (x1 - x0)
    let t = (xi - x[0]) / (x[1] - x[0]);
    Some(t.mul_add(y[1] - y[0], y[0]))
}

#[cfg(test)]
mod tests {
    use super::linear as interpolate_linear;

    #[test]
    fn linear_interpolation_reproduces_endpoints_and_intermediate_crfs() {
        // Test basic linear interpolation using real CRF/score data
        let x = [82.502861, 87.600777]; // scores (ascending order)
        let y = [20.0, 10.0]; // CRFs

        // Test exact points
        assert_eq!(interpolate_linear(&x, &y, 82.502861), Some(20.0));
        assert_eq!(interpolate_linear(&x, &y, 87.600777), Some(10.0));

        // Test midpoint - score 85.051819 should give CRF ~15
        assert!(
            (interpolate_linear(&x, &y, 85.051819).expect("result should exist") - 15.0).abs()
                < 0.1
        );

        // Test interpolation for score 84.0
        let result = interpolate_linear(&x, &y, 84.0);
        assert!(result.is_some());
        assert!(
            result.expect("result should exist") > 15.0
                && result.expect("result should exist") < 20.0
        );

        let x2 = [78.737953, 89.179634]; // scores (ascending order)
        let y2 = [15.0, 5.0]; // CRFs
        assert!(
            (interpolate_linear(&x2, &y2, 83.958794).expect("result should exist") - 10.0).abs()
                < 0.1
        );
    }

    #[test]
    fn linear_interpolation_rejects_unsorted_or_equal_score_knots() {
        let y = [20.0, 10.0];
        // Test non-increasing x values (should return None)
        let x_bad = [87.600777, 82.502861]; // Not ascending
        let y_bad = [10.0, 20.0];
        assert_eq!(interpolate_linear(&x_bad, &y_bad, 85.0), None);

        // Test equal x values (should return None)
        let x_equal = [85.0, 85.0];
        assert_eq!(interpolate_linear(&x_equal, &y, 85.0), None);
    }
}
