//! Universal WASM Loader — Rust port of the UniversalWasmLoader Spec v2.0.0.
//!
//! Load a `.wasm` file (or URL) with automatic WIT-based ABI translation:
//!
//! ```no_run
//! use universal_wasm_loader::wasm_import;
//!
//! # async fn example() -> anyhow::Result<()> {
//! // From a relative file path (resolved against the current working directory):
//! let m = wasm_import("tests/math_50.wasm", None).await?;
//!
//! // From a URL:
//! let m = wasm_import("https://example.com/math.wasm", None).await?;
//!
//! let result: i32 = m.call("add", (3i32, 4i32))?;
//! assert_eq!(result, 7);
//! # Ok(())
//! # }
//! ```

pub mod abi;
pub mod types;
pub mod wit_parser;

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use anyhow::{anyhow, Result};
use once_cell::sync::OnceCell;
use tokio::sync::Semaphore;
use wasmtime::{Engine, Linker, Module, Store};

pub use abi::{HostCallback, WasmVal};
pub use types::{IntoWasmVal, FromWasmVal, WasmArgs, FromWasmArgs, host_fn};
pub use wit_parser::ParsedWit;

// ── Internal ABI kind (not part of public API) ────────────────────────────────

#[derive(Debug, Clone)]
enum AbiKind {
    Component,
    Wasic,
}

// ── Callbacks ─────────────────────────────────────────────────────────────────

/// Host callbacks for WASM imports.
///
/// Keys are the **camelCase** WIT import name (e.g. `"envMul"` for `env-mul`).
/// Pass a typed closure to [`on`] — argument and return types are decoded automatically:
///
/// ```no_run
/// use universal_wasm_loader::{Callbacks, wasm_import};
///
/// # async fn example() -> anyhow::Result<()> {
/// let cbs = Callbacks::new()
///     .on("envMul", |(a, b): (f64, f64)| a * b)
///     .on("envAdd", |(a, b): (i32, i32)| a + b);
///
/// let m = wasm_import("tests/imports_50.wasm", Some(cbs)).await?;
/// # Ok(())
/// # }
/// ```
///
/// [`on`]: Callbacks::on
#[derive(Default, Clone)]
pub struct Callbacks {
    pub(crate) map: HashMap<String, HostCallback>,
}

impl Callbacks {
    pub fn new() -> Self { Self::default() }

    /// Register a host callback by its camelCase WIT import name.
    ///
    /// The closure receives decoded Rust values (as a destructured tuple) and returns
    /// a native value. Argument and return types are inferred from the closure signature.
    pub fn on<F, Args, R>(mut self, name: impl Into<String>, f: F) -> Self
    where
        Args: FromWasmArgs + 'static,
        R: IntoWasmVal + 'static,
        F: Fn(Args) -> R + Send + Sync + 'static,
    {
        self.map.insert(name.into(), Arc::new(move |raw: &[WasmVal]| {
            let args = Args::from_wasm_vals(raw)?;
            Ok(f(args).into_wasm_val())
        }));
        self
    }

    /// Register a pre-built [`HostCallback`] (e.g. one returned by [`host_fn`]).
    ///
    /// Use this when you need to store or share a callback value before registering it.
    /// For inline closures, prefer [`on`].
    ///
    /// [`on`]: Callbacks::on
    pub fn on_raw(mut self, name: impl Into<String>, cb: HostCallback) -> Self {
        self.map.insert(name.into(), cb); self
    }
}

// ── ModuleExports ─────────────────────────────────────────────────────────────

/// A loaded, ABI-translated WASM module.
///
/// Use [`call`] to invoke exported functions by their snake_case name.
///
/// [`call`]: ModuleExports::call
#[derive(Clone)]
pub struct ModuleExports {
    inner: Arc<Mutex<ExportsInner>>,
}

struct ExportsInner {
    store:    Store<()>,
    instance: wasmtime::Instance,
    parsed:   Option<ParsedWit>,
    abi:      AbiKind,
}

impl ModuleExports {
    /// Call a WIT export with native Rust argument and return types.
    ///
    /// ```no_run
    /// # use universal_wasm_loader::wasm_import;
    /// # async fn ex() -> anyhow::Result<()> {
    /// # let m = wasm_import("x.wasm", None).await?;
    /// let n: i32    = m.call("add",         (3i32, 4i32))?;
    /// let s: String = m.call("greet",       ("World",))?;
    /// let b: bool   = m.call("is_positive", (1.0f64,))?;
    /// # Ok(()) }
    /// ```
    pub fn call<Args: WasmArgs, R: FromWasmVal>(&self, name: &str, args: Args) -> Result<R> {
        let vals = args.into_wasm_vals();
        let raw = self.call_dyn(name, &vals)?;
        R::from_wasm_val(raw)
    }

