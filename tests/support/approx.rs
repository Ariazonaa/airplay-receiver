//! `doctest::Approx` aus v1: |a − b| < ε · (1 + max(|a|, |b|)), Standard-ε = 100 · f32::EPSILON.

#![allow(dead_code)]

pub const DEFAULT_EPS: f64 = f32::EPSILON as f64 * 100.0;

pub fn approx_eps(actual: f64, expected: f64, eps: f64) -> bool {
    (actual - expected).abs() < eps * (1.0 + actual.abs().max(expected.abs()))
}

pub fn approx(actual: f64, expected: f64) -> bool {
    approx_eps(actual, expected, DEFAULT_EPS)
}

macro_rules! assert_approx {
    ($actual:expr, $expected:expr) => {{
        let (a, e) = (f64::from($actual), f64::from($expected));
        assert!(approx::approx(a, e), "{} ≉ {}", a, e);
    }};
    ($actual:expr, $expected:expr, $eps:expr) => {{
        let (a, e) = (f64::from($actual), f64::from($expected));
        assert!(approx::approx_eps(a, e, $eps), "{} ≉ {} (ε {})", a, e, $eps);
    }};
}
