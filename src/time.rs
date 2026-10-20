//! Exact media time. Arithmetic never uses floating point; serialization is checked.
use anyhow::{bail, ensure, Context, Result};
use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::{Signed, ToPrimitive, Zero};
use serde_json::{json, Value};
use std::{fmt, str::FromStr};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Time(pub BigRational);
impl Time {
    pub fn zero() -> Self {
        Self(BigRational::zero())
    }
    pub fn integer(n: i64) -> Self {
        Self(BigRational::from_integer(n.into()))
    }
    pub fn new(n: i64, d: i64) -> Result<Self> {
        ensure!(d > 0, "time denominator must be positive");
        Ok(Self(BigRational::new(n.into(), d.into())))
    }
    pub fn from_json(v: &Value) -> Result<Self> {
        Self::new(
            v["num"]
                .as_i64()
                .context("time numerator must be a signed 64-bit integer")?,
            v["den"]
                .as_i64()
                .context("time denominator must be a positive signed 64-bit integer")?,
        )
    }
    pub fn json(&self) -> Result<Value> {
        Ok(
            json!({"num":self.0.numer().to_i64().context("time numerator overflow")?,"den":self.0.denom().to_i64().context("time denominator overflow")?}),
        )
    }
    pub fn add(&self, other: &Self) -> Self {
        Self(&self.0 + &other.0)
    }
    pub fn sub(&self, other: &Self) -> Self {
        Self(&self.0 - &other.0)
    }
    pub fn mul(&self, other: &Self) -> Self {
        Self(&self.0 * &other.0)
    }
    pub fn div(&self, other: &Self) -> Result<Self> {
        ensure!(!other.0.is_zero(), "division by zero time");
        Ok(Self(&self.0 / &other.0))
    }
    pub fn nonnegative(&self) -> bool {
        !self.0.is_negative()
    }
    pub fn ceil_i64(&self) -> Result<i64> {
        self.0
            .ceil()
            .to_integer()
            .to_i64()
            .context("sample count overflow")
    }
    /// FFmpeg expression, exact until the media backend evaluates it.
    pub fn expression(&self) -> String {
        format!("{}/{}", self.0.numer(), self.0.denom())
    }
}
impl fmt::Display for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.expression())
    }
}
impl FromStr for Time {
    type Err = anyhow::Error;
    fn from_str(input: &str) -> Result<Self> {
        let input = input.strip_suffix('s').unwrap_or(input);
        let (n, d) = if let Some((n, d)) = input.split_once('/') {
            (n.parse::<BigInt>()?, d.parse::<BigInt>()?)
        } else if let Some((whole, part)) = input.split_once('.') {
            ensure!(
                !part.is_empty() && part.len() <= 18 && part.bytes().all(|c| c.is_ascii_digit()),
                "invalid decimal time"
            );
            let den = BigInt::from(10u8).pow(part.len() as u32);
            let sign = if whole.starts_with('-') { -1 } else { 1 };
            let whole = if whole.is_empty() { "0" } else { whole };
            (
                whole.parse::<BigInt>()? * &den + part.parse::<BigInt>()? * sign,
                den,
            )
        } else {
            (input.parse::<BigInt>()?, 1.into())
        };
        ensure!(d > BigInt::zero(), "time denominator must be positive");
        let time = Self(BigRational::new(n, d));
        if !time.nonnegative() {
            bail!("time must be non-negative");
        }
        time.json()?;
        Ok(time)
    }
}
pub fn bounds(v: &Value) -> Result<(Time, Time)> {
    let start = Time::from_json(&v["start"])?;
    let end = start.add(&Time::from_json(&v["duration"])?);
    Ok((start, end))
}
pub fn range(start: &Time, duration: &Time) -> Result<Value> {
    ensure!(
        start.nonnegative() && duration > &Time::zero(),
        "invalid time range"
    );
    Ok(json!({"start":start.json()?,"duration":duration.json()?}))
}