    /// Raw dynamic dispatch — accepts and returns [`WasmVal`] directly.
    pub fn call_dyn(&self, name: &str, args: &[WasmVal]) -> Result<WasmVal> {
        let mut g = self.inner.lock().unwrap();
        let ExportsInner { store, instance, parsed, abi } = &mut *g;

        match parsed {
            Some(p) => {
                let func = p.exports.iter()
                    .find(|f| f.snake_name == name)
                    .ok_or_else(|| anyhow!("no export '{name}' in WIT"))?
                    .clone();
                match abi {
                    AbiKind::Component => abi::call_component_export(store, instance, &func, args),
                    AbiKind::Wasic     => abi::call_wasic_export(store, instance, &func, args),
                }
            }
            None => abi::call_raw_export(store, instance, name, args),
        }
    }

    /// Bind a named export as a standalone callable — no `await` needed at call time.
    ///
    /// The returned closure owns a clone of this handle and captures the export name,
    /// so it can be stored and called repeatedly without re-loading the module.
    ///
    /// ```no_run
    /// # use universal_wasm_loader::wasm_load;
    /// let m = wasm_load("tests/math_50.wasm", None)?;
    /// let add = m.bind::<(i32, i32), i32>("add");
    /// assert_eq!(add((3, 4))?, 7);
    /// # anyhow::Ok(())
    /// ```
    pub fn bind<Args, R>(&self, name: &str) -> impl Fn(Args) -> Result<R>
    where
        Args: WasmArgs,
        R: FromWasmVal,
    {
        let me = self.clone();
        let name = name.to_string();
        move |args: Args| me.call(&name, args)
    }

    /// Check pointer equality — two values from the same [`create_singleton`] call
    /// share the same underlying instance.
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

// ── Source loading ────────────────────────────────────────────────────────────

fn is_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
}

fn derive_wit_source(source: &str) -> String {
    if let Some(pos) = source.rfind(".wasm") {
        let mut s = source.to_string();
        s.replace_range(pos..pos + 5, ".wit");
        s
    } else {
        format!("{source}.wit")
    }
}

/// Load WASM bytes and optional WIT source from a file path or URL.
async fn load_source(source: &str) -> Result<(Vec<u8>, Option<String>)> {
    if is_url(source) {
        load_url(source).await
    } else {
        load_file(source).await
    }
}

async fn load_file(path: &str) -> Result<(Vec<u8>, Option<String>)> {
    let wasm_bytes = tokio::fs::read(path).await
        .map_err(|e| anyhow!("failed to read '{}': {e}", path))?;
    let wit_src = tokio::fs::read_to_string(derive_wit_source(path)).await.ok();
    Ok((wasm_bytes, wit_src))
}

async fn load_url(url: &str) -> Result<(Vec<u8>, Option<String>)> {
    let wasm_bytes = reqwest::get(url).await
        .map_err(|e| anyhow!("failed to fetch '{}': {e}", url))?
        .error_for_status()
        .map_err(|e| anyhow!("HTTP error fetching '{}': {e}", url))?
        .bytes().await
        .map_err(|e| anyhow!("failed to read response body from '{}': {e}", url))?
        .to_vec();

    let wit_url = derive_wit_source(url);
    let wit_src = match reqwest::get(&wit_url).await {
        Ok(r) if r.status().is_success() => r.text().await.ok(),
        _ => None,
    };

    Ok((wasm_bytes, wit_src))
}

// ── Core instantiation ────────────────────────────────────────────────────────

fn parse_version_suffix(s: &str) -> (&str, Option<i32>) {
    if let Some(at) = s.rfind('@') {
        if let Ok(n) = s[at + 1..].parse::<i32>() {
            return (&s[..at], Some(n));
        }
    }
    (s, None)
}

