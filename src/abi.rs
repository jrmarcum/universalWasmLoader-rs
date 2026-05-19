use std::collections::HashMap;
use std::sync::Arc;
use wasmtime::{Caller, Extern, FuncType, Linker, Store, ValType};
use anyhow::{anyhow, Result};
use crate::wit_parser::{WitFunc, WitType};

// ── Public value type ─────────────────────────────────────────────────────────

/// A dynamically-typed WASM value with ABI translation applied.
#[derive(Debug, Clone, PartialEq)]
pub enum WasmVal {
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
    Bool(bool),
    Str(String),
    Void,
}

impl WasmVal {
    pub fn as_i32(&self) -> Option<i32>  { if let Self::I32(v)  = self { Some(*v) } else { None } }
    pub fn as_i64(&self) -> Option<i64>  { if let Self::I64(v)  = self { Some(*v) } else { None } }
    pub fn as_f32(&self) -> Option<f32>  { if let Self::F32(v)  = self { Some(*v) } else { None } }
    pub fn as_f64(&self) -> Option<f64>  { if let Self::F64(v)  = self { Some(*v) } else { None } }
    pub fn as_bool(&self) -> Option<bool> { if let Self::Bool(v) = self { Some(*v) } else { None } }
    pub fn as_str(&self)  -> Option<&str> { if let Self::Str(v)  = self { Some(v)  } else { None } }
}

/// A host callback — receives and returns decoded `WasmVal`s.
pub type HostCallback = Arc<dyn Fn(&[WasmVal]) -> Result<WasmVal> + Send + Sync>;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn read_str(data: &[u8], ptr: u32, len: u32) -> Result<String> {
    let s = ptr as usize;
    let e = s + len as usize;
    if e > data.len() {
        return Err(anyhow!("string read OOB ptr={ptr} len={len}"));
    }
    Ok(String::from_utf8(data[s..e].to_vec())?)
}

fn wit_to_wasm_params(params: &[crate::wit_parser::WitParam]) -> Vec<ValType> {
    params.iter().flat_map(|p| match &p.ty {
        WitType::Str  => vec![ValType::I32, ValType::I32],
        WitType::S64  => vec![ValType::I64],
        WitType::F32  => vec![ValType::F32],
        WitType::F64  => vec![ValType::F64],
        _             => vec![ValType::I32],
    }).collect()
}

fn wit_to_wasm_results(result: &Option<WitType>) -> Vec<ValType> {
    match result {
        None                  => vec![],
        Some(WitType::S64)    => vec![ValType::I64],
        Some(WitType::F32)    => vec![ValType::F32],
        Some(WitType::F64)    => vec![ValType::F64],
        Some(WitType::Str)    => vec![],
        _                     => vec![ValType::I32],
    }
}

// ── Import environment ────────────────────────────────────────────────────────

