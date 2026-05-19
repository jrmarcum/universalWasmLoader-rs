/// Parsed WIT type.
#[derive(Debug, Clone, PartialEq)]
pub enum WitType {
    S32,
    S64,
    F32,
    F64,
    Bool,
    Str,
}

impl WitType {
    fn from_str(s: &str) -> Self {
        match s.trim() {
            "s32" => Self::S32,
            "s64" => Self::S64,
            "f32" => Self::F32,
            "f64" => Self::F64,
            "bool" => Self::Bool,
            "string" => Self::Str,
            _ => Self::S32,
        }
    }
}

/// A single parameter in a WIT function signature.
#[derive(Debug, Clone)]
pub struct WitParam {
    /// snake_case name for use in Rust.
    pub name: String,
    pub ty: WitType,
}

/// A parsed WIT function (import or export).
#[derive(Debug, Clone)]
pub struct WitFunc {
    /// Original kebab-case name from WIT.
    pub name: String,
    /// camelCase name — this is the actual name used in the WASM binary for exports.
    pub camel_name: String,
    /// snake_case name for host language (Rust/Python) — used in the public API.
    pub snake_name: String,
    pub params: Vec<WitParam>,
    pub result: Option<WitType>,
}

/// Fully parsed WIT world.
#[derive(Debug, Clone)]
pub struct ParsedWit {
    pub package_name: String,
    pub world_name: String,
    pub imports: Vec<WitFunc>,
    pub exports: Vec<WitFunc>,
}

/// Convert kebab-case to snake_case.
pub fn kebab_to_snake(s: &str) -> String {
    s.replace('-', "_")
}

/// Convert kebab-case to camelCase (matches JS `kebabToCamel`).
pub fn kebab_to_camel(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut capitalize_next = false;
    for ch in s.chars() {
        if ch == '-' {
            capitalize_next = true;
        } else if capitalize_next {
            result.extend(ch.to_uppercase());
            capitalize_next = false;
        } else {
            result.push(ch);
        }
    }
    result
}

fn parse_params(raw: &str) -> Vec<WitParam> {
    let raw = raw.trim();
    if raw.is_empty() {
        return vec![];
    }
    raw.split(',')
        .filter_map(|part| {
            let colon = part.find(':')?;
            let raw_name = part[..colon].trim();
            let ty = WitType::from_str(&part[colon + 1..]);
            Some(WitParam { name: kebab_to_snake(raw_name), ty })
        })
        .collect()
}

fn parse_funcs(body: &str, keyword: &str) -> Vec<WitFunc> {
    let pattern = format!(
        r"\b{}\s+([\w-]+)\s*:\s*func\s*\(([^)]*)\)(?:\s*->\s*([\w-]+))?\s*;",
        keyword
    );
    let re = regex_lite::Regex::new(&pattern).expect("valid regex");
    re.captures_iter(body)
        .map(|cap| {
            let name = cap[1].to_string();
            let camel_name = kebab_to_camel(&name);
            let snake_name = kebab_to_snake(&name);
            let params = parse_params(&cap[2]);
            let result = cap.get(3).map(|m| WitType::from_str(m.as_str()));
            WitFunc { name, camel_name, snake_name, params, result }
        })
        .collect()
}

/// Parse a WIT source string in the format emitted by wasmtk.
pub fn parse_wit(src: &str) -> ParsedWit {
    let pkg_re = regex_lite::Regex::new(r"package\s+([\w:/-]+)\s*;").unwrap();
    let package_name = pkg_re
        .captures(src)
        .map(|c| c[1].to_string())
        .unwrap_or_default();

    let world_re = regex_lite::Regex::new(r"world\s+([\w-]+)\s*\{([\s\S]*)\}").unwrap();
    let (world_name, world_body) = world_re
        .captures(src)
        .map(|c| (c[1].to_string(), c[2].to_string()))
        .unwrap_or_default();

    ParsedWit {
        package_name,
        world_name,
        imports: parse_funcs(&world_body, "import"),
        exports: parse_funcs(&world_body, "export"),
    }
}
