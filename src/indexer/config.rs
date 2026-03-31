/// Centralized configuration for file discovery, directory filtering, and language support.
///
/// All skip-lists and extension-lists live here so there is a single source of truth.

/// Directories to skip during file walking, indexing, and architecture scanning.
pub const SKIP_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    ".next",
    "dist",
    "build",
    "target",
    "coverage",
    "__pycache__",
    ".venv",
    "venv",
    ".DS_Store",
];

/// Source file extensions recognized by the project.
pub const SOURCE_EXTENSIONS: &[&str] = &[
    "js", "jsx", "ts", "tsx", "mjs", "cjs",
    "py", "pyi",
    "rs",
    "go",
    "java", "kt", "scala",
    "rb",
    "php",
    "c", "cpp", "cc", "h", "hpp",
    "cs",
    "swift",
    "json", "yaml", "yml", "toml",
    "md", "txt",
    "css", "scss", "less",
    "html", "vue", "svelte",
    "sql",
    "sh", "bash", "zsh",
    "dockerfile",
];

/// Extensions with tree-sitter AST support for symbol extraction.
pub const AST_EXTENSIONS: &[&str] = &[
    "js", "jsx", "ts", "tsx", "mjs", "cjs",
    "py", "pyi",
    "rs",
    "go",
    "c", "h", "cpp", "cc", "cxx", "hpp", "hxx",
];

/// Check if a directory name should be skipped during traversal.
pub fn is_skip_dir(name: &str) -> bool {
    SKIP_DIRS.contains(&name)
}

/// Check if a file extension is a recognized source file.
pub fn is_source_file(ext: &str) -> bool {
    SOURCE_EXTENSIONS.contains(&ext)
}

/// Check if a file extension has tree-sitter AST support.
pub fn has_ast_support(ext: &str) -> bool {
    AST_EXTENSIONS.contains(&ext)
}
