use std::sync::Arc;
use anyhow::{anyhow, Result};
use crate::abi::{HostCallback, WasmVal};

// ── IntoWasmVal ───────────────────────────────────────────────────────────────

/// Convert a native Rust value into a [`WasmVal`] for passing to a WASM export.
pub trait IntoWasmVal {
    fn into_wasm_val(self) -> WasmVal;
}

impl IntoWasmVal for i32    { fn into_wasm_val(self) -> WasmVal { WasmVal::I32(self) } }
impl IntoWasmVal for i64    { fn into_wasm_val(self) -> WasmVal { WasmVal::I64(self) } }
impl IntoWasmVal for f32    { fn into_wasm_val(self) -> WasmVal { WasmVal::F32(self) } }
impl IntoWasmVal for f64    { fn into_wasm_val(self) -> WasmVal { WasmVal::F64(self) } }
impl IntoWasmVal for bool   { fn into_wasm_val(self) -> WasmVal { WasmVal::Bool(self) } }
impl IntoWasmVal for String { fn into_wasm_val(self) -> WasmVal { WasmVal::Str(self) } }
impl IntoWasmVal for &str   { fn into_wasm_val(self) -> WasmVal { WasmVal::Str(self.to_string()) } }

// ── FromWasmVal ───────────────────────────────────────────────────────────────

/// Extract a native Rust value from a [`WasmVal`] returned by a WASM export.
pub trait FromWasmVal: Sized {
    fn from_wasm_val(v: WasmVal) -> Result<Self>;
}

impl FromWasmVal for i32 {
    fn from_wasm_val(v: WasmVal) -> Result<Self> {
        v.as_i32().ok_or_else(|| anyhow!("expected i32, got {:?}", v))
    }
}
impl FromWasmVal for i64 {
    fn from_wasm_val(v: WasmVal) -> Result<Self> {
        v.as_i64().ok_or_else(|| anyhow!("expected i64, got {:?}", v))
    }
}
impl FromWasmVal for f32 {
    fn from_wasm_val(v: WasmVal) -> Result<Self> {
        v.as_f32().ok_or_else(|| anyhow!("expected f32, got {:?}", v))
    }
}
impl FromWasmVal for f64 {
    fn from_wasm_val(v: WasmVal) -> Result<Self> {
        v.as_f64().ok_or_else(|| anyhow!("expected f64, got {:?}", v))
    }
}
impl FromWasmVal for bool {
    fn from_wasm_val(v: WasmVal) -> Result<Self> {
        v.as_bool().ok_or_else(|| anyhow!("expected bool, got {:?}", v))
    }
}
impl FromWasmVal for String {
    fn from_wasm_val(v: WasmVal) -> Result<Self> {
        v.as_str().map(|s| s.to_string()).ok_or_else(|| anyhow!("expected string, got {:?}", v))
    }
}
impl FromWasmVal for () {
    fn from_wasm_val(_: WasmVal) -> Result<Self> { Ok(()) }
}

// ── WasmArgs — tuple of args going INTO a WASM export ────────────────────────

/// A tuple of [`IntoWasmVal`] values that can be passed as export call arguments.
pub trait WasmArgs {
    fn into_wasm_vals(self) -> Vec<WasmVal>;
}

impl WasmArgs for () {
    fn into_wasm_vals(self) -> Vec<WasmVal> { vec![] }
}
impl<A: IntoWasmVal> WasmArgs for (A,) {
    fn into_wasm_vals(self) -> Vec<WasmVal> { vec![self.0.into_wasm_val()] }
}
impl<A: IntoWasmVal, B: IntoWasmVal> WasmArgs for (A, B) {
    fn into_wasm_vals(self) -> Vec<WasmVal> {
        vec![self.0.into_wasm_val(), self.1.into_wasm_val()]
    }
}
impl<A: IntoWasmVal, B: IntoWasmVal, C: IntoWasmVal> WasmArgs for (A, B, C) {
    fn into_wasm_vals(self) -> Vec<WasmVal> {
        vec![self.0.into_wasm_val(), self.1.into_wasm_val(), self.2.into_wasm_val()]
    }
}
impl<A: IntoWasmVal, B: IntoWasmVal, C: IntoWasmVal, D: IntoWasmVal> WasmArgs for (A, B, C, D) {
    fn into_wasm_vals(self) -> Vec<WasmVal> {
        vec![
            self.0.into_wasm_val(), self.1.into_wasm_val(),
            self.2.into_wasm_val(), self.3.into_wasm_val(),
        ]
    }
}

// ── FromWasmArgs — tuple of args coming OUT of WASM into a host callback ─────

/// A tuple of [`FromWasmVal`] values decoded from the `&[WasmVal]` slice
/// that WASM passes to a host import callback.
pub trait FromWasmArgs: Sized {
    fn from_wasm_vals(vals: &[WasmVal]) -> Result<Self>;
}

impl<A: FromWasmVal> FromWasmArgs for (A,) {
    fn from_wasm_vals(vals: &[WasmVal]) -> Result<Self> {
        Ok((A::from_wasm_val(vals.first().cloned().unwrap_or(WasmVal::Void))?,))
    }
}
impl<A: FromWasmVal, B: FromWasmVal> FromWasmArgs for (A, B) {
    fn from_wasm_vals(vals: &[WasmVal]) -> Result<Self> {
        Ok((
            A::from_wasm_val(vals.first().cloned().unwrap_or(WasmVal::Void))?,
            B::from_wasm_val(vals.get(1).cloned().unwrap_or(WasmVal::Void))?,
        ))
    }
}
impl<A: FromWasmVal, B: FromWasmVal, C: FromWasmVal> FromWasmArgs for (A, B, C) {
    fn from_wasm_vals(vals: &[WasmVal]) -> Result<Self> {
        Ok((
            A::from_wasm_val(vals.first().cloned().unwrap_or(WasmVal::Void))?,
            B::from_wasm_val(vals.get(1).cloned().unwrap_or(WasmVal::Void))?,
            C::from_wasm_val(vals.get(2).cloned().unwrap_or(WasmVal::Void))?,
        ))
    }
}

// ── host_fn ───────────────────────────────────────────────────────────────────

/// Wrap a typed closure as a [`HostCallback`] for storing or passing as a value.
///
/// Most callers can pass closures directly to [`Callbacks::on`] without this wrapper.
/// Use `host_fn` when you need a standalone [`HostCallback`] — for example, to store
/// one in a variable or share it across multiple registrations. Pass the result to
/// [`Callbacks::on_raw`].
///
/// # Examples
///
/// ```no_run
/// use universal_wasm_loader::{host_fn, Callbacks};
///
/// // Store a callback in a variable, then register it.
/// let mul_cb = host_fn(|(a, b): (f64, f64)| a * b);
///
/// let cbs = Callbacks::new()
///     .on("envMul", |(a, b): (f64, f64)| a * b)  // inline closure
///     .on_raw("envAdd", host_fn(|(a, b): (i32, i32)| a + b)); // pre-built HostCallback
/// ```
pub fn host_fn<Args, R, F>(f: F) -> HostCallback
where
    Args: FromWasmArgs + 'static,
    R: IntoWasmVal + 'static,
    F: Fn(Args) -> R + Send + Sync + 'static,
{
    Arc::new(move |raw: &[WasmVal]| {
        let args = Args::from_wasm_vals(raw)?;
        Ok(f(args).into_wasm_val())
    })
}
