#!/bin/bash
# REAL benchmark: MCP vs No-MCP (native Read)
# Measures actual byte/token counts from real file operations
set -e

CB="./target/release/context-brain"
PROJECT="../lead-gen-system"
PROJECT_ABS=$(cd "$PROJECT" && pwd)

echo "╔══════════════════════════════════════════════════════════════╗"
echo "║     REAL BENCHMARK: MCP vs Native Read (No Faking)         ║"
echo "╚══════════════════════════════════════════════════════════════╝"
echo ""
echo "Method: Compare raw byte counts of actual outputs."
echo "        'Native Read' = cat the file (what Claude's Read tool does)."
echo "        'MCP' = context-brain output for same file."
echo "        Token estimate = bytes / 4 (industry standard approximation)."
echo ""
echo "Project: $PROJECT_ABS"
echo "Date:    $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo ""

# ──────────────────────────────────────────────────────
# Helper: get raw byte count (no tricks)
# ──────────────────────────────────────────────────────
bytes() {
    echo -n "$1" | wc -c | tr -d ' '
}

tokens() {
    local b
    b=$(bytes "$1")
    echo $(( b / 4 ))
}

FILES=(
    "src/siteProcessor.js"
    "src/googleScraper.js"
    "src/dataManager.js"
    "index.js"
    "dashboard.js"
    "backfill-emails.js"
    "traffic-backfill.js"
    "whois-backfill.js"
)

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "PART 1: Per-file comparison (Native Read vs MCP summary)"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
printf "%-30s %8s %8s %8s %6s\n" "File" "Native" "MCP sum" "MCP sym" "Saved%"
printf "%-30s %8s %8s %8s %6s\n" "----" "------" "-------" "-------" "------"

total_native=0
total_summary=0
total_symbols=0

for f in "${FILES[@]}"; do
    # NATIVE READ: just the raw file content (exactly what Claude's Read tool returns)
    native_content=$(cat "$PROJECT/$f")
    native_tok=$(tokens "$native_content")

    # MCP SUMMARY: what context-brain returns in summary mode
    mcp_summary=$($CB summary --file "$PROJECT/$f" --mode summary 2>/dev/null)
    summary_tok=$(tokens "$mcp_summary")

    # MCP SYMBOLS: compact mode
    mcp_symbols=$($CB summary --file "$PROJECT/$f" --mode symbols 2>/dev/null)
    symbols_tok=$(tokens "$mcp_symbols")

    saved_pct=0
    if [ "$native_tok" -gt 0 ]; then
        saved_pct=$(( (native_tok - summary_tok) * 100 / native_tok ))
    fi

    printf "%-30s %7d  %7d  %7d  %5d%%\n" "$f" "$native_tok" "$summary_tok" "$symbols_tok" "$saved_pct"

    total_native=$((total_native + native_tok))
    total_summary=$((total_summary + summary_tok))
    total_symbols=$((total_symbols + symbols_tok))
done

echo ""
printf "%-30s %7d  %7d  %7d\n" "TOTAL (8 files)" "$total_native" "$total_summary" "$total_symbols"
echo ""

total_saved_summary=$(( (total_native - total_summary) * 100 / total_native ))
total_saved_symbols=$(( (total_native - total_symbols) * 100 / total_native ))
echo "  Native Read total:  ~$total_native tokens"
echo "  MCP summary total:  ~$total_summary tokens (saves ${total_saved_summary}%)"
echo "  MCP symbols total:  ~$total_symbols tokens (saves ${total_saved_symbols}%)"

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "PART 2: Search comparison"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# Scenario: "I need to find the processSite function"
# WITHOUT MCP: grep to find it, then Read the entire file
echo "  Scenario: Find and understand 'processSite'"
echo ""

# Native approach: grep finds it in siteProcessor.js, then Read whole file
native_search=$(cat "$PROJECT/src/siteProcessor.js")
native_search_tok=$(tokens "$native_search")
echo "  Without MCP:"
echo "    Step 1: Grep 'processSite' → finds src/siteProcessor.js (~20 tokens)"
echo "    Step 2: Read entire file   → $native_search_tok tokens"
echo "    Total: ~$((native_search_tok + 20)) tokens"

# MCP approach: search returns pointers, then get_symbol gets just the function
mcp_search=$($CB search --query "processSite" --project "$PROJECT" --max-results 3 2>/dev/null)
mcp_search_tok=$(tokens "$mcp_search")
echo ""
echo "  With MCP:"
echo "    Step 1: search_codebase 'processSite' → $mcp_search_tok tokens"
echo "    (Already includes file location + symbol info + context lines)"
echo "    Total: ~$mcp_search_tok tokens"

search_saved=$(( (native_search_tok + 20 - mcp_search_tok) * 100 / (native_search_tok + 20) ))
echo ""
echo "  Search savings: ${search_saved}%"

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "PART 3: Full conversation simulation"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "  Scenario: 'Fix the email scraping logic'"
echo "  User needs to understand email-related code across the project."
echo ""

