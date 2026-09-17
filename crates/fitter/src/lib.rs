pub struct Sample {
    pub watts: f64,
    pub rps: Vec<f64>,
}

pub struct Fit {
    pub intercept: f64,
    pub coefs: Vec<f64>,
    pub r2: f64,
    pub samples: usize,
}

pub fn fit(samples: &[Sample], k: usize, ridge: f64) -> Option<Fit> {
    // for "degree of freedom of the redisuals", n - (k+1) where k is the # of routes; +1 because of the intercept
    if samples.len() < k + 2 {
        return None;
    }
    // Linear regressions. Accumulation phase
    // We have to solve : (Xᵀ X) β = Xᵀ y
    // X matrix is n * k+1
    // First column "resolves" to the intercepts (idlkes)
    let n = k + 1;
    // Gram matrix (ata) and atb
    let mut ata = vec![vec![0.0f64; n]; n];
    let mut atb = vec![0.0f64; n];
    for s in samples {
        if s.rps.len() != k {
            return None;
        }
        let mut x = Vec::with_capacity(n);
        x.push(1.0);
        x.extend_from_slice(&s.rps);
        for i in 0..n {
            for j in 0..n {
                ata[i][j] += x[i] * x[j];
            }
            atb[i] += x[i] * s.watts;
        }
    }

    // Getting the ridge term. "Tikhonov regularisation" which is the ridge.
    // We now have : (XᵀX + ρI) β = Xᵀy instead of (Xᵀ X) β = Xᵀ y
    // skip(1) is a common practice apparently
    // TODO : Check with a math person
    for (i, row) in ata.iter_mut().enumerate().skip(1) {
        row[i] += ridge;
    }

    // Solving
    let beta = solve(ata, atb)?;

    let mean = samples.iter().map(|s| s.watts).sum::<f64>() / samples.len() as f64;
    let mut ss_res = 0.0;
    let mut ss_tot = 0.0;
    for s in samples {
        let mut p = beta[0];
        for (j, r) in s.rps.iter().enumerate() {
            p += beta[j + 1] * r;
        }
        ss_res += (s.watts - p).powi(2);
        ss_tot += (s.watts - mean).powi(2);
    }

    // Coeff of determination
    // Currently only represent how solved model represent the data, not prediction
    // TODO: What happens when we add route as we accumulate
    let r2 = if ss_tot <= f64::EPSILON {
        0.0
    } else {
        1.0 - ss_res / ss_tot
    };

    Some(Fit {
        intercept: beta[0],
        coefs: beta[1..].to_vec(),
        r2,
        samples: samples.len(),
    })
}

// Gaussian elimination with partial pivoting
// TODO : Check for QR factorisation of X (Householder reflections), or SVD
//      : apparently we should not implement it this way for forming Xᵀ X due to LEast Squares
//      : but it seems fine for small number of routes though.
// TODO : check solving methods and bench against number of routes (precision vs computation time)

// TODO : Check if we can use the solved artifact to get confidence intervals
fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    let mut min_pivot = f64::MAX;
    let mut max_pivot: f64 = 0.0;
    for c in 0..n {
        let mut piv = c;
        for r in (c + 1)..n {
            if a[r][c].abs() > a[piv][c].abs() {
                piv = r;
            }
        }
        let mag = a[piv][c].abs();
        // "singularity check"
        if mag < 1e-12 {
            return None;
        }
        min_pivot = min_pivot.min(mag);
        max_pivot = max_pivot.max(mag);
        a.swap(c, piv);
        b.swap(c, piv);
        // one clone per column so the pivot row and the target row are not
        // borrowed at once
        // TODO: Check if clone necessary ?
        let pivot_row = a[c].clone();
        let pivot_b = b[c];
        for r in (c + 1)..n {
            let f = a[r][c] / pivot_row[c];
            if f == 0.0 {
                continue;
            }
            for (k, v) in a[r].iter_mut().enumerate().skip(c) {
                *v -= f * pivot_row[k];
            }
            b[r] -= f * pivot_b;
        }
    }
    // "near-singularity" check
    if min_pivot / max_pivot < 1e-8 {
        return None;
    }
    let mut x = vec![0.0; n];
    for r in (0..n).rev() {
        let mut s = b[r];
        for c in (r + 1)..n {
            s -= a[r][c] * x[c];
        }
        x[r] = s / a[r][r];
    }
    Some(x)
}
