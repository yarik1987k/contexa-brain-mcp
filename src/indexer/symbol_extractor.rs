use std::cell::RefCell;
use std::collections::HashMap;

use anyhow::{Result, bail};
use tree_sitter::{Language, Parser, Node};

// Thread-local parser cache: one parser per language, reused across calls.
thread_local! {
    static PARSER_CACHE: RefCell<HashMap<&'static str, Parser>> = RefCell::new(HashMap::new());
}

/// A symbol extracted from source code via AST parsing.
#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub start_line: usize,
    pub end_line: usize,
    pub signature: String,
    pub code: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SymbolKind {
    Function,
    AsyncFunction,
    Class,
    Method,
    Interface,
    TypeAlias,
    Enum,
    Struct,
    Trait,
    Impl,
    Export,
    Constant,
}

impl std::fmt::Display for SymbolKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SymbolKind::Function => write!(f, "function"),
            SymbolKind::AsyncFunction => write!(f, "async function"),
            SymbolKind::Class => write!(f, "class"),
            SymbolKind::Method => write!(f, "method"),
            SymbolKind::Interface => write!(f, "interface"),
            SymbolKind::TypeAlias => write!(f, "type"),
            SymbolKind::Enum => write!(f, "enum"),
            SymbolKind::Struct => write!(f, "struct"),
            SymbolKind::Trait => write!(f, "trait"),
            SymbolKind::Impl => write!(f, "impl"),
            SymbolKind::Export => write!(f, "export"),
            SymbolKind::Constant => write!(f, "const"),
        }
    }
}

// ── Data-driven extraction rules ─────────────────────────────────────

/// A rule mapping a tree-sitter node kind to a symbol kind and name field.
struct ExtractionRule {
    node_kind: &'static str,
    symbol_kind: SymbolKind,
    name_field: &'static str,
}

/// Rules for simple "match node kind → extract name field" patterns.
const JS_RULES: &[ExtractionRule] = &[
    ExtractionRule { node_kind: "function_declaration", symbol_kind: SymbolKind::Function, name_field: "name" },
    ExtractionRule { node_kind: "class_declaration", symbol_kind: SymbolKind::Class, name_field: "name" },
];

const TS_EXTRA_RULES: &[ExtractionRule] = &[
    ExtractionRule { node_kind: "interface_declaration", symbol_kind: SymbolKind::Interface, name_field: "name" },
    ExtractionRule { node_kind: "type_alias_declaration", symbol_kind: SymbolKind::TypeAlias, name_field: "name" },
    ExtractionRule { node_kind: "enum_declaration", symbol_kind: SymbolKind::Enum, name_field: "name" },
];

const PYTHON_RULES: &[ExtractionRule] = &[
    ExtractionRule { node_kind: "function_definition", symbol_kind: SymbolKind::Function, name_field: "name" },
    ExtractionRule { node_kind: "class_definition", symbol_kind: SymbolKind::Class, name_field: "name" },
];

const RUST_RULES: &[ExtractionRule] = &[
    ExtractionRule { node_kind: "function_item", symbol_kind: SymbolKind::Function, name_field: "name" },
    ExtractionRule { node_kind: "struct_item", symbol_kind: SymbolKind::Struct, name_field: "name" },
    ExtractionRule { node_kind: "enum_item", symbol_kind: SymbolKind::Enum, name_field: "name" },
    ExtractionRule { node_kind: "trait_item", symbol_kind: SymbolKind::Trait, name_field: "name" },
    ExtractionRule { node_kind: "impl_item", symbol_kind: SymbolKind::Impl, name_field: "type" },
];

const GO_RULES: &[ExtractionRule] = &[
    ExtractionRule { node_kind: "function_declaration", symbol_kind: SymbolKind::Function, name_field: "name" },
    ExtractionRule { node_kind: "method_declaration", symbol_kind: SymbolKind::Method, name_field: "name" },
];

const C_RULES: &[ExtractionRule] = &[];

