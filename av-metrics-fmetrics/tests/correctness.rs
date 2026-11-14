//! Verifies the crate loads the fmetrics built from the fork and scores through
//! it. Each test skips itself when the library is absent, so this is harmless
//! on a machine without it. CI sets `AV_METRICS_REQUIRE_CPU` to forbid that skip.

use std::sync::Mutex;

use av_metrics_fmetrics::{
    FmetricsApi, FmetricsButteraugliOptions, FmetricsColorspace, FmetricsCvvdpDisplayModel,
    FmetricsCvvdpResult, FmetricsErr, FmetricsImg, FmetricsMetric, FmetricsWorkspace,
    cvvdp_version, fmetrics_version, is_available,
};

/// Serialises every call into the library.
///
/// These tests exercise the raw FFI rather than [`FmetricsScorer`], which
/// manages its own workspaces. Here each test creates one workspace for the
/// duration of its own call, so the lock is not needed for correctness — it
/// only keeps the output readable by stopping four tests from interleaving.
/// What the raw layer cannot do is share *one* workspace between threads, which
/// is why the scorer hands out a distinct workspace per caller instead.
static CALL: Mutex<()> = Mutex::new(());

/// Take the library lock, or report that the tests must run serially.
macro_rules! library {
    () => {
        match CALL.lock() {
            Ok(guard) => guard,
            // A poisoned lock means another test already faulted inside the
            // library; the panic it carries is the real diagnosis.
            Err(poisoned) => poisoned.into_inner(),
        }
    };
}

/// Build an interleaved RGB pair from a deterministic pattern.
fn images() -> (Vec<u8>, Vec<u8>, FmetricsImg, FmetricsImg) {
    const WIDTH: u32 = 64;
    const HEIGHT: u32 = 64;

    let mut reference = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
    let mut distorted = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);

    for y in 0..HEIGHT as usize {
        for x in 0..WIDTH as usize {
            reference.push(((x * 4 + y * 3) % 256) as u8);
            reference.push(((y * 5 + 10) % 256) as u8);
            reference.push(((x * 2 + y * 2) % 256) as u8);

            distorted.push(((x * 4 + y * 3 + 6) % 256) as u8);
            distorted.push(((y * 5 + 16) % 256) as u8);
            distorted.push(((x * 2 + y * 2 + 6) % 256) as u8);
        }
    }

    let reference_img = FmetricsImg::rgb8(
        reference.as_ptr().cast(),
        WIDTH,
        HEIGHT,
        WIDTH * 3,
        FmetricsColorspace::Srgb,
        false,
    );
    let distorted_img = FmetricsImg::rgb8(
        distorted.as_ptr().cast(),
        WIDTH,
        HEIGHT,
        WIDTH * 3,
        FmetricsColorspace::Srgb,
        false,
    );

    (reference, distorted, reference_img, distorted_img)
}

/// A pair of images of an explicit size, for the size-dependent metrics.
fn small_images(size: u32) -> (Vec<u8>, Vec<u8>, FmetricsImg, FmetricsImg) {
    let mut reference = Vec::with_capacity((size * size * 3) as usize);
    let mut distorted = Vec::with_capacity((size * size * 3) as usize);

    for y in 0..size as usize {
        for x in 0..size as usize {
            reference.push(((x + y) % 256) as u8);
            reference.push(128);
            reference.push(((x * 2 + y) % 256) as u8);

            distorted.push(((x + y + 4) % 256) as u8);
            distorted.push(128);
            distorted.push(((x * 2 + y + 4) % 256) as u8);
        }
    }

    let reference_img = FmetricsImg::rgb8(
        reference.as_ptr().cast(),
        size,
        size,
        size * 3,
        FmetricsColorspace::Srgb,
        false,
    );
    let distorted_img = FmetricsImg::rgb8(
        distorted.as_ptr().cast(),
        size,
        size,
        size * 3,
        FmetricsColorspace::Srgb,
        false,
    );

    (reference, distorted, reference_img, distorted_img)
}

#[test]
fn loads_and_reports_its_version() {
    if std::env::var_os("AV_METRICS_REQUIRE_CPU").is_some() {
        assert!(
            is_available(),
            "required CPU-native checks need libfmetrics"
        );
    }
    if !is_available() {
        eprintln!("skipping: fmetrics is not available");
        return;
    }

    let version = fmetrics_version().expect("available means loadable");
    assert!(!version.is_empty(), "version must not be empty");

    // The CVVDP backend is a separate component with its own version.
    let cvvdp = cvvdp_version().expect("available means loadable");
    assert!(!cvvdp.is_empty(), "cvvdp version must not be empty");

    let _guard = library!();
    let api = FmetricsApi::load().expect("available means loadable");
    assert!(!api.version_string().is_empty());
}

