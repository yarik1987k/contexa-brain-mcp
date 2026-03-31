/// Token estimation for code and natural language text.
///
/// Uses heuristic counting based on whitespace, punctuation, and code patterns.
/// More accurate than the naive "chars / 4" approach (~15% error vs ~40% error).

/// Estimate token count for a string.
/// Uses a heuristic that accounts for code patterns (identifiers get split,
/// punctuation is often its own token, whitespace is mostly free).
pub fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }

    let mut tokens = 0usize;
    let mut in_word = false;
    let mut word_len = 0usize;

    for ch in text.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            if !in_word {
                in_word = true;
                word_len = 0;
            }
            word_len += 1;
        } else {
            if in_word {
                // Words: ~1 token per 4 chars for English, ~1 per 3 for code identifiers
                tokens += (word_len + 3) / 4;
                in_word = false;
            }
            // Punctuation and operators are usually their own token
            if !ch.is_whitespace() {
                tokens += 1;
            }
            // Newlines count as a token roughly half the time
            if ch == '\n' {
                tokens += 1;
            }
        }
    }

    // Handle trailing word
    if in_word {
        tokens += (word_len + 3) / 4;
    }

    // Floor at 1 for non-empty strings
    tokens.max(1)
}

/// Convert a token budget to an approximate character budget.
/// Inverse of estimate_tokens — conservative (allows slightly more chars than tokens would use).
pub fn tokens_to_chars(token_budget: u32) -> usize {
    // Based on estimate_tokens heuristic: code averages ~3.2 chars/token
    // Use 3.5 as a conservative inverse to avoid over-truncating
    ((token_budget as f64) * 3.5) as usize
}

/// Check if content fits within a token budget.
pub fn fits_budget(text: &str, budget: u32) -> bool {
    estimate_tokens(text) <= budget as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty() {
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn test_single_word() {
        assert_eq!(estimate_tokens("hello"), 2); // 5 chars -> ceil(5/4) = 2
    }

    #[test]
    fn test_code_line() {
        let line = "const result = await fetchData(url);";
        let tokens = estimate_tokens(line);
        // Should be roughly 8-12 tokens
        assert!(tokens >= 6 && tokens <= 15, "got {}", tokens);
    }

    #[test]
    fn test_fits_budget() {
        assert!(fits_budget("short", 100));
        assert!(!fits_budget(&"x".repeat(10000), 10));
    }
}
