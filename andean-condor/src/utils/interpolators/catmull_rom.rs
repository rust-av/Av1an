use tracing::trace;

#[inline]
pub fn catmull_rom(x: &[f64; 4], y: &[f64; 4], xi: f64) -> Option<f64> {
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
            x[1], x[2]
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

#[cfg(test)]
mod tests {
    use super::catmull_rom as interpolate_catmull_rom;

    #[test]
    fn catmull_rom_interpolates_between_the_middle_knots() {
        // Test with CRF/score data
        // CRF 5 (92.4354), CRF 15 (85.7452), CRF 25 (80.5088), CRF 35 (72.9709)
        let x = [72.9709, 80.5088, 85.7452, 92.4354]; // scores (ascending order)
        let y = [35.0, 25.0, 15.0, 5.0]; // CRFs

        // Test exact points (at x[1] and x[2])
        assert!(
            (interpolate_catmull_rom(&x, &y, 80.5088).expect("result should exist") - 25.0).abs()
                < 1e-10
        );
        assert!(
            (interpolate_catmull_rom(&x, &y, 85.7452).expect("result should exist") - 15.0).abs()
                < 1e-10
        );

        // Test interpolation between x[1] and x[2]
        let result = interpolate_catmull_rom(&x, &y, 83.0);
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
            (interpolate_catmull_rom(&x2, &y2, 50.740498).expect("result should exist") - 50.0)
                .abs()
                < 1e-10
        );
        assert!(
            (interpolate_catmull_rom(&x2, &y2, 57.916622).expect("result should exist") - 45.0)
                .abs()
                < 1e-10
        );

        // Test interpolation for score 54.0
        let result = interpolate_catmull_rom(&x2, &y2, 54.0);
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
        let result = interpolate_catmull_rom(&x3, &y3, 60.0);
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
        let result = interpolate_catmull_rom(&x4, &y4, 80.0);
        assert!(result.is_some());
        let crf = result.expect("result should exist");
        assert!(crf > 20.0 && crf < 22.0);
    }

    #[test]
    fn catmull_rom_rejects_unsorted_knots_and_scores_outside_the_middle_interval() {
        let x = [72.9709, 80.5088, 85.7452, 92.4354];
        let y = [35.0, 25.0, 15.0, 5.0];
        // Test with non-increasing x values (should return None)
        let x_bad = [72.9709, 88.0, 85.7452, 92.4354]; // Not properly ordered
        let y_bad = [35.0, 12.0, 15.0, 5.0];
        assert_eq!(interpolate_catmull_rom(&x_bad, &y_bad, 87.0), None);

        // Test outside interpolation range (should return None)
        // Note: Catmull-Rom only interpolates between x[1] and x[2]
        assert_eq!(interpolate_catmull_rom(&x, &y, 75.0), None); // Before x[1]
        assert_eq!(interpolate_catmull_rom(&x, &y, 90.0), None); // After x[2]
    }
}
