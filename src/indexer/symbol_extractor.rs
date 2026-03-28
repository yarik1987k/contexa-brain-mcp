use anyhow::{Result, bail};
use tree_sitter::{Language, Parser, Node};

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

/// Extract symbols from source code using tree-sitter.
pub fn extract_symbols(source: &str, extension: &str) -> Result<Vec<Symbol>> {
    let language = get_language(extension)?;

    let mut parser = Parser::new();
    parser.set_language(&language)?;

    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow::anyhow!("Failed to parse source"))?;

    let root = tree.root_node();
    let mut symbols = Vec::new();

    match extension {
        "js" | "jsx" | "mjs" | "cjs" => extract_js_symbols(&root, source, &mut symbols),
        "ts" | "tsx" => extract_ts_symbols(&root, source, &mut symbols),
        "py" | "pyi" => extract_python_symbols(&root, source, &mut symbols),
        "rs" => extract_rust_symbols(&root, source, &mut symbols),
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
        _ => bail!("Unsupported language: .{}", extension),
    }
}

// ── JavaScript extraction ─────────────────────────────────────────────

fn extract_js_symbols(node: &Node, source: &str, symbols: &mut Vec<Symbol>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_declaration" => {
                if let Some(name_node) = child.child_by_field_name("name") {
                    let name = node_text(&name_node, source);
                    push_symbol(symbols, name, SymbolKind::Function, &child, source);
                }
            }
            "class_declaration" => {
                if let Some(name_node) = child.child_by_field_name("name") {
                    let name = node_text(&name_node, source);
                    push_symbol(symbols, name, SymbolKind::Class, &child, source);
                }
            }
            "lexical_declaration" | "variable_declaration" => {
                extract_variable_declarations(&child, source, symbols);
            }
            "export_statement" => {
                // Recurse into exported declarations
                let mut inner_cursor = child.walk();
                for inner in child.children(&mut inner_cursor) {
                    match inner.kind() {
                        "function_declaration" => {
                            if let Some(name_node) = inner.child_by_field_name("name") {
                                let name = node_text(&name_node, source);
                                push_symbol(symbols, name, SymbolKind::Function, &child, source);
                            }
                        }
                        "class_declaration" => {
                            if let Some(name_node) = inner.child_by_field_name("name") {
                                let name = node_text(&name_node, source);
                                push_symbol(symbols, name, SymbolKind::Class, &child, source);
                            }
                        }
                        "lexical_declaration" | "variable_declaration" => {
                            extract_variable_declarations(&inner, source, symbols);
                        }
                        _ => {}
                    }
                }
            }
            // CommonJS: module.exports = { ... } or exports.X = ...
            "expression_statement" => {
                extract_commonjs_exports(&child, source, symbols);
            }
            _ => {}
        }
    }
}

/// Extract CommonJS module.exports = { ... } and exports.X = ... patterns
fn extract_commonjs_exports(node: &Node, source: &str, symbols: &mut Vec<Symbol>) {
    let text = source[node.byte_range()].to_string();

    // module.exports = { key1, key2, ... }
    if text.starts_with("module.exports") {
        // Find the assignment_expression
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "assignment_expression" {
                if let Some(right) = child.child_by_field_name("right") {
                    if right.kind() == "object" {
                        // Extract each property name from the exports object
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
                        // module.exports = someFunction or module.exports = class ...
                        let name = format!("module.exports = {}", node_text(&right, source).chars().take(60).collect::<String>());
                        push_symbol(symbols, name, SymbolKind::Export, node, source);
                    }
                }
            }
        }
    }
    // exports.X = ...
    else if text.starts_with("exports.") {
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
                    // For functions/classes use the parent node span, for constants just the declarator
                    push_symbol(symbols, name, kind, node, source);
                }
            }
        }
    }
}

// ── TypeScript extraction (extends JS) ────────────────────────────────

