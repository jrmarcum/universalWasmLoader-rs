# universal-wasm-loader

A universal WebAssembly loader for Rust with automatic WIT-based ABI translation.
Rust port of the [UniversalWasmLoader Spec v2.0.0](https://github.com/jrmarcum/universalWasmLoader-js/blob/main/SPEC.md).

Backed by [wasmtime](https://crates.io/crates/wasmtime). Works with any `.wasm` + `.wit` pair produced by [`wasmtk`](https://github.com/jrmarcum/wasmtk). ABI (wasic vs Canonical) is auto-detected — no profile configuration needed.

---

## Quick Start

```toml
[dependencies]
universal-wasm-loader = "0.1"
```

```rust
use universal_wasm_loader::wasm_load;

fn main() -> anyhow::Result<()> {
    let m = wasm_load("my_module.wasm", None)?;
    let result: i32 = m.call("add", (3i32, 4i32))?;
    println!("{result}"); // 7
    Ok(())
}
```

`wasm_load` auto-detects the companion `.wit` file by replacing `.wasm` → `.wit`. WIT export names are converted from kebab-case to snake_case for the Rust API (e.g. `is-positive` → `"is_positive"`). If no `.wit` file is found, raw WASM exports are returned with no ABI translation.

---

## Typed Interface

Use `wasm_interface!` to declare a named struct with typed methods — the closest Rust equivalent to a TypeScript interface. The `load` function is synchronous; no `await` anywhere in user code.

```rust
use universal_wasm_loader::wasm_interface;

wasm_interface! {
    pub struct Math {
        fn add(a: i32, b: i32) -> i32;
        fn multiply(a: f64, b: f64) -> f64;
        fn square(x: i32) -> i32;
    }
}

fn main() -> anyhow::Result<()> {
    let math = Math::load("math.wasm", None)?;
    assert_eq!(math.add(3, 4)?,      7);
    assert_eq!(math.multiply(2.5, 4.0)?, 10.0);
    assert_eq!(math.square(5)?,      25);
    Ok(())
}
```

---

## Bind Individual Exports

Extract a single export as a standalone callable — useful when you only need one or two functions.

```rust
use universal_wasm_loader::wasm_load;

fn main() -> anyhow::Result<()> {
    let m      = wasm_load("math.wasm", None)?;
    let add    = m.bind::<(i32, i32), i32>("add");
    let square = m.bind::<(i32,), i32>("square");

    assert_eq!(add((3, 4))?,  7);
    assert_eq!(square((5,))?, 25);
    Ok(())
}
```

---

## Host Import Callbacks

Provide callbacks for WIT `import` declarations using `Callbacks::on` — pass and receive native Rust types, no manual encoding. Callback keys are **camelCase** WIT import names (e.g. `"envMul"` for `env-mul`):

```rust
use universal_wasm_loader::{wasm_load, Callbacks};

fn main() -> anyhow::Result<()> {
    let cbs = Callbacks::new()
        .on("envMul", |(a, b): (f64, f64)| a * b)
        .on("envAdd", |(a, b): (i32, i32)| a + b);

    let m = wasm_load("imports.wasm", Some(cbs))?;
    let r: f64 = m.call("scale",   (3.0f64, 4.0f64))?;
    let n: i32 = m.call("combine", (10i32,  7i32))?;
    assert_eq!(r, 12.0);
    assert_eq!(n, 17);
    Ok(())
}
```

---

## URL Loading (Async)

`wasm_import` supports `http://` and `https://` sources. It also works with local file paths when you are already in an async context.

```toml
[dependencies]
universal-wasm-loader = "0.1"
tokio = { version = "1", features = ["full"] }
```

```rust
use universal_wasm_loader::wasm_import;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let m = wasm_import("https://example.com/math.wasm", None).await?;
    let r: i32 = m.call("add", (3i32, 4i32))?;
    println!("{r}");
    Ok(())
}
```

### Version Pinning

Append `@N` to the path to assert the module's `version` i32 global equals N:

```rust
let m = wasm_import("my_module.wasm@2", None).await?;
// errors if module.version != 2
```

---

## Instance Lifecycle

### Singleton

Loads once on the first call; all subsequent calls return a clone of the same `Arc`-backed instance.

```rust
use universal_wasm_loader::create_singleton;

fn main() -> anyhow::Result<()> {
    let get_mod = create_singleton("math.wasm", None);
    let a = get_mod()?;
    let b = get_mod()?;
    assert!(a.ptr_eq(&b)); // same underlying instance
    let r: i32 = a.call("add", (1i32, 2i32))?;
    assert_eq!(r, 3);
    Ok(())
}
```

### InstancePool

Pre-instantiates N independent WASM instances — each with its own linear memory. `run()` atomically acquires, calls, and releases, even if the closure returns an error.

```rust
use universal_wasm_loader::InstancePool;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let pool = InstancePool::new("math.wasm", None, 4).await?;

    let r: i32 = pool.run(|m| m.call("square", (5i32,))).await?;
    assert_eq!(r, 25);

    // concurrent — each run() gets its own instance
    let (a, b) = tokio::join!(
        pool.run(|m| -> anyhow::Result<i32> { m.call("add", (10i32, 5i32)) }),
        pool.run(|m| -> anyhow::Result<i32> { m.call("add", (3i32,  4i32)) }),
    );
    assert_eq!(a?, 15);
    assert_eq!(b?, 7);
    Ok(())
}
```

---

## Type Mapping

`call` accepts any tuple of these types as arguments and returns any of them:

| Rust type | WIT type |
| --- | --- |
| `i32` | `s32` |
| `i64` | `s64` |
| `f32` | `f32` |
| `f64` | `f64` |
| `bool` | `bool` |
| `String` / `&str` (arg) → `String` (return) | `string` |
| `()` | (no return value) |

The same types work in `Callbacks::on` closures — destructure the arg tuple and return any of the above directly.

---

## Running Tests

```sh
cargo test
```

All 24 assertions pass. The suite covers numeric, bool, string, host-import, singleton, pool, bind, and `wasm_interface!` patterns across four fixture modules.

---

## Ecosystem

This crate is part of the [polyglot WASM ecosystem](https://github.com/jrmarcum/wasmtk):

- [`wasmtk`](https://github.com/jrmarcum/wasmtk) — TypeScript-to-WASM compiler and polyglot build CLI
- [`universalWasmLoader-js`](https://github.com/jrmarcum/universalWasmLoader-js) — JS/TS reference implementation and spec
- **`universalWasmLoader-rs`** — this crate
- `universalWasmLoader-py` — Python port (Stage 2, in progress)

---

## License

MIT — see [LICENSE](LICENSE).