/// Register host import callbacks into a `Linker<()>` under the `"env"` namespace.
///
/// `user_callbacks` keys are **camelCase** WIT import names (e.g. `"envMul"`).
/// Must be called before instantiation; memory is resolved from the caller at call time.
pub fn link_import_env(
    linker: &mut Linker<()>,
    import_funcs: &[WitFunc],
    user_callbacks: &HashMap<String, HostCallback>,
) -> Result<()> {
    for func in import_funcs {
        // WASM binary import name is snake_case (kebab → underscore).
        let wasm_key = func.name.replace('-', "_");
        let params   = func.params.clone();
        let result   = func.result.clone();
        // User-facing key is camelCase (e.g. "envMul").
        let cb = user_callbacks.get(&func.camel_name).cloned();

        let param_types  = wit_to_wasm_params(&params);
        let result_types = wit_to_wasm_results(&result);
        let func_type = FuncType::new(linker.engine(), param_types, result_types);

        linker.func_new("env", &wasm_key, func_type, move |mut caller: Caller<'_, ()>, raw: &[wasmtime::Val], results: &mut [wasmtime::Val]| {
            let mut wasm_vals: Vec<WasmVal> = Vec::new();
            let mut i = 0usize;
            for p in &params {
                match &p.ty {
                    WitType::Str => {
                        let ptr = raw[i].unwrap_i32() as u32;
                        let len = raw[i + 1].unwrap_i32() as u32;
                        i += 2;
                        let mem = caller.get_export("memory")
                            .and_then(|e| if let Extern::Memory(m) = e { Some(m) } else { None })
                            .ok_or_else(|| anyhow!("no memory export"))?;
                        let data = mem.data(&caller);
                        let s = read_str(data, ptr, len)?;
                        wasm_vals.push(WasmVal::Str(s));
                    }
                    WitType::Bool => {
                        wasm_vals.push(WasmVal::Bool(raw[i].unwrap_i32() != 0));
                        i += 1;
                    }
                    WitType::S32 => { wasm_vals.push(WasmVal::I32(raw[i].unwrap_i32())); i += 1; }
                    WitType::S64 => { wasm_vals.push(WasmVal::I64(raw[i].unwrap_i64())); i += 1; }
                    WitType::F32 => { wasm_vals.push(WasmVal::F32(raw[i].unwrap_f32())); i += 1; }
                    WitType::F64 => { wasm_vals.push(WasmVal::F64(raw[i].unwrap_f64())); i += 1; }
                }
            }

            let ret = if let Some(ref cb) = cb {
                cb(&wasm_vals)?
            } else {
                default_wasm_val(&result)
            };

            if !results.is_empty() {
                results[0] = encode_result_val(&result, &ret);
            }
            Ok(())
        })?;
    }
    Ok(())
}

fn default_wasm_val(ty: &Option<WitType>) -> WasmVal {
    match ty {
        Some(WitType::F32)  => WasmVal::F32(0.0),
        Some(WitType::F64)  => WasmVal::F64(0.0),
        Some(WitType::S64)  => WasmVal::I64(0),
        Some(WitType::Bool) => WasmVal::Bool(false),
        Some(WitType::Str)  => WasmVal::Str(String::new()),
        _                   => WasmVal::I32(0),
    }
}

fn encode_result_val(ty: &Option<WitType>, val: &WasmVal) -> wasmtime::Val {
    match (ty, val) {
        (Some(WitType::Bool), WasmVal::Bool(b)) => wasmtime::Val::I32(if *b { 1 } else { 0 }),
        (Some(WitType::Bool), WasmVal::I32(v))  => wasmtime::Val::I32(*v),
        (Some(WitType::F32),  WasmVal::F32(v))  => wasmtime::Val::F32(v.to_bits()),
        (Some(WitType::F64),  WasmVal::F64(v))  => wasmtime::Val::F64(v.to_bits()),
        (Some(WitType::S64),  WasmVal::I64(v))  => wasmtime::Val::I64(*v),
        (_,                   WasmVal::I32(v))  => wasmtime::Val::I32(*v),
        (_,                   WasmVal::F64(v))  => wasmtime::Val::F64(v.to_bits()),
        _                                       => wasmtime::Val::I32(0),
    }
}

// ── wasic export call ─────────────────────────────────────────────────────────

