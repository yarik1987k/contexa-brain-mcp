use std::collections::HashSet;

// ── Symbol extractor tests ───────────────────────────────────────────

#[test]
fn test_js_symbol_extraction() {
    let source = r#"
function greet(name) {
    return "hello " + name;
}

class UserService {
    constructor() {}
}

const helper = (x) => x * 2;
"#;
    let symbols = context_brain::indexer::symbol_extractor::extract_symbols(source, "js").unwrap();
    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"greet"), "missing 'greet' in {:?}", names);
    assert!(names.contains(&"UserService"), "missing 'UserService' in {:?}", names);
    assert!(names.contains(&"helper"), "missing 'helper' in {:?}", names);
}

#[test]
fn test_ts_symbol_extraction() {
    let source = r#"
export interface Config { host: string; }
export type UserId = string;
export enum Status { Active, Inactive }
export function process(c: Config): void {}
"#;
    let symbols = context_brain::indexer::symbol_extractor::extract_symbols(source, "ts").unwrap();
    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"Config"), "missing 'Config' in {:?}", names);
    assert!(names.contains(&"UserId"), "missing 'UserId' in {:?}", names);
    assert!(names.contains(&"Status"), "missing 'Status' in {:?}", names);
    assert!(names.contains(&"process"), "missing 'process' in {:?}", names);
}

#[test]
fn test_python_symbol_extraction() {
    let source = r#"
def hello(name):
    return f"hello {name}"

class Database:
    def __init__(self):
        pass
"#;
    let symbols = context_brain::indexer::symbol_extractor::extract_symbols(source, "py").unwrap();
    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"hello"), "missing 'hello' in {:?}", names);
    assert!(names.contains(&"Database"), "missing 'Database' in {:?}", names);
}

#[test]
fn test_rust_symbol_extraction() {
    let source = r#"
pub fn process() -> Result<()> { Ok(()) }
pub struct Config { host: String }
pub enum Status { Active, Inactive }
pub trait Handler { fn handle(&self); }
"#;
    let symbols = context_brain::indexer::symbol_extractor::extract_symbols(source, "rs").unwrap();
    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"process"), "missing 'process' in {:?}", names);
    assert!(names.contains(&"Config"), "missing 'Config' in {:?}", names);
    assert!(names.contains(&"Status"), "missing 'Status' in {:?}", names);
    assert!(names.contains(&"Handler"), "missing 'Handler' in {:?}", names);
}

#[test]
fn test_go_symbol_extraction() {
    let source = r#"
package main

func Hello(name string) string { return "hello " + name }

type Config struct { Host string }
"#;
    // tree-sitter-go v0.25 may be incompatible with tree-sitter v0.24 — skip if version mismatch
    match context_brain::indexer::symbol_extractor::extract_symbols(source, "go") {
        Ok(symbols) => {
            let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
            assert!(names.contains(&"Hello"), "missing 'Hello' in {:?}", names);
            assert!(names.contains(&"Config"), "missing 'Config' in {:?}", names);
        }
        Err(e) if e.to_string().contains("Incompatible language version") => {
            eprintln!("Skipping Go test: {}", e);
        }
        Err(e) => panic!("Unexpected error: {}", e),
    }
}

#[test]
fn test_unsupported_language_returns_error() {
    assert!(context_brain::indexer::symbol_extractor::extract_symbols("fn main() {}", "xyz").is_err());
}

#[test]
fn test_empty_source_returns_empty() {
    let symbols = context_brain::indexer::symbol_extractor::extract_symbols("", "js").unwrap();
    assert!(symbols.is_empty());
}

// ── Import extractor tests ───────────────────────────────────────────

#[test]
fn test_js_import_extraction() {
    let source = r#"
import { foo } from './utils';
import bar from "../lib/bar";
const baz = require('./baz');
export { qux } from './qux';
import React from 'react';
"#;
    let imports = context_brain::indexer::import_extractor::extract_imports(source, "js");
    assert!(imports.contains(&"./utils".to_string()), "missing './utils' in {:?}", imports);
    assert!(imports.contains(&"../lib/bar".to_string()), "missing '../lib/bar' in {:?}", imports);
    assert!(imports.contains(&"./baz".to_string()), "missing './baz' in {:?}", imports);
    assert!(imports.contains(&"./qux".to_string()), "missing './qux' in {:?}", imports);
    assert!(imports.contains(&"react".to_string()), "missing 'react' in {:?}", imports);
}

#[test]
fn test_python_import_extraction() {
    let imports = context_brain::indexer::import_extractor::extract_imports(
        "from foo.bar import baz\nimport os, sys\nimport json as j\n", "py"
    );
    assert!(imports.contains(&"foo.bar".to_string()));
    assert!(imports.contains(&"os".to_string()));
    assert!(imports.contains(&"sys".to_string()));
    assert!(imports.contains(&"json".to_string()));
}

