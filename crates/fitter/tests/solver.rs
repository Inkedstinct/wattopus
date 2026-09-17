use fitter::{fit, Sample};

/// watts = 2.0 + 3.0*a + 0.5*b, with a and b varying independently
fn synthetic() -> Vec<Sample> {
    let mut out = Vec::new();
    for i in 0..40 {
        let a = (i % 7) as f64 * 0.3;
        let b = (i % 5) as f64 * 0.7;
        out.push(Sample {
            watts: 2.0 + 3.0 * a + 0.5 * b,
            rps: vec![a, b],
        });
    }
    out
}

#[test]
fn recovers_known_coefficients() {
    let f = fit(&synthetic(), 2, 1e-9).expect("independent regressors must fit");
    assert!(
        (f.intercept - 2.0).abs() < 1e-4,
        "intercept {}",
        f.intercept
    );
    assert!((f.coefs[0] - 3.0).abs() < 1e-4, "beta_a {}", f.coefs[0]);
    assert!((f.coefs[1] - 0.5).abs() < 1e-4, "beta_b {}", f.coefs[1]);
    assert!(f.r2 > 0.999, "r2 {}", f.r2);
    assert_eq!(f.samples, 40);
}

/// the 1:1:1 demo loop: rates move together, coefficients are not separable.
/// rejecting is the whole point - a confident wrong split is worse than none
#[test]
fn rejects_collinear_design() {
    let samples: Vec<Sample> = (0..40)
        .map(|i| {
            let a = i as f64 * 0.1;
            Sample {
                watts: 1.0 + a,
                rps: vec![a, 2.0 * a],
            }
        })
        .collect();
    assert!(fit(&samples, 2, 1e-9).is_none());
}

#[test]
fn rejects_too_few_samples() {
    let samples = vec![Sample {
        watts: 1.0,
        rps: vec![0.5, 0.5],
    }];
    assert!(fit(&samples, 2, 1e-9).is_none());
}

#[test]
fn reports_r2_below_one_on_noisy_data() {
    let mut samples = synthetic();
    samples[0].watts += 5.0;
    let f = fit(&samples, 2, 1e-9).expect("still fittable");
    assert!(f.r2 < 0.999, "noise must lower r2, got {}", f.r2);
    assert!(
        f.r2 > 0.5,
        "one outlier must not destroy the fit, got {}",
        f.r2
    );
}

#[test]
fn rejects_ragged_samples() {
    let samples = vec![
        Sample {
            watts: 1.0,
            rps: vec![0.1, 0.2],
        },
        Sample {
            watts: 2.0,
            rps: vec![0.3],
        },
    ];
    assert!(fit(&samples, 2, 1e-9).is_none());
}