/// Compile and instantiate from pre-loaded bytes. CPU-intensive; run in spawn_blocking.
fn instantiate_from_bytes(
    wasm_bytes: Vec<u8>,
    wit_src: Option<String>,
    callbacks: &Callbacks,
    required_version: Option<i32>,
) -> Result<ModuleExports> {
    let engine = Engine::default();
    let module = Module::new(&engine, &wasm_bytes)?;
    let abi_kind = detect_abi(&module);

    let parsed = wit_src.map(|src| wit_parser::parse_wit(&src));

    let mut linker: Linker<()> = Linker::new(&engine);
    if let Some(ref p) = parsed {
        if !p.imports.is_empty() {
            abi::link_import_env(&mut linker, &p.imports, &callbacks.map)?;
        }
    }

    let mut store = Store::new(&engine, ());
    let instance = linker.instantiate(&mut store, &module)?;

    if let Some(required) = required_version {
        let ver_global = instance.get_global(&mut store, "version")
            .ok_or_else(|| anyhow!("@{required} pinned but module has no 'version' global"))?;
        let actual = ver_global.get(&mut store).unwrap_i32();
        if actual != required {
            return Err(anyhow!("version mismatch: module is v{actual}, requested @{required}"));
        }
    }

    Ok(ModuleExports {
        inner: Arc::new(Mutex::new(ExportsInner { store, instance, parsed, abi: abi_kind })),
    })
}

/// Synchronous load + compile for use in non-async contexts (singleton, pool).
fn instantiate_sync(path: &Path, callbacks: &Callbacks) -> Result<ModuleExports> {
    let path_str = path.to_string_lossy();
    let (actual, required_version) = parse_version_suffix(&path_str);
    let actual_path = Path::new(actual);

    let wasm_bytes = std::fs::read(actual_path)
        .map_err(|e| anyhow!("failed to read '{}': {e}", actual_path.display()))?;

    let wit_path = {
        let mut p = actual_path.to_path_buf();
        p.set_extension("wit");
        p
    };
    let wit_src = std::fs::read_to_string(&wit_path).ok();

    instantiate_from_bytes(wasm_bytes, wit_src, callbacks, required_version)
}

fn detect_abi(module: &Module) -> AbiKind {
    for export in module.exports() {
        if export.name() == "cabi_realloc" {
            return AbiKind::Component;
        }
    }
    AbiKind::Wasic
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Load a `.wasm` module and return a [`ModuleExports`] handle.
///
/// `source` may be:
/// - A **relative file path** — resolved against the current working directory
///   (`cargo test` sets this to the package root, so `"tests/math_50.wasm"` just works)
/// - An **absolute file path**
/// - An **`http://` or `https://` URL** — WASM and companion `.wit` are both fetched
///
/// Version pinning: append `@N` to assert the module's `version` i32 global equals N.
///
/// The companion `.wit` file is auto-detected (`.wasm` → `.wit`). If not found,
/// raw WASM exports are returned with no ABI translation.
pub async fn wasm_import(
    source: impl AsRef<Path>,
    callbacks: Option<Callbacks>,
) -> Result<ModuleExports> {
    let raw = source.as_ref().to_string_lossy().into_owned();
    let cbs = callbacks.unwrap_or_default();

    let (actual, required_version) = {
        let (s, v) = parse_version_suffix(&raw);
        (s.to_string(), v)
    };

    let (wasm_bytes, wit_src) = load_source(&actual).await?;

    tokio::task::spawn_blocking(move || {
        instantiate_from_bytes(wasm_bytes, wit_src, &cbs, required_version)
    }).await?
}

// ── Singleton ─────────────────────────────────────────────────────────────────

/// Returns a closure that loads the WASM instance on the first call and caches
/// the result. All subsequent calls return a clone of the same `Arc`-backed value.
pub fn create_singleton(
    wasm_path: impl AsRef<Path> + Send + Sync + 'static,
    callbacks: Option<Callbacks>,
) -> impl Fn() -> Result<ModuleExports> {
    let path = wasm_path.as_ref().to_path_buf();
    let cbs = callbacks.unwrap_or_default();
    let cell: Arc<OnceCell<Arc<Mutex<ExportsInner>>>> = Arc::new(OnceCell::new());

    move || {
        let inner = cell.get_or_try_init(|| -> Result<Arc<Mutex<ExportsInner>>> {
            let exports = instantiate_sync(&path, &cbs)?;
            Ok(exports.inner)
        })?;
        Ok(ModuleExports { inner: inner.clone() })
    }
}

// ── InstancePool ──────────────────────────────────────────────────────────────

/// A pool of pre-instantiated WASM modules.
///
/// Each instance has its own independent linear memory. Use [`run`] for an
/// atomic acquire → call → release pattern.
///
/// [`run`]: InstancePool::run
pub struct InstancePool {
    available: Arc<Mutex<Vec<ModuleExports>>>,
    semaphore: Arc<Semaphore>,
    size: usize,
}

impl InstancePool {
    /// Pre-instantiate `size` independent WASM instances.
    pub async fn new(
        wasm_path: impl AsRef<Path>,
        callbacks: Option<Callbacks>,
        size: usize,
    ) -> Result<Self> {
        let path = wasm_path.as_ref().to_path_buf();
        let cbs = callbacks.unwrap_or_default();

        let mut instances = Vec::with_capacity(size);
        for _ in 0..size {
            let p2 = path.clone();
            let o2 = cbs.clone();
            let inst = tokio::task::spawn_blocking(move || instantiate_sync(&p2, &o2)).await??;
            instances.push(inst);
        }

        Ok(Self {
            available: Arc::new(Mutex::new(instances)),
            semaphore: Arc::new(Semaphore::new(size)),
            size,
        })
    }

    /// Total pool capacity.
    pub fn size(&self) -> usize { self.size }

    /// Number of currently idle instances.
    pub fn available(&self) -> usize {
        self.available.lock().unwrap().len()
    }

    /// Check out an instance. Waits if all are in use.
    pub async fn acquire(&self) -> Result<ModuleExports> {
        let _permit = self.semaphore.acquire().await
            .map_err(|_| anyhow!("pool semaphore closed"))?;
        std::mem::forget(_permit);
        let inst = self.available.lock().unwrap().pop()
            .ok_or_else(|| anyhow!("pool empty after semaphore acquire"))?;
        Ok(inst)
    }

    /// Return an instance to the pool.
    pub fn release(&self, inst: ModuleExports) {
        self.available.lock().unwrap().push(inst);
        self.semaphore.add_permits(1);
    }

    /// Acquire an instance, call `f`, release — even if `f` returns `Err`.
    pub async fn run<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&ModuleExports) -> Result<R>,
    {
        let inst = self.acquire().await?;
        let result = f(&inst);
        self.release(inst);
        result
    }
}