# WITHOUT MCP: Read all relevant files fully
echo "  ┌─────────────────────────────────────────────────────┐"
echo "  │ WITHOUT MCP (native Read)                           │"
echo "  └─────────────────────────────────────────────────────┘"

no_mcp_total=0
for f in src/siteProcessor.js backfill-emails.js whois-backfill.js index.js; do
    content=$(cat "$PROJECT/$f")
    tok=$(tokens "$content")
    printf "    Read %-30s → %5d tokens\n" "$f" "$tok"
    no_mcp_total=$((no_mcp_total + tok))
done
echo "    ─────────────────────────────────────────────────"
printf "    Total tool results in context:       %5d tokens\n" "$no_mcp_total"

echo ""
echo "  ┌─────────────────────────────────────────────────────┐"
echo "  │ WITH MCP (context-brain)                            │"
echo "  └─────────────────────────────────────────────────────┘"

# Fixed overhead: tool schemas loaded into every conversation
MCP_SCHEMA_OVERHEAD=2150
printf "    Tool schemas overhead (one-time):    %5d tokens\n" "$MCP_SCHEMA_OVERHEAD"

# Step 1: Search
mcp_search_email=$($CB search --query "email scraping extraction" --project "$PROJECT" --max-results 5 2>/dev/null)
mcp_search_email_tok=$(tokens "$mcp_search_email")
printf "    search_codebase 'email scraping':    %5d tokens\n" "$mcp_search_email_tok"

# Step 2: Read files in summary mode
mcp_total=$MCP_SCHEMA_OVERHEAD
mcp_total=$((mcp_total + mcp_search_email_tok))

for f in src/siteProcessor.js backfill-emails.js whois-backfill.js index.js; do
    summ=$($CB summary --file "$PROJECT/$f" --mode summary 2>/dev/null)
    tok=$(tokens "$summ")
    printf "    summary %-28s → %5d tokens\n" "$f" "$tok"
    mcp_total=$((mcp_total + tok))
done
echo "    ─────────────────────────────────────────────────"
printf "    Total (including schema overhead):   %5d tokens\n" "$mcp_total"

echo ""
echo "  ┌─────────────────────────────────────────────────────┐"
echo "  │ COMPARISON                                          │"
echo "  └─────────────────────────────────────────────────────┘"
echo ""
printf "    Without MCP:  %5d tokens\n" "$no_mcp_total"
printf "    With MCP:     %5d tokens\n" "$mcp_total"

if [ "$no_mcp_total" -gt "$mcp_total" ]; then
    net_saved=$((no_mcp_total - mcp_total))
    net_pct=$((net_saved * 100 / no_mcp_total))
    echo ""
    echo "    ✅ MCP saves $net_saved tokens ($net_pct%) even with schema overhead"
else
    net_cost=$((mcp_total - no_mcp_total))
    net_pct=$((net_cost * 100 / no_mcp_total))
    echo ""
    echo "    ❌ MCP costs $net_cost more tokens ($net_pct% overhead)"
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "PART 4: Verification (prove numbers aren't faked)"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "  Raw file sizes (wc -c):"
for f in "${FILES[@]}"; do
    raw_bytes=$(wc -c < "$PROJECT/$f" | tr -d ' ')
    echo "    $f: $raw_bytes bytes"
done

echo ""
echo "  MCP summary output sizes (wc -c of actual output):"
for f in "${FILES[@]}"; do
    mcp_out=$($CB summary --file "$PROJECT/$f" --mode summary 2>/dev/null)
    mcp_bytes=$(echo -n "$mcp_out" | wc -c | tr -d ' ')
    echo "    $f: $mcp_bytes bytes"
done

echo ""
echo "  SHA256 of native vs MCP (proves content is different):"
native_hash=$(cat "$PROJECT/src/siteProcessor.js" | shasum -a 256 | cut -c1-16)
mcp_hash=$($CB summary --file "$PROJECT/src/siteProcessor.js" --mode summary 2>/dev/null | shasum -a 256 | cut -c1-16)
echo "    siteProcessor.js native:  $native_hash..."
echo "    siteProcessor.js MCP sum: $mcp_hash..."
echo "    (Different hashes prove MCP returns compressed content, not passthrough)"

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "PART 5: Break-even analysis"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

per_file_save=$(( (total_native - total_summary) / ${#FILES[@]} ))
echo "  Average tokens saved per file:     $per_file_save"
echo "  MCP schema overhead per session:   $MCP_SCHEMA_OVERHEAD"
if [ "$per_file_save" -gt 0 ]; then
    breakeven=$(( MCP_SCHEMA_OVERHEAD / per_file_save + 1 ))
    echo "  Break-even point:                  $breakeven files"
    echo ""
    echo "  Conversations reading <$breakeven files: MCP costs more ❌"
    echo "  Conversations reading ≥$breakeven files: MCP saves tokens ✅"
else
    echo "  No savings per file — MCP is not beneficial"
fi
echo ""