fn extract_ts_symbols(node: &Node, source: &str, symbols: &mut Vec<Symbol>) {
    // TS is a superset of JS, start with JS extraction
    extract_js_symbols(node, source, symbols);

    // Add TS-specific: interfaces, type aliases, enums
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "interface_declaration" => {
                if let Some(name_node) = child.child_by_field_name("name") {
                    let name = node_text(&name_node, source);
                    push_symbol(symbols, name, SymbolKind::Interface, &child, source);
                }
            }
            "type_alias_declaration" => {
                if let Some(name_node) = child.child_by_field_name("name") {
                    let name = node_text(&name_node, source);
                    push_symbol(symbols, name, SymbolKind::TypeAlias, &child, source);
                }
            }
            "enum_declaration" => {
                if let Some(name_node) = child.child_by_field_name("name") {
                    let name = node_text(&name_node, source);
                    push_symbol(symbols, name, SymbolKind::Enum, &child, source);
                }
            }
            "export_statement" => {
                let mut inner_cursor = child.walk();
                for inner in child.children(&mut inner_cursor) {
                    match inner.kind() {
                        "interface_declaration" => {
                            if let Some(name_node) = inner.child_by_field_name("name") {
                                let name = node_text(&name_node, source);
                                push_symbol(symbols, name, SymbolKind::Interface, &child, source);
                            }
                        }
                        "type_alias_declaration" => {
                            if let Some(name_node) = inner.child_by_field_name("name") {
                                let name = node_text(&name_node, source);
                                push_symbol(symbols, name, SymbolKind::TypeAlias, &child, source);
                            }
                        }
                        "enum_declaration" => {
                            if let Some(name_node) = inner.child_by_field_name("name") {
                                let name = node_text(&name_node, source);
                                push_symbol(symbols, name, SymbolKind::Enum, &child, source);
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}

// ── Python extraction ─────────────────────────────────────────────────

fn extract_python_symbols(node: &Node, source: &str, symbols: &mut Vec<Symbol>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_definition" => {
                if let Some(name_node) = child.child_by_field_name("name") {
                    let name = node_text(&name_node, source);
                    let kind = if source[child.byte_range()].starts_with("async") {
                        SymbolKind::AsyncFunction
                    } else {
                        SymbolKind::Function
                    };
                    push_symbol(symbols, name, kind, &child, source);
                }
            }
            "class_definition" => {
                if let Some(name_node) = child.child_by_field_name("name") {
                    let name = node_text(&name_node, source);
                    push_symbol(symbols, name, SymbolKind::Class, &child, source);
                }
            }
            "decorated_definition" => {
                // Look inside for the actual definition
                let mut inner_cursor = child.walk();
                for inner in child.children(&mut inner_cursor) {
                    match inner.kind() {
                        "function_definition" => {
                            if let Some(name_node) = inner.child_by_field_name("name") {
                                let name = node_text(&name_node, source);
                                push_symbol(symbols, name, SymbolKind::Function, &child, source);
                            }
                        }
                        "class_definition" => {
                            if let Some(name_node) = inner.child_by_field_name("name") {
                                let name = node_text(&name_node, source);
                                push_symbol(symbols, name, SymbolKind::Class, &child, source);
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}

// ── Rust extraction ───────────────────────────────────────────────────

fn extract_rust_symbols(node: &Node, source: &str, symbols: &mut Vec<Symbol>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_item" => {
                if let Some(name_node) = child.child_by_field_name("name") {
                    let name = node_text(&name_node, source);
                    let kind = if source[child.byte_range()].contains("async") {
                        SymbolKind::AsyncFunction
                    } else {
                        SymbolKind::Function
                    };
                    push_symbol(symbols, name, kind, &child, source);
                }
            }
            "struct_item" => {
                if let Some(name_node) = child.child_by_field_name("name") {
                    let name = node_text(&name_node, source);
                    push_symbol(symbols, name, SymbolKind::Struct, &child, source);
                }
            }
            "enum_item" => {
                if let Some(name_node) = child.child_by_field_name("name") {
                    let name = node_text(&name_node, source);
                    push_symbol(symbols, name, SymbolKind::Enum, &child, source);
                }
            }
            "trait_item" => {
                if let Some(name_node) = child.child_by_field_name("name") {
                    let name = node_text(&name_node, source);
                    push_symbol(symbols, name, SymbolKind::Trait, &child, source);
                }
            }
            "impl_item" => {
                if let Some(name_node) = child.child_by_field_name("type") {
                    let name = node_text(&name_node, source);
                    push_symbol(symbols, name, SymbolKind::Impl, &child, source);
                }
            }
            _ => {}
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────

fn node_text(node: &Node, source: &str) -> String {
    source[node.byte_range()].to_string()
}

fn push_symbol(symbols: &mut Vec<Symbol>, name: String, kind: SymbolKind, node: &Node, source: &str) {
    let start_line = node.start_position().row + 1;
    let end_line = node.end_position().row + 1;
    let code = source[node.byte_range()].to_string();

    // Build a signature (first line of the code block)
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