/// Apply simple extraction rules to top-level children.
fn extract_by_rules(node: &Node, source: &str, rules: &[ExtractionRule], symbols: &mut Vec<Symbol>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        for rule in rules {
            if child.kind() == rule.node_kind {
                if let Some(name_node) = child.child_by_field_name(rule.name_field) {
                    let name = node_text(&name_node, source);
                    let mut kind = rule.symbol_kind.clone();
                    // Detect async functions
                    if kind == SymbolKind::Function && source[child.byte_range()].starts_with("async") {
                        kind = SymbolKind::AsyncFunction;
                    }
                    push_symbol(symbols, name, kind, &child, source);
                }
            }
        }
    }
}

// ── Public API ───────────────────────────────────────────────────────

/// Extract symbols from source code using tree-sitter.
/// Parser is cached per language in thread-local storage to avoid re-allocation.
pub fn extract_symbols(source: &str, extension: &str) -> Result<Vec<Symbol>> {
    let language = get_language(extension)?;

    // Canonicalize extension to a stable key for the cache
    let cache_key: &'static str = match extension {
        "js" | "jsx" | "mjs" | "cjs" => "js",
        "ts" => "ts",
        "tsx" => "tsx",
        "py" | "pyi" => "py",
        "rs" => "rs",
        "go" => "go",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" | "hxx" => "cpp",
        _ => return Err(anyhow::anyhow!("Unsupported language: .{}", extension)),
    };

    let tree = PARSER_CACHE.with({
        let language = language.clone();
        move |cache| -> Result<tree_sitter::Tree> {
            let mut cache = cache.borrow_mut();
            if !cache.contains_key(cache_key) {
                let mut p = Parser::new();
                p.set_language(&language)?;
                cache.insert(cache_key, p);
            }
            let parser = cache.get_mut(cache_key).unwrap();
            parser.parse(source, None)
                .ok_or_else(|| anyhow::anyhow!("Failed to parse source"))
        }
    })?;

    let root = tree.root_node();
    let mut symbols = Vec::new();

    match extension {
        "js" | "jsx" | "mjs" | "cjs" => extract_js_symbols(&root, source, &mut symbols),
        "ts" | "tsx" => extract_ts_symbols(&root, source, &mut symbols),
        "py" | "pyi" => extract_python_symbols(&root, source, &mut symbols),
        "rs" => extract_rust_symbols(&root, source, &mut symbols),
        "go" => extract_go_symbols(&root, source, &mut symbols),
        "c" | "h" | "cpp" | "cc" | "cxx" | "hpp" | "hxx" => extract_c_symbols(&root, source, &mut symbols),
        _ => {}
    }

    Ok(symbols)
}

/// Get the tree-sitter language for a file extension.
fn get_language(extension: &str) -> Result<Language> {
    match extension {
        "js" | "jsx" | "mjs" | "cjs" => Ok(tree_sitter_javascript::LANGUAGE.into()),
        "ts" => Ok(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "tsx" => Ok(tree_sitter_typescript::LANGUAGE_TSX.into()),
        "py" | "pyi" => Ok(tree_sitter_python::LANGUAGE.into()),
        "rs" => Ok(tree_sitter_rust::LANGUAGE.into()),
        "go" => Ok(tree_sitter_go::LANGUAGE.into()),
        "c" | "h" => Ok(tree_sitter_c::LANGUAGE.into()),
        "cpp" | "cc" | "cxx" | "hpp" | "hxx" => Ok(tree_sitter_cpp::LANGUAGE.into()),
        _ => bail!("Unsupported language: .{}", extension),
    }
}

// ── JavaScript extraction ────────────────────────────────────────────

fn extract_js_symbols(node: &Node, source: &str, symbols: &mut Vec<Symbol>) {
    // Apply simple rules for function/class declarations
    extract_by_rules(node, source, JS_RULES, symbols);

    // Handle JS-specific patterns that need custom logic
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "lexical_declaration" | "variable_declaration" => {
                extract_variable_declarations(&child, source, symbols);
            }
            "export_statement" => {
                extract_export_contents(&child, source, symbols, JS_RULES);
            }
            "expression_statement" => {
                extract_commonjs_exports(&child, source, symbols);
            }
            _ => {}
        }
    }
}

