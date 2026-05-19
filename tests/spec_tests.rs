//! Reference test suite — all assertions from SPEC.md §8.
//! Run with: cargo test
//!
//! Paths are relative to the package root, which cargo test sets as the cwd.

use universal_wasm_loader::{wasm_load, create_singleton, InstancePool, Callbacks, wasm_interface};

// ── math_50 — numeric round-trip ─────────────────────────────────────────────

#[test]
fn math_add() -> anyhow::Result<()> {
    let m = wasm_load("tests/math_50.wasm", None)?;
    let r: i32 = m.call("add", (3i32, 4i32))?;
    assert_eq!(r, 7);
    Ok(())
}

#[test]
fn math_multiply() -> anyhow::Result<()> {
    let m = wasm_load("tests/math_50.wasm", None)?;
    let r: f64 = m.call("multiply", (2.5f64, 4.0f64))?;
    assert_eq!(r, 10.0);
    Ok(())
}

#[test]
fn math_square() -> anyhow::Result<()> {
    let m = wasm_load("tests/math_50.wasm", None)?;
    let r: i32 = m.call("square", (5i32,))?;
    assert_eq!(r, 25);
    Ok(())
}

// ── booleans_50 — bool normalization ─────────────────────────────────────────

#[test]
fn bool_is_positive_true() -> anyhow::Result<()> {
    let m = wasm_load("tests/booleans_50.wasm", None)?;
    let r: bool = m.call("is_positive", (1.0f64,))?;
    assert!(r);
    Ok(())
}

#[test]
fn bool_is_positive_false() -> anyhow::Result<()> {
    let m = wasm_load("tests/booleans_50.wasm", None)?;
    let r: bool = m.call("is_positive", (-1.0f64,))?;
    assert!(!r);
    Ok(())
}

#[test]
fn bool_in_range_true() -> anyhow::Result<()> {
    let m = wasm_load("tests/booleans_50.wasm", None)?;
    let r: bool = m.call("in_range", (5.0f64, 0.0f64, 10.0f64))?;
    assert!(r);
    Ok(())
}

#[test]
fn bool_in_range_false() -> anyhow::Result<()> {
    let m = wasm_load("tests/booleans_50.wasm", None)?;
    let r: bool = m.call("in_range", (11.0f64, 0.0f64, 10.0f64))?;
    assert!(!r);
    Ok(())
}

#[test]
fn bool_is_even_true() -> anyhow::Result<()> {
    let m = wasm_load("tests/booleans_50.wasm", None)?;
    let r: bool = m.call("is_even", (4i32,))?;
    assert!(r);
    Ok(())
}

#[test]
fn bool_is_even_false() -> anyhow::Result<()> {
    let m = wasm_load("tests/booleans_50.wasm", None)?;
    let r: bool = m.call("is_even", (3i32,))?;
    assert!(!r);
    Ok(())
}

// ── strings_50 — string param + return ───────────────────────────────────────

#[test]
fn str_greet() -> anyhow::Result<()> {
    let m = wasm_load("tests/strings_50.wasm", None)?;
    let r: String = m.call("greet", ("World",))?;
    assert_eq!(r, "Hello, World!");
    Ok(())
}

#[test]
fn str_shout() -> anyhow::Result<()> {
    let m = wasm_load("tests/strings_50.wasm", None)?;
    let r: String = m.call("shout", ("hi",))?;
    assert_eq!(r, "hihi");
    Ok(())
}

#[test]
fn str_len() -> anyhow::Result<()> {
    let m = wasm_load("tests/strings_50.wasm", None)?;
    let r: i32 = m.call("str_len", ("hello",))?;
    assert_eq!(r, 5);
    Ok(())
}

// ── imports_50 — host import callbacks ───────────────────────────────────────

fn imports_50_cbs() -> Callbacks {
    Callbacks::new()
        .on("envMul", |(a, b): (f64, f64)| a * b)
        .on("envAdd", |(a, b): (i32, i32)| a + b)
}

#[test]
fn imports_scale() -> anyhow::Result<()> {
    let m = wasm_load("tests/imports_50.wasm", Some(imports_50_cbs()))?;
    let r: f64 = m.call("scale", (3.0f64, 4.0f64))?;
    assert_eq!(r, 12.0);
    Ok(())
}

#[test]
fn imports_combine() -> anyhow::Result<()> {
    let m = wasm_load("tests/imports_50.wasm", Some(imports_50_cbs()))?;
    let r: i32 = m.call("combine", (10i32, 7i32))?;
    assert_eq!(r, 17);
    Ok(())
}