#[test]
fn ssimulacra2_scores_identically_across_runs() {
    if !is_available() {
        eprintln!("skipping: fmetrics is not available");
        return;
    }

    let _guard = library!();
    let api = FmetricsApi::load().expect("available means loadable");
    let (reference, distorted, reference_img, distorted_img) = images();

    // SAFETY: the workspace is released below, and both images outlive the
    // calls because their buffers stay in scope.
    let workspace = unsafe { (api.workspace_create)() };
    assert!(
        !workspace.is_null(),
        "workspace_create must not return null"
    );

    let mut scores = Vec::new();
    for _ in 0..3 {
        let mut value = 0f64;
        // SAFETY: the pointers match the signatures transcribed from
        // `fmetrics.h`, and the workspace is live.
        let status =
            unsafe { (api.ssimu2_cmp)(workspace, &reference_img, &distorted_img, &mut value) };
        assert_eq!(status, FmetricsErr::Ok, "ssimu2 must succeed");
        scores.push(value);
    }

    // SAFETY: the workspace came from `workspace_create` above.
    unsafe { (api.workspace_destroy)(workspace) };

    // Determinism is an acceptance criterion: the same fixture must give the
    // same number every time, or a score cannot be trusted.
    assert_eq!(scores[0], scores[1], "SSIMULACRA2 must be deterministic");
    assert_eq!(scores[1], scores[2], "SSIMULACRA2 must be deterministic");
    assert!(
        scores[0].is_finite(),
        "score must be finite, got {}",
        scores[0]
    );

    // Keep the buffers observably alive.
    assert_eq!(reference.len(), distorted.len());
}

#[test]
fn butteraugli_rejects_a_null_workspace_gracefully() {
    if !is_available() {
        eprintln!("skipping: fmetrics is not available");
        return;
    }

    let _guard = library!();
    let api = FmetricsApi::load().expect("available means loadable");
    // Bound for the life of the test. The descriptors are pointers into these
    // buffers, so they must outlive the call below even though a null workspace
    // is expected to be rejected before either is read.
    let (reference, distorted, reference_img, distorted_img) = images();
    let _keep = (&reference, &distorted);

    let options = FmetricsButteraugliOptions::default();
    let mut value = 0f64;
    // SAFETY: a null workspace is a deliberately invalid argument; the call
    // must report failure rather than dereference it.
    let status = unsafe {
        (api.butteraugli_cmp)(
            FmetricsWorkspace(std::ptr::null_mut()),
            &reference_img,
            &distorted_img,
            &options,
            &mut value,
        )
    };

    assert!(
        !status.is_ok(),
        "a null workspace must be reported as an error, not succeed"
    );
}

#[test]
fn native_ssimulacra2_returns_a_finite_score() {
    if !is_available() {
        eprintln!("skipping: fmetrics is not available");
        return;
    }

    let _guard = library!();
    let api = FmetricsApi::load().expect("available means loadable");
    // Descriptors borrow these buffers through the native call.
    let (reference, distorted, reference_img, distorted_img) = images();
    let _keep = (&reference, &distorted);
    let mut ssimu2 = 0f64;
    let status = {
        // SAFETY: `workspace_create` takes no arguments and returns an opaque
        // handle, or null.
        let workspace = unsafe { (api.workspace_create)() };
        assert!(!workspace.is_null());
        // SAFETY: the workspace is live and both images outlive the call.
        let status =
            unsafe { (api.ssimu2_cmp)(workspace, &reference_img, &distorted_img, &mut ssimu2) };
        // SAFETY: the workspace came from `workspace_create`.
        unsafe { (api.workspace_destroy)(workspace) };
        status
    };
    assert_eq!(status, FmetricsErr::Ok, "ssimu2 must succeed");
    assert!(ssimu2.is_finite(), "ssimu2 must be finite, got {ssimu2}");
}

#[test]
fn native_butteraugli_returns_a_non_nan_score() {
    if !is_available() {
        eprintln!("skipping: fmetrics is not available");
        return;
    }

    let _guard = library!();
    let api = FmetricsApi::load().expect("available means loadable");
    let (reference, distorted, reference_img, distorted_img) = images();
    let _keep = (&reference, &distorted);
    let options = FmetricsButteraugliOptions::default();

    let mut butteraugli = 0f64;
    let status = {
        // SAFETY: `workspace_create` takes no arguments and returns an opaque
        // handle, or null.
        let workspace = unsafe { (api.workspace_create)() };
        // SAFETY: as above; `options` outlives the call.
        let status = unsafe {
            (api.butteraugli_cmp)(
                workspace,
                &reference_img,
                &distorted_img,
                &options,
                &mut butteraugli,
            )
        };
        // SAFETY: the workspace came from `workspace_create`.
        unsafe { (api.workspace_destroy)(workspace) };
        status
    };
    assert_eq!(status, FmetricsErr::Ok, "butteraugli must succeed");
    // Butteraugli is a distance, and a large one saturates to infinity rather
    // than overflowing to a misleading finite value. Both are valid outcomes,
    // so only NaN is rejected.
    assert!(
        !butteraugli.is_nan(),
        "butteraugli must not be NaN, got {butteraugli}"
    );
}