pub fn call_wasic_export(
    store: &mut Store<()>,
    instance: &wasmtime::Instance,
    func: &WitFunc,
    args: &[WasmVal],
) -> Result<WasmVal> {
    let mem = instance.get_memory(&mut *store, "memory")
        .ok_or_else(|| anyhow!("no memory export"))?;

    let wasm_fn = instance.get_func(&mut *store, &func.camel_name)
        .ok_or_else(|| anyhow!("export '{}' not found", func.camel_name))?;

    let mut wasm_args: Vec<wasmtime::Val> = Vec::new();

    for (param, arg) in func.params.iter().zip(args.iter()) {
        match &param.ty {
            WitType::Str => {
                let s = arg.as_str().ok_or_else(|| anyhow!("expected string"))?;
                let bytes = s.as_bytes().to_vec();
                let malloc = instance.get_typed_func::<i32, i32>(&mut *store, "__malloc")
                    .map_err(|_| anyhow!("__malloc not exported"))?;
                let ptr = malloc.call(&mut *store, bytes.len() as i32)?;
                mem.data_mut(&mut *store)[ptr as usize..ptr as usize + bytes.len()]
                    .copy_from_slice(&bytes);
                wasm_args.push(wasmtime::Val::I32(ptr));
                wasm_args.push(wasmtime::Val::I32(bytes.len() as i32));
            }
            WitType::Bool => wasm_args.push(wasmtime::Val::I32(if arg.as_bool().unwrap_or(false) { 1 } else { 0 })),
            WitType::S32  => wasm_args.push(wasmtime::Val::I32(arg.as_i32().ok_or_else(|| anyhow!("expected i32"))?)),
            WitType::S64  => wasm_args.push(wasmtime::Val::I64(arg.as_i64().ok_or_else(|| anyhow!("expected i64"))?)),
            WitType::F32  => wasm_args.push(wasmtime::Val::F32(arg.as_f32().ok_or_else(|| anyhow!("expected f32"))?.to_bits())),
            WitType::F64  => wasm_args.push(wasmtime::Val::F64(arg.as_f64().ok_or_else(|| anyhow!("expected f64"))?.to_bits())),
        }
    }

    if func.result == Some(WitType::Str) {
        wasm_fn.call(&mut *store, &wasm_args, &mut [])?;
        let ptr = instance.get_global(&mut *store, "__str_ret_ptr")
            .ok_or_else(|| anyhow!("__str_ret_ptr not found"))?
            .get(&mut *store).unwrap_i32() as u32;
        let len = instance.get_global(&mut *store, "__str_ret_len")
            .ok_or_else(|| anyhow!("__str_ret_len not found"))?
            .get(&mut *store).unwrap_i32() as u32;
        let data = mem.data(&*store);
        return Ok(WasmVal::Str(read_str(data, ptr, len)?));
    }

    let n = if func.result.is_some() { 1 } else { 0 };
    let mut results = vec![wasmtime::Val::I32(0); n];
    wasm_fn.call(&mut *store, &wasm_args, &mut results)?;
    decode_result(&func.result, &results)
}

// ── component export call (Canonical ABI) ─────────────────────────────────────

pub fn call_component_export(
    store: &mut Store<()>,
    instance: &wasmtime::Instance,
    func: &WitFunc,
    args: &[WasmVal],
) -> Result<WasmVal> {
    let mem = instance.get_memory(&mut *store, "memory")
        .ok_or_else(|| anyhow!("no memory export"))?;

    let cabi = instance
        .get_typed_func::<(i32, i32, i32, i32), i32>(&mut *store, "cabi_realloc")
        .map_err(|_| anyhow!("cabi_realloc not exported"))?;

    let wasm_fn = instance.get_func(&mut *store, &func.camel_name)
        .ok_or_else(|| anyhow!("export '{}' not found", func.camel_name))?;

    let mut wasm_args: Vec<wasmtime::Val> = Vec::new();

    for (param, arg) in func.params.iter().zip(args.iter()) {
        match &param.ty {
            WitType::Str => {
                let s = arg.as_str().ok_or_else(|| anyhow!("expected string"))?;
                let bytes = s.as_bytes().to_vec();
                let ptr = cabi.call(&mut *store, (0, 0, 1, bytes.len() as i32))?;
                mem.data_mut(&mut *store)[ptr as usize..ptr as usize + bytes.len()]
                    .copy_from_slice(&bytes);
                wasm_args.push(wasmtime::Val::I32(ptr));
                wasm_args.push(wasmtime::Val::I32(bytes.len() as i32));
            }
            WitType::Bool => wasm_args.push(wasmtime::Val::I32(if arg.as_bool().unwrap_or(false) { 1 } else { 0 })),
            WitType::S32  => wasm_args.push(wasmtime::Val::I32(arg.as_i32().ok_or_else(|| anyhow!("expected i32"))?)),
            WitType::S64  => wasm_args.push(wasmtime::Val::I64(arg.as_i64().ok_or_else(|| anyhow!("expected i64"))?)),
            WitType::F32  => wasm_args.push(wasmtime::Val::F32(arg.as_f32().ok_or_else(|| anyhow!("expected f32"))?.to_bits())),
            WitType::F64  => wasm_args.push(wasmtime::Val::F64(arg.as_f64().ok_or_else(|| anyhow!("expected f64"))?.to_bits())),
        }
    }

    if func.result == Some(WitType::Str) {
        let ret_buf = cabi.call(&mut *store, (0, 0, 4, 8))?;
        wasm_args.push(wasmtime::Val::I32(ret_buf));
        wasm_fn.call(&mut *store, &wasm_args, &mut [])?;
        let data = mem.data(&*store);
        let b = ret_buf as usize;
        let ret_ptr = i32::from_le_bytes(data[b..b+4].try_into()?) as u32;
        let ret_len = i32::from_le_bytes(data[b+4..b+8].try_into()?) as u32;
        return Ok(WasmVal::Str(read_str(data, ret_ptr, ret_len)?));
    }

    let n = if func.result.is_some() { 1 } else { 0 };
    let mut results = vec![wasmtime::Val::I32(0); n];
    wasm_fn.call(&mut *store, &wasm_args, &mut results)?;
    decode_result(&func.result, &results)
}