// ── Instance lifecycle ────────────────────────────────────────────────────────

#[test]
fn singleton_same_instance() -> anyhow::Result<()> {
    let get_mod = create_singleton("tests/math_50.wasm", None);
    let a = get_mod()?;
    let b = get_mod()?;
    assert!(a.ptr_eq(&b), "singleton must return same Arc-backed instance");
    Ok(())
}

#[test]
fn singleton_correct_result() -> anyhow::Result<()> {
    let get_mod = create_singleton("tests/math_50.wasm", None);
    let m = get_mod()?;
    let r: i32 = m.call("add", (1i32, 2i32))?;
    assert_eq!(r, 3);
    Ok(())
}

#[tokio::test]
async fn pool_run_returns_correct_result() -> anyhow::Result<()> {
    let pool = InstancePool::new("tests/math_50.wasm", None, 2).await?;
    let r: i32 = pool.run(|m| m.call("add", (10i32, 5i32))).await?;
    assert_eq!(r, 15);
    Ok(())
}

#[tokio::test]
async fn pool_concurrent_run() -> anyhow::Result<()> {
    let pool = std::sync::Arc::new(
        InstancePool::new("tests/booleans_50.wasm", None, 2).await?
    );
    let p1 = pool.clone();
    let p2 = pool.clone();
    let (a, b) = tokio::join!(
        tokio::spawn(async move { p1.run(|m| -> anyhow::Result<bool> { m.call("is_even", (4i32,)) }).await }),
        tokio::spawn(async move { p2.run(|m| -> anyhow::Result<bool> { m.call("is_even", (3i32,)) }).await }),
    );
    assert!(a??);
    assert!(!b??);
    Ok(())
}

// ── bind — extract export as a standalone callable ────────────────────────────

#[test]
fn bind_add() -> anyhow::Result<()> {
    let m = wasm_load("tests/math_50.wasm", None)?;
    let add = m.bind::<(i32, i32), i32>("add");
    assert_eq!(add((10, 5))?, 15);
    Ok(())
}

#[test]
fn bind_multiple_exports() -> anyhow::Result<()> {
    let m = wasm_load("tests/math_50.wasm", None)?;
    let add    = m.bind::<(i32, i32), i32>("add");
    let square = m.bind::<(i32,), i32>("square");
    assert_eq!(add((3, 4))?, 7);
    assert_eq!(square((5,))?, 25);
    Ok(())
}

#[test]
fn bind_bool_export() -> anyhow::Result<()> {
    let m = wasm_load("tests/booleans_50.wasm", None)?;
    let is_even = m.bind::<(i32,), bool>("is_even");
    assert!(is_even((4,))?);
    assert!(!is_even((3,))?);
    Ok(())
}

// ── wasm_interface! — typed handle struct ─────────────────────────────────────

wasm_interface! {
    pub struct Math {
        fn add(a: i32, b: i32) -> i32;
        fn multiply(a: f64, b: f64) -> f64;
        fn square(x: i32) -> i32;
    }
}

wasm_interface! {
    pub struct Booleans {
        fn is_positive(x: f64) -> bool;
        fn in_range(x: f64, lo: f64, hi: f64) -> bool;
        fn is_even(n: i32) -> bool;
    }
}

wasm_interface! {
    pub struct Strings {
        fn greet(name: &str) -> String;
        fn shout(s: &str) -> String;
        fn str_len(s: &str) -> i32;
    }
}

#[test]
fn interface_math() -> anyhow::Result<()> {
    let math = Math::load("tests/math_50.wasm", None)?;
    assert_eq!(math.add(3, 4)?, 7);
    assert_eq!(math.multiply(2.5, 4.0)?, 10.0);
    assert_eq!(math.square(5)?, 25);
    Ok(())
}

#[test]
fn interface_booleans() -> anyhow::Result<()> {
    let b = Booleans::load("tests/booleans_50.wasm", None)?;
    assert!(b.is_positive(1.0)?);
    assert!(!b.is_positive(-1.0)?);
    assert!(b.in_range(5.0, 0.0, 10.0)?);
    assert!(!b.in_range(11.0, 0.0, 10.0)?);
    assert!(b.is_even(4)?);
    assert!(!b.is_even(3)?);
    Ok(())
}

#[test]
fn interface_strings() -> anyhow::Result<()> {
    let s = Strings::load("tests/strings_50.wasm", None)?;
    assert_eq!(s.greet("World")?, "Hello, World!");
    assert_eq!(s.shout("hi")?, "hihi");
    assert_eq!(s.str_len("hello")?, 5);
    Ok(())
}