#[test]
fn native_iwssim_rejects_image_one_pixel_below_declared_minimum() {
    if !is_available() {
        eprintln!("skipping: fmetrics is not available");
        return;
    }

    let _guard = library!();
    let api = FmetricsApi::load().expect("available means loadable");

    // IW-SSIM refuses an image below its five-scale floor, and names the cause
    // rather than returning a meaningless number. MS-SSIM has a lower floor, so
    // only IW-SSIM is asserted here; the point is that the library rejects
    // rather than scores, and `minimum_dimension` lets a caller avoid
    // reaching it.
    let minimum = FmetricsMetric::Iwssim.minimum_dimension().expect("declared") as u32;
    let too_small = minimum - 1;
    let (reference, distorted, reference_img, distorted_img) = small_images(too_small);
    // Bound so the buffers outlive the descriptors, which point into them.
    let _keep = (&reference, &distorted);

    let mut value = 0f64;
    // SAFETY: the workspace is created and released here, and both images
    // outlive the call.
    let workspace = unsafe { (api.workspace_create)() };
    // SAFETY: as above.
    let status = unsafe { (api.iwssim_cmp)(workspace, &reference_img, &distorted_img, &mut value) };
    // SAFETY: the workspace came from `workspace_create`.
    unsafe { (api.workspace_destroy)(workspace) };

    assert_eq!(
        status,
        FmetricsErr::IwssimImgTooSmall,
        "iwssim must name the size as the cause at {too_small}px"
    );
}

#[test]
fn native_iwssim_scores_a_two_hundred_fifty_six_square_image() {
    score_multiscale_metric("iwssim", true);
}

#[test]
fn native_msssim_scores_a_two_hundred_fifty_six_square_image() {
    score_multiscale_metric("msssim", false);
}

fn score_multiscale_metric(name: &str, is_iwssim: bool) {
    if !is_available() {
        eprintln!("skipping: fmetrics is not available");
        return;
    }

    let _guard = library!();
    let api = FmetricsApi::load().expect("available means loadable");

    // 256x256 is comfortably past any five-scale minimum, and is the size the
    // CLI itself needs for IW-SSIM.
    let size = 256;
    let (reference, distorted, reference_img, distorted_img) = small_images(size);
    // Bound so the buffers outlive the descriptors, which point into them.
    let _keep = (&reference, &distorted);

    let mut value = 0f64;
    // SAFETY: the workspace is created and released here, and both images
    // outlive the call.
    let workspace = unsafe { (api.workspace_create)() };
    // SAFETY: as above.
    let status = unsafe {
        if is_iwssim {
            (api.iwssim_cmp)(workspace, &reference_img, &distorted_img, &mut value)
        } else {
            (api.msssim_cmp)(workspace, &reference_img, &distorted_img, &mut value)
        }
    };
    // SAFETY: the workspace came from `workspace_create`.
    unsafe { (api.workspace_destroy)(workspace) };

    assert_eq!(
        status,
        FmetricsErr::Ok,
        "{name} must accept a {size}x{size} image"
    );
    assert!(value.is_finite(), "{name} must be finite, got {value}");
}

#[test]
fn native_cvvdp_one_shot_returns_distinct_finite_jod_and_quality() {
    if !is_available() {
        eprintln!("skipping: fmetrics is not available");
        return;
    }

    let _guard = library!();
    let api = FmetricsApi::load().expect("available means loadable");
    let (reference, distorted, reference_img, distorted_img) = images();
    // Bound so the buffers outlive the descriptors, which point into them.
    let _keep = (&reference, &distorted);

    // The one-shot form avoids the context lifetime, so this tests the result
    // shape rather than the accumulation.
    let mut result = FmetricsCvvdpResult::default();
    // SAFETY: both images outlive the call; a null `custom_params` selects the
    // display model's own parameters.
    let status = unsafe {
        (api.cvvdp_cmp)(
            &reference_img,
            &distorted_img,
            FmetricsCvvdpDisplayModel::StandardFhd,
            1,
            std::ptr::null(),
            &mut result,
        )
    };

    assert_eq!(status, FmetricsErr::Ok, "cvvdp must succeed: {status:?}");
    assert!(
        result.jod.is_finite(),
        "jod must be finite, got {}",
        result.jod
    );
    assert!(
        result.quality.is_finite(),
        "quality must be finite, got {}",
        result.quality
    );
    // The two are on different scales, so they should not be the same number.
    assert_ne!(
        result.jod, result.quality,
        "jod and quality are distinct measures"
    );
}
