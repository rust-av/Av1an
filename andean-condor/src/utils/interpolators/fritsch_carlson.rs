use tracing::trace;

/// Fritsch–Carlson monotone cubic interpolation over exactly three points.
///
/// Unlike [`super::natural_cubic_spline`], the interpolant is guaranteed not to
/// overshoot between samples: the interior derivative is a weighted harmonic
/// mean of the two neighbouring secant slopes, and is forced to zero wherever
/// those slopes disagree in sign. That keeps the predicted quantizer inside the
/// range the probes actually observed, which matters when the target sits very
/// close to a probe's score.
#[inline]
pub fn fritsch_carlson(x: &[f64; 3], y: &[f64; 3], xi: f64) -> Option<f64> {
    // Check strictly increasing
    for i in 0..2 {
        if x[i + 1] <= x[i] {
            return None;
        }
    }

    // Verify xi is within the observed range
    if xi < x[0] || xi > x[2] {
        trace!(
            "Fritsch-Carlson interpolation: unexpected extrapolation case - xi = {xi}, range = \
             [{}, {}]",
            x[0], x[2]
        );
        return None;
    }

    // Secant slopes of the two intervals
    let d0 = (y[1] - y[0]) / (x[1] - x[0]);
    let d1 = (y[2] - y[1]) / (x[2] - x[1]);

    // Derivatives at the three knots. The endpoints take their secant slope;
    // the interior knot is the weighted harmonic mean of the two secants, or
    // zero where they disagree in sign (a local extremum).
    let mut m = [0.0; 3];
    m[0] = d0;
    m[2] = d1;

    if d0 * d1 <= 0.0 {
        m[1] = 0.0;
    } else {
        let h0 = x[1] - x[0];
        let h1 = x[2] - x[1];
        let w1 = 2.0f64.mul_add(h1, h0);
        let w2 = 2.0f64.mul_add(h0, h1);
        m[1] = (w1 + w2) / (w1 / d0 + w2 / d1);
    }

    // Select the interval containing xi
    let k = usize::from(xi >= x[1]);

    // Hermite cubic evaluation on [x[k], x[k + 1]]
    let h = x[k + 1] - x[k];
    let t = (xi - x[k]) / h;
    let t2 = t * t;
    let t3 = t2 * t;

    let h00 = 2.0f64.mul_add(t3, 3.0f64.mul_add(-t2, 1.0));
    let h10 = 2.0f64.mul_add(-t2, t3.mul_add(1.0, t));
    let h01 = (-2.0f64).mul_add(t3, 3.0 * t2);
    let h11 = t3 - t2;

    // h00 * y[k] + h10 * h * m[k] + h01 * y[k + 1] + h11 * h * m[k + 1]
    Some((h11 * h).mul_add(
        m[k + 1],
        h00.mul_add(y[k], h10.mul_add(h * m[k], h01 * y[k + 1])),
    ))
}

#[cfg(test)]
mod tests {
    use super::fritsch_carlson as interpolate_fritsch_carlson;

    #[test]
    fn fritsch_carlson() {
        // Test with CRF/score data
        // CRF 10 (84.872162), CRF 20 (78.517479), CRF 30 (72.812233)
        let x = [72.812233, 78.517479, 84.872162]; // scores (ascending order)
        let y = [30.0, 20.0, 10.0]; // CRFs

        // Test exact points
        assert!(
            (interpolate_fritsch_carlson(&x, &y, 72.812233).expect("result should exist") - 30.0)
                .abs()
                < 1e-10
        );
        assert!(
            (interpolate_fritsch_carlson(&x, &y, 78.517479).expect("result should exist") - 20.0)
                .abs()
                < 1e-10
        );
        assert!(
            (interpolate_fritsch_carlson(&x, &y, 84.872162).expect("result should exist") - 10.0)
                .abs()
                < 1e-10
        );

        // Test interpolation for score 81.0
        let result = interpolate_fritsch_carlson(&x, &y, 81.0);
        assert!(result.is_some());
        assert!(
            result.expect("result should exist") > 10.0
                && result.expect("result should exist") < 20.0
        );

        // Test with another set of CRF data
        // CRF 15 (84.864449), CRF 25 (80.161186), CRF 35 (72.134048)
        let x2 = [72.134048, 80.161186, 84.864449]; // scores (ascending order)
        let y2 = [35.0, 25.0, 15.0]; // CRFs

        // Test interpolation for score 82.0
        let result = interpolate_fritsch_carlson(&x2, &y2, 82.0);
        assert!(result.is_some());
        assert!(
            result.expect("result should exist") > 15.0
                && result.expect("result should exist") < 25.0
        );

        // Test with data that has varying slopes
        // CRF 20 (83.0155), CRF 30 (77.7812), CRF 40 (67.3447)
        let x3 = [67.3447, 77.7812, 83.0155]; // scores (ascending order)
        let y3 = [40.0, 30.0, 20.0]; // CRFs

        // Test interpolation for score 80.0
        let result = interpolate_fritsch_carlson(&x3, &y3, 80.0);
        assert!(result.is_some());
        assert!(
            result.expect("result should exist") > 20.0
                && result.expect("result should exist") < 30.0
        );

        // Monotone data must not overshoot: the interpolant stays between the
        // neighbouring samples on every subinterval.
        for step in 1..20 {
            let score = (x[2] - x[0]).mul_add(step as f64 / 20.0, x[0]);
            let crf = interpolate_fritsch_carlson(&x, &y, score).expect("result should exist");
            assert!(
                (10.0..=30.0).contains(&crf),
                "score {score} predicted CRF {crf} outside the observed range"
            );
        }

        // A local extremum in the middle forces a zero derivative there, so the
        // curve flattens instead of overshooting.
        let x_extremum = [70.0, 75.0, 80.0];
        let y_extremum = [30.0, 20.0, 25.0];
        let result = interpolate_fritsch_carlson(&x_extremum, &y_extremum, 75.0);
        assert!(result.is_some());
        assert!(
            (result.expect("result should exist") - 20.0).abs() < 1e-10,
            "the knot value is reproduced exactly"
        );

        // Test with non-increasing x values (should return None)
        let x_bad = [84.872162, 78.517479, 80.0]; // Not properly ordered
        let y_bad = [10.0, 20.0, 25.0];
        assert_eq!(interpolate_fritsch_carlson(&x_bad, &y_bad, 79.0), None);

        // Test extrapolation (should return None)
        assert_eq!(interpolate_fritsch_carlson(&x, &y, 65.0), None); // Below range
        assert_eq!(interpolate_fritsch_carlson(&x, &y, 90.0), None); // Above range
    }
}