// ── raw export call (no WIT / no ABI translation) ─────────────────────────────

/// Call a WASM export directly by name with no ABI translation.
/// Used when no companion `.wit` file is found.
pub fn call_raw_export(
    store: &mut Store<()>,
    instance: &wasmtime::Instance,
    name: &str,
    args: &[WasmVal],
) -> Result<WasmVal> {
    let wasm_fn = instance.get_func(&mut *store, name)
        .ok_or_else(|| anyhow!("raw export '{name}' not found"))?;

    let mut wasm_args: Vec<wasmtime::Val> = Vec::new();
    for v in args {
        wasm_args.push(match v {
            WasmVal::I32(x)  => wasmtime::Val::I32(*x),
            WasmVal::I64(x)  => wasmtime::Val::I64(*x),
            WasmVal::F32(x)  => wasmtime::Val::F32(x.to_bits()),
            WasmVal::F64(x)  => wasmtime::Val::F64(x.to_bits()),
            WasmVal::Bool(b) => wasmtime::Val::I32(if *b { 1 } else { 0 }),
            _ => return Err(anyhow!("strings require a companion .wit file")),
        });
    }

    let n = wasm_fn.ty(&*store).results().len();
    let mut results = vec![wasmtime::Val::I32(0); n];
    wasm_fn.call(&mut *store, &wasm_args, &mut results)?;

    if n == 0 { return Ok(WasmVal::Void); }
    Ok(match results[0] {
        wasmtime::Val::I32(v)    => WasmVal::I32(v),
        wasmtime::Val::I64(v)    => WasmVal::I64(v),
        wasmtime::Val::F32(bits) => WasmVal::F32(f32::from_bits(bits)),
        wasmtime::Val::F64(bits) => WasmVal::F64(f64::from_bits(bits)),
        _                        => WasmVal::Void,
    })
}

fn decode_result(result: &Option<WitType>, results: &[wasmtime::Val]) -> Result<WasmVal> {
    Ok(match result {
        None                 => WasmVal::Void,
        Some(WitType::S32)   => WasmVal::I32(results[0].unwrap_i32()),
        Some(WitType::S64)   => WasmVal::I64(results[0].unwrap_i64()),
        Some(WitType::F32)   => WasmVal::F32(results[0].unwrap_f32()),
        Some(WitType::F64)   => WasmVal::F64(results[0].unwrap_f64()),
        Some(WitType::Bool)  => WasmVal::Bool(results[0].unwrap_i32() != 0),
        Some(WitType::Str)   => unreachable!("string returns handled above"),
    })
}