/// Extract exported declarations — applies rules inside export statements.
fn extract_export_contents(node: &Node, source: &str, symbols: &mut Vec<Symbol>, rules: &[ExtractionRule]) {
    let mut cursor = node.walk();
    for inner in node.children(&mut cursor) {
        // Try rule-based extraction first
        for rule in rules {
            if inner.kind() == rule.node_kind {
                if let Some(name_node) = inner.child_by_field_name(rule.name_field) {
                    let name = node_text(&name_node, source);
                    push_symbol(symbols, name, rule.symbol_kind.clone(), node, source);
                }
            }
        }
        // Variable declarations in exports
        if inner.kind() == "lexical_declaration" || inner.kind() == "variable_declaration" {
            extract_variable_declarations(&inner, source, symbols);
        }
    }
}

/// Extract CommonJS module.exports = { ... } and exports.X = ... patterns
fn extract_commonjs_exports(node: &Node, source: &str, symbols: &mut Vec<Symbol>) {
    let text = source[node.byte_range()].to_string();

    if text.starts_with("module.exports") {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "assignment_expression" {
                if let Some(right) = child.child_by_field_name("right") {
                    if right.kind() == "object" {
                        let mut obj_cursor = right.walk();
                        let mut export_names = Vec::new();
                        for prop in right.children(&mut obj_cursor) {
                            match prop.kind() {
                                "shorthand_property_identifier" => {
                                    export_names.push(node_text(&prop, source));
                                }
                                "pair" => {
                                    if let Some(key) = prop.child_by_field_name("key") {
                                        export_names.push(node_text(&key, source));
                                    }
                                }
                                _ => {}
                            }
                        }
                        if !export_names.is_empty() {
                            let name = format!("module.exports({})", export_names.join(", "));
                            push_symbol(symbols, name, SymbolKind::Export, node, source);
                        }
                    } else {
                        let name = format!("module.exports = {}", node_text(&right, source).chars().take(60).collect::<String>());
                        push_symbol(symbols, name, SymbolKind::Export, node, source);
                    }
                }
            }
        }
    } else if text.starts_with("exports.") {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "assignment_expression" {
                if let Some(left) = child.child_by_field_name("left") {
                    if left.kind() == "member_expression" {
                        if let Some(prop) = left.child_by_field_name("property") {
                            let name = node_text(&prop, source);
                            push_symbol(symbols, name, SymbolKind::Export, node, source);
                        }
                    }
                }
            }
        }
    }
}

fn extract_variable_declarations(node: &Node, source: &str, symbols: &mut Vec<Symbol>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "variable_declarator" {
            if let Some(name_node) = child.child_by_field_name("name") {
                if let Some(value_node) = child.child_by_field_name("value") {
                    let kind = match value_node.kind() {
                        "arrow_function" | "function" => SymbolKind::Function,
                        "class" => SymbolKind::Class,
                        _ => SymbolKind::Constant,
                    };
                    let name = node_text(&name_node, source);
                    push_symbol(symbols, name, kind, node, source);
                }
            }
        }
    }
}

// ── TypeScript extraction (extends JS + TS-specific rules) ──────────

fn extract_ts_symbols(node: &Node, source: &str, symbols: &mut Vec<Symbol>) {
    // TS is a superset of JS, start with JS extraction
    extract_js_symbols(node, source, symbols);

    // Add TS-specific types using rules
    extract_by_rules(node, source, TS_EXTRA_RULES, symbols);

    // Handle TS-specific types inside export statements
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "export_statement" {
            extract_export_contents(&child, source, symbols, TS_EXTRA_RULES);
        }
    }
}

// ── Python extraction ────────────────────────────────────────────────

fn extract_python_symbols(node: &Node, source: &str, symbols: &mut Vec<Symbol>) {
    extract_by_rules(node, source, PYTHON_RULES, symbols);

    // Handle decorated definitions
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "decorated_definition" {
            let mut inner_cursor = child.walk();
            for inner in child.children(&mut inner_cursor) {
                for rule in PYTHON_RULES {
                    if inner.kind() == rule.node_kind {
                        if let Some(name_node) = inner.child_by_field_name(rule.name_field) {
                            let name = node_text(&name_node, source);
                            push_symbol(symbols, name, rule.symbol_kind.clone(), &child, source);
                        }
                    }
                }
            }
        }
    }
}

// ── Rust extraction ──────────────────────────────────────────────────