// ── wasm_load — sync entry point ──────────────────────────────────────────────

/// Load a `.wasm` module synchronously — no `await` needed.
///
/// Reads from a **file path** only (URLs require the async [`wasm_import`]).
/// Version pinning (`@N`) is supported the same way.
///
/// Use this when you want to call `wasm_load` from a non-async context, or
/// together with [`bind`] / [`wasm_interface!`] for a fully synchronous API.
///
/// ```no_run
/// use universal_wasm_loader::wasm_load;
///
/// let m = wasm_load("tests/math_50.wasm", None)?;
/// let r: i32 = m.call("add", (3i32, 4i32))?;
/// assert_eq!(r, 7);
/// # anyhow::Ok(())
/// ```
///
/// [`bind`]: ModuleExports::bind
pub fn wasm_load(source: impl AsRef<Path>, callbacks: Option<Callbacks>) -> Result<ModuleExports> {
    instantiate_sync(source.as_ref(), &callbacks.unwrap_or_default())
}

// ── wasm_interface! macro ─────────────────────────────────────────────────────

/// Generate a typed handle struct for a WASM module.
///
/// Declares a struct whose methods map 1-to-1 to WASM exports. The generated
/// `load` associated function is **synchronous** — no `await` required.
///
/// # Example
///
/// ```no_run
/// use universal_wasm_loader::wasm_interface;
///
/// wasm_interface! {
///     pub struct Math {
///         fn add(a: i32, b: i32) -> i32;
///         fn square(x: i32) -> i32;
///     }
/// }
///
/// let math = Math::load("tests/math_50.wasm", None)?;
/// assert_eq!(math.add(3, 4)?, 7);
/// assert_eq!(math.square(5)?, 25);
/// # anyhow::Ok(())
/// ```
#[macro_export]
macro_rules! wasm_interface {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident {
            $(fn $fn_name:ident($($param:ident: $param_ty:ty),*) -> $ret_ty:ty;)*
        }
    ) => {
        $(#[$meta])*
        $vis struct $name($crate::ModuleExports);

        impl $name {
            /// Load the WASM module synchronously and return a typed handle.
            pub fn load(
                source: impl ::std::convert::AsRef<::std::path::Path>,
                callbacks: ::std::option::Option<$crate::Callbacks>,
            ) -> ::anyhow::Result<Self> {
                $crate::wasm_load(source, callbacks).map(Self)
            }

            $(
                pub fn $fn_name(&self, $($param: $param_ty),*) -> ::anyhow::Result<$ret_ty> {
                    self.0.call(stringify!($fn_name), ($($param,)*))
                }
            )*
        }
    };
}