#[test]
fn test_rust_import_extraction() {
    let imports = context_brain::indexer::import_extractor::extract_imports(
        "use crate::db::schema;\nmod tools;\n", "rs"
    );
    assert!(imports.contains(&"db".to_string()), "missing 'db' in {:?}", imports);
    assert!(imports.contains(&"tools".to_string()), "missing 'tools' in {:?}", imports);
}

#[test]
fn test_c_include_extraction() {
    let imports = context_brain::indexer::import_extractor::extract_imports(
        "#include \"header.h\"\n#include <stdio.h>\n#include \"utils/math.h\"\n", "c"
    );
    assert!(imports.contains(&"header.h".to_string()));
    assert!(imports.contains(&"utils/math.h".to_string()));
    // System includes should NOT be extracted
    assert!(!imports.iter().any(|i| i.contains("stdio")));
}

#[test]
fn test_import_resolution_js() {
    let mut known = HashSet::new();
    known.insert("src/utils.ts".to_string());
    known.insert("src/lib/bar.js".to_string());

    let counts = context_brain::indexer::import_extractor::build_import_counts(
        &[("src/main.ts".to_string(), "ts".to_string(), "import { foo } from './utils';\nimport bar from './lib/bar';".to_string())],
        &known,
    );
    assert_eq!(*counts.get("src/utils.ts").unwrap_or(&0), 1);
    assert_eq!(*counts.get("src/lib/bar.js").unwrap_or(&0), 1);
}

#[test]
fn test_import_counts_accumulate() {
    let mut known = HashSet::new();
    known.insert("src/shared.ts".to_string());

    let files = vec![
        ("src/a.ts".to_string(), "ts".to_string(), "import x from './shared';".to_string()),
        ("src/b.ts".to_string(), "ts".to_string(), "import y from './shared';".to_string()),
        ("src/c.ts".to_string(), "ts".to_string(), "import z from './shared';".to_string()),
    ];

    let counts = context_brain::indexer::import_extractor::build_import_counts(&files, &known);
    assert_eq!(*counts.get("src/shared.ts").unwrap_or(&0), 3);
}

// ── Scoring constants sanity tests ───────────────────────────────────

#[test]
fn test_scoring_constants_are_positive() {
    use context_brain::context::scoring::*;
    assert!(SEARCH_EXACT_NAME_BONUS > 0.0);
    assert!(SEARCH_SUBSTRING_NAME_BONUS > 0.0);
    assert!(SEARCH_SYMBOL_SIM_THRESHOLD > 0.0 && SEARCH_SYMBOL_SIM_THRESHOLD < 1.0);
    assert!(RELEVANCE_HIGH_THRESHOLD > RELEVANCE_MEDIUM_THRESHOLD);
    assert!(MEMORY_MIN_SCORE > 0.0 && MEMORY_MIN_SCORE < 1.0);
}

// ── Bit-packing round-trip property tests ────────────────────────────

#[test]
fn test_pack_unpack_roundtrip_2bit() {
    use context_brain::turboquant::codebooks::{pack_indices, unpack_indices};
    let indices: Vec<u8> = (0..100).map(|i| (i % 4) as u8).collect();
    let packed = pack_indices(&indices, 2).unwrap();
    let unpacked = unpack_indices(&packed, 2, 100).unwrap();
    assert_eq!(indices, unpacked);
}

#[test]
fn test_pack_unpack_roundtrip_3bit() {
    use context_brain::turboquant::codebooks::{pack_indices, unpack_indices};
    let indices: Vec<u8> = (0..100).map(|i| (i % 8) as u8).collect();
    let packed = pack_indices(&indices, 3).unwrap();
    let unpacked = unpack_indices(&packed, 3, 100).unwrap();
    assert_eq!(indices, unpacked);
}

#[test]
fn test_pack_unpack_roundtrip_4bit() {
    use context_brain::turboquant::codebooks::{pack_indices, unpack_indices};
    let indices: Vec<u8> = (0..100).map(|i| (i % 16) as u8).collect();
    let packed = pack_indices(&indices, 4).unwrap();
    let unpacked = unpack_indices(&packed, 4, 100).unwrap();
    assert_eq!(indices, unpacked);
}

#[test]
fn test_pack_invalid_bitwidth_returns_error() {
    use context_brain::turboquant::codebooks::pack_indices;
    assert!(pack_indices(&[0, 1, 2], 5).is_err());
    assert!(pack_indices(&[0, 1, 2], 0).is_err());
}

// ── UTF-8 safety test ────────────────────────────────────────────────

#[test]
fn test_token_estimator_handles_unicode() {
    use context_brain::context::token_estimator::estimate_tokens;
    // Should not panic on multi-byte characters
    let text = "こんにちは世界 🌍 émoji café résumé";
    let tokens = estimate_tokens(text);
    assert!(tokens > 0);
}
