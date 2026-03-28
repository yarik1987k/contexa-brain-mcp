#!/bin/bash
#
# Context Brain — Test Script
# Usage: ./test_context_brain.sh /path/to/your/project
#
# Tests all 6 tools and measures token savings.
# Takes ~30 seconds on first run (downloads 23MB embedding model).

set -e

PROJECT="${1:-.}"
CB="$(dirname "$0")/../target/release/context-brain"

if [ ! -f "$CB" ]; then
    echo "ERROR: Binary not found at $CB"
    echo "Run: cargo build --release"
    exit 1
fi

PROJECT=$(cd "$PROJECT" && pwd)
PASS=0
FAIL=0
TOTAL=6

echo ""
echo "============================================================"
echo "  CONTEXT BRAIN TEST SUITE"
echo "============================================================"
echo "  Project: $PROJECT"
echo "  Binary:  $CB"
echo "============================================================"
echo ""

# ── Test 1: list_files ─────────────────────────────────────────
echo "TEST 1/6: list_files"
output=$("$CB" list --project "$PROJECT" --depth 1 2>/dev/null)
if echo "$output" | grep -q "files total"; then
    file_count=$(echo "$output" | grep "files total" | grep -o '[0-9]*')
    echo "  PASS — found $file_count files"
    PASS=$((PASS + 1))
else
    echo "  FAIL — no file listing returned"
    FAIL=$((FAIL + 1))
fi
echo ""

# ── Test 2: get_file_context (summary mode) ───────────────────
echo "TEST 2/6: get_file_context (summary mode)"
# Find a code file to test with
test_file=$(find "$PROJECT" -maxdepth 3 -type f \( -name "*.js" -o -name "*.ts" -o -name "*.py" -o -name "*.rs" \) 2>/dev/null | head -1)
if [ -z "$test_file" ]; then
    test_file=$(find "$PROJECT" -maxdepth 3 -type f -name "*.json" 2>/dev/null | head -1)
fi

if [ -n "$test_file" ]; then
    full_chars=$(wc -c < "$test_file")
    summary=$("$CB" summary --file "$test_file" --mode summary 2>/dev/null)
    summary_chars=$(echo "$summary" | wc -c)

    if [ "$summary_chars" -gt 10 ]; then
        savings=$(( (full_chars - summary_chars) * 100 / full_chars ))
        echo "  PASS — $(basename "$test_file"): ${full_chars} chars → ${summary_chars} chars (${savings}% savings)"
        PASS=$((PASS + 1))
    else
        echo "  FAIL — empty summary returned"
        FAIL=$((FAIL + 1))
    fi
else
    echo "  SKIP — no code files found in project"
    FAIL=$((FAIL + 1))
fi
echo ""

# ── Test 3: search_codebase ───────────────────────────────────
echo "TEST 3/6: search_codebase"
output=$("$CB" search --query "main entry point" --project "$PROJECT" --max-results 3 2>/dev/null)
if echo "$output" | grep -q "results for"; then
    result_count=$(echo "$output" | head -1 | grep -o '^Found [0-9]*' | grep -o '[0-9]*')
    echo "  PASS — found ${result_count:-0} results for 'main entry point'"
    PASS=$((PASS + 1))
else
    echo "  FAIL — search returned no results"
    FAIL=$((FAIL + 1))
fi
echo ""

# ── Test 4: save_memory ──────────────────────────────────────
echo "TEST 4/6: save_memory"
output=$("$CB" remember \
    --content "TEST: This project was tested with Context Brain on $(date '+%Y-%m-%d')" \
    --category decision \
    --project "$PROJECT" 2>/dev/null)
if echo "$output" | grep -qi "saved"; then
    echo "  PASS — memory saved successfully"
    PASS=$((PASS + 1))
else
    echo "  FAIL — memory save failed"
    FAIL=$((FAIL + 1))
fi
echo ""

# ── Test 5: recall_memory ────────────────────────────────────
echo "TEST 5/6: recall_memory"
output=$("$CB" recall --query "Context Brain test" --project "$PROJECT" 2>/dev/null)
if echo "$output" | grep -qi "tested with Context Brain"; then
    echo "  PASS — memory recalled successfully"
    PASS=$((PASS + 1))
else
    echo "  FAIL — memory recall failed"
    echo "  Output: $output"
    FAIL=$((FAIL + 1))
fi
echo ""

# ── Test 6: Token savings benchmark ─────────────────────────
echo "TEST 6/6: Token savings benchmark"

# Find 3 largest code files
largest_files=$(find "$PROJECT" -maxdepth 4 \( -name "*.js" -o -name "*.ts" -o -name "*.py" -o -name "*.rs" \) -type f 2>/dev/null | \
    xargs wc -c 2>/dev/null | sort -rn | head -4 | tail -3 | awk '{print $2}')

total_full=0
total_summary=0

for f in $largest_files; do
    if [ -f "$f" ]; then
        full_chars=$(wc -c < "$f")
        summary_chars=$("$CB" summary --file "$f" --mode summary 2>/dev/null | wc -c)
        total_full=$((total_full + full_chars / 4))
        total_summary=$((total_summary + summary_chars / 4))
    fi
done

if [ "$total_full" -gt 0 ]; then
    saved=$((total_full - total_summary))
    pct=$((saved * 100 / total_full))
    echo "  PASS — 3 largest files: ${total_full} tokens (full) → ${total_summary} tokens (summary) = ${pct}% savings"
    PASS=$((PASS + 1))
else
    echo "  SKIP — could not find code files for benchmark"
    FAIL=$((FAIL + 1))
fi

echo ""
echo "============================================================"
echo "  RESULTS: ${PASS}/${TOTAL} passed, ${FAIL}/${TOTAL} failed"
echo "============================================================"
echo ""

if [ "$PASS" -eq "$TOTAL" ]; then
    echo "  ALL TESTS PASSED"
    echo ""
    echo "  Context Brain is working correctly."
    echo "  Add it to your editor (see README.md) to start saving tokens."
else
    echo "  SOME TESTS FAILED"
    echo "  Check the output above for details."
fi
echo ""
