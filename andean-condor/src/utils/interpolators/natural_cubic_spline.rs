use tracing::trace;

#[inline]
pub fn natural_cubic_spline(x: &[f64], y: &[f64], xi: f64) -> Option<f64> {
    let n = x.len();
    if n < 3 || n != y.len() {
        return None;
    }

    // Noramally, no bounds check is needed - we're interpolating, not
    // extrapolating The target (xi) is a score value we're looking for, not
    // restricted to input range

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

#[cfg(test)]
mod tests {
    use super::natural_cubic_spline as interpolate_natural_cubic_spline;

    #[test]
    fn natural_cubic_spline_reproduces_knots_and_interpolates_between_them() {
        // CRF 10 (84.872162), CRF 20 (78.517479), CRF 30 (72.812233)
        let x = vec![72.812233, 78.517479, 84.872162]; // scores (ascending order)
        let y = vec![30.0, 20.0, 10.0]; // CRFs

        // Test exact points
        assert!(
            (interpolate_natural_cubic_spline(&x, &y, 72.812233).expect("result should exist")
                - 30.0)
                .abs()
                < 1e-10
        );
        assert!(
            (interpolate_natural_cubic_spline(&x, &y, 78.517479).expect("result should exist")
                - 20.0)
                .abs()
                < 1e-10
        );
        assert!(
            (interpolate_natural_cubic_spline(&x, &y, 84.872162).expect("result should exist")
                - 10.0)
                .abs()
                < 1e-10
        );

        // Test interpolation for score 81.0
        let result = interpolate_natural_cubic_spline(&x, &y, 81.0);
        assert!(result.is_some());
        assert!(
            result.expect("result should exist") > 10.0
                && result.expect("result should exist") < 20.0
        );

        // CRF 15 (84.864449), CRF 25 (80.161186), CRF 35 (72.134048)
        let x2 = vec![72.134048, 80.161186, 84.864449]; // scores (ascending order)
        let y2 = vec![35.0, 25.0, 15.0]; // CRFs

        // Test interpolation for score 82.0
        let result = interpolate_natural_cubic_spline(&x2, &y2, 82.0);
        assert!(result.is_some());
        assert!(
            result.expect("result should exist") > 15.0
                && result.expect("result should exist") < 25.0
        );

        // CRF 20 (83.0155), CRF 30 (77.7812), CRF 40 (67.3447)
        let x3 = vec![67.3447, 77.7812, 83.0155]; // scores (ascending order)
        let y3 = vec![40.0, 30.0, 20.0]; // CRFs

        // Test interpolation for score 80.0
        let result = interpolate_natural_cubic_spline(&x3, &y3, 80.0);
        assert!(result.is_some());
        assert!(
            result.expect("result should exist") > 20.0
                && result.expect("result should exist") < 30.0
        );
    }

    #[test]
    fn natural_cubic_spline_rejects_unsorted_short_or_mismatched_inputs() {
        // Test with non-increasing x values (should return None)
        let x_bad = vec![84.872162, 78.517479, 80.0]; // Not properly ordered
        let y_bad = vec![10.0, 20.0, 25.0];
        assert_eq!(interpolate_natural_cubic_spline(&x_bad, &y_bad, 79.0), None);

        // Test with too few points (should return None)
        let x_short = vec![87.0715, 90.0064];
        let y_short = vec![20.0, 10.0];
        assert_eq!(
            interpolate_natural_cubic_spline(&x_short, &y_short, 88.0),
            None
        );

        // Test with mismatched lengths (should return None)
        let x_mismatch = vec![83.8005, 87.0715, 90.0064];
        let y_mismatch = vec![30.0, 20.0];
        assert_eq!(
            interpolate_natural_cubic_spline(&x_mismatch, &y_mismatch, 85.0),
            None
        );
    }
}