fn extract_rust_symbols(node: &Node, source: &str, symbols: &mut Vec<Symbol>) {
    extract_by_rules(node, source, RUST_RULES, symbols);
}

// ── Go extraction ────────────────────────────────────────────────────

fn extract_go_symbols(node: &Node, source: &str, symbols: &mut Vec<Symbol>) {
    extract_by_rules(node, source, GO_RULES, symbols);

    // Go-specific: type declarations and const/var blocks
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "type_declaration" => {
                let mut inner_cursor = child.walk();
                for spec in child.children(&mut inner_cursor) {
                    if spec.kind() == "type_spec" {
                        if let Some(name_node) = spec.child_by_field_name("name") {
                            let name = node_text(&name_node, source);
                            let kind = if let Some(type_node) = spec.child_by_field_name("type") {
                                match type_node.kind() {
                                    "struct_type" => SymbolKind::Struct,
                                    "interface_type" => SymbolKind::Interface,
                                    _ => SymbolKind::TypeAlias,
                                }
                            } else {
                                SymbolKind::TypeAlias
                            };
                            push_symbol(symbols, name, kind, &spec, source);
                        }
                    }
                }
            }
            "const_declaration" | "var_declaration" => {
                let mut inner_cursor = child.walk();
                for spec in child.children(&mut inner_cursor) {
                    if spec.kind() == "const_spec" || spec.kind() == "var_spec" {
                        if let Some(name_node) = spec.child_by_field_name("name") {
                            let name = node_text(&name_node, source);
                            push_symbol(symbols, name, SymbolKind::Constant, &spec, source);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

// ── C/C++ extraction ─────────────────────────────────────────────────

fn extract_c_symbols(node: &Node, source: &str, symbols: &mut Vec<Symbol>) {
    extract_by_rules(node, source, C_RULES, symbols);

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_definition" => {
                if let Some(declarator) = child.child_by_field_name("declarator") {
                    if let Some(name) = find_c_function_name(&declarator, source) {
                        push_symbol(symbols, name, SymbolKind::Function, &child, source);
                    }
                }
            }
            "declaration" => {
                if let Some(type_node) = child.child_by_field_name("type") {
                    match type_node.kind() {
                        "struct_specifier" | "union_specifier" => {
                            if let Some(name_node) = type_node.child_by_field_name("name") {
                                let name = node_text(&name_node, source);
                                push_symbol(symbols, name, SymbolKind::Struct, &child, source);
                            }
                        }
                        "enum_specifier" => {
                            if let Some(name_node) = type_node.child_by_field_name("name") {
                                let name = node_text(&name_node, source);
                                push_symbol(symbols, name, SymbolKind::Enum, &child, source);
                            }
                        }
                        _ => {}
                    }
                }
            }
            "struct_specifier" | "union_specifier" => {
                if let Some(name_node) = child.child_by_field_name("name") {
                    let name = node_text(&name_node, source);
                    push_symbol(symbols, name, SymbolKind::Struct, &child, source);
                }
            }
            "enum_specifier" => {
                if let Some(name_node) = child.child_by_field_name("name") {
                    let name = node_text(&name_node, source);
                    push_symbol(symbols, name, SymbolKind::Enum, &child, source);
                }
            }
            _ => {}
        }
    }
}

/// Recursively find the identifier inside a C declarator node.
fn find_c_function_name(node: &Node, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" => Some(node_text(node, source)),
        "function_declarator" | "pointer_declarator" | "parenthesized_declarator" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if let Some(name) = find_c_function_name(&child, source) {
                    return Some(name);
                }
            }
            None
        }
        _ => None,
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

fn node_text(node: &Node, source: &str) -> String {
    source[node.byte_range()].to_string()
}

fn push_symbol(symbols: &mut Vec<Symbol>, name: String, kind: SymbolKind, node: &Node, source: &str) {
    let start_line = node.start_position().row + 1;
    let end_line = node.end_position().row + 1;
    let code = source[node.byte_range()].to_string();
    let signature = code.lines().next().unwrap_or("").to_string();

    symbols.push(Symbol {
        name,
        kind,
        start_line,
        end_line,
        signature,
        code,
    });
}
