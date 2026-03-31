#!/bin/bash
# Benchmark: Compare token output sizes across context-brain modes
set -e

CB="./target/release/context-brain"
PROJECT="../lead-gen-system"

echo "=========================================="
echo "  Context Brain Token Optimization Benchmark"
echo "=========================================="
echo ""

count_tokens() {
    local chars
    chars=$(echo -n "$1" | wc -c | tr -d ' ')
    echo $((chars / 4))
}

echo "── TEST 1: siteProcessor.js (296 lines) ──"
full=$($CB summary --file "$PROJECT/src/siteProcessor.js" --mode full 2>/dev/null)
summ=$($CB summary --file "$PROJECT/src/siteProcessor.js" --mode summary 2>/dev/null)
syms=$($CB summary --file "$PROJECT/src/siteProcessor.js" --mode symbols 2>/dev/null)
ft=$(count_tokens "$full"); st=$(count_tokens "$summ"); yt=$(count_tokens "$syms")
echo "  full:    ~${ft} tokens"
echo "  summary: ~${st} tokens (saves $(( (ft - st) * 100 / ft ))%)"
echo "  symbols: ~${yt} tokens (saves $(( (ft - yt) * 100 / ft ))%)"
echo ""

echo "── TEST 2: googleScraper.js (204 lines) ──"
full=$($CB summary --file "$PROJECT/src/googleScraper.js" --mode full 2>/dev/null)
summ=$($CB summary --file "$PROJECT/src/googleScraper.js" --mode summary 2>/dev/null)
syms=$($CB summary --file "$PROJECT/src/googleScraper.js" --mode symbols 2>/dev/null)
ft2=$(count_tokens "$full"); st2=$(count_tokens "$summ"); yt2=$(count_tokens "$syms")
echo "  full:    ~${ft2} tokens"
echo "  summary: ~${st2} tokens (saves $(( (ft2 - st2) * 100 / ft2 ))%)"
echo "  symbols: ~${yt2} tokens (saves $(( (ft2 - yt2) * 100 / ft2 ))%)"
echo ""

echo "── TEST 3: dataManager.js (103 lines) ──"
full=$($CB summary --file "$PROJECT/src/dataManager.js" --mode full 2>/dev/null)
summ=$($CB summary --file "$PROJECT/src/dataManager.js" --mode summary 2>/dev/null)
syms=$($CB summary --file "$PROJECT/src/dataManager.js" --mode symbols 2>/dev/null)
ft3=$(count_tokens "$full"); st3=$(count_tokens "$summ"); yt3=$(count_tokens "$syms")
echo "  full:    ~${ft3} tokens"
echo "  summary: ~${st3} tokens (saves $(( (ft3 - st3) * 100 / ft3 ))%)"
echo "  symbols: ~${yt3} tokens (saves $(( (ft3 - yt3) * 100 / ft3 ))%)"
echo ""

echo "── TEST 4: index.js (130 lines) ──"
full=$($CB summary --file "$PROJECT/index.js" --mode full 2>/dev/null)
summ=$($CB summary --file "$PROJECT/index.js" --mode summary 2>/dev/null)
syms=$($CB summary --file "$PROJECT/index.js" --mode symbols 2>/dev/null)
ft4=$(count_tokens "$full"); st4=$(count_tokens "$summ"); yt4=$(count_tokens "$syms")
echo "  full:    ~${ft4} tokens"
echo "  summary: ~${st4} tokens (saves $(( (ft4 - st4) * 100 / ft4 ))%)"
echo "  symbols: ~${yt4} tokens (saves $(( (ft4 - yt4) * 100 / ft4 ))%)"
echo ""

echo "── TEST 5: Search performance ──"
echo "  Indexing project first..."
$CB index --project "$PROJECT" 2>/dev/null
echo ""
search_out=$($CB search --query "processSite" --project "$PROJECT" --max-results 5 2>/dev/null)
search_tok=$(count_tokens "$search_out")
echo "  Search 'processSite': ~${search_tok} tokens"

search_out2=$($CB search --query "email extraction" --project "$PROJECT" --max-results 5 2>/dev/null)
search_tok2=$(count_tokens "$search_out2")
echo "  Search 'email extraction': ~${search_tok2} tokens"
echo ""

echo "── TEST 6: get_symbol (most efficient) ──"
sym_out=$($CB search --query "processSite" --project "$PROJECT" --max-results 1 2>/dev/null)
sym_tok=$(count_tokens "$sym_out")
echo "  get_symbol 'processSite': ~${sym_tok} tokens"
echo "  vs full siteProcessor.js: ~${ft} tokens"
if [ "$ft" -gt 0 ]; then
    echo "  Savings: $(( (ft - sym_tok) * 100 / ft ))%"
fi
echo ""

echo "══════════════════════════════════════════"
echo "  AGGREGATE: 4-file conversation scenario"
echo "══════════════════════════════════════════"
echo ""
total_full=$((ft + ft2 + ft3 + ft4))
total_summ=$((st + st2 + st3 + st4))
total_syms=$((yt + yt2 + yt3 + yt4))

echo "  4 files × full read:    ~${total_full} tokens"
echo "  4 files × summary:      ~${total_summ} tokens"
echo "  4 files × symbols:      ~${total_syms} tokens"
echo ""
echo "  Summary saves: $(( (total_full - total_summ) * 100 / total_full ))% vs full"
echo "  Symbols saves: $(( (total_full - total_syms) * 100 / total_full ))% vs full"
echo ""

echo "══════════════════════════════════════════"
echo "  OVERHEAD vs SAVINGS"
echo "══════════════════════════════════════════"
echo ""

# Measure MCP tool schema overhead from the actual source code.
# This counts the real tool descriptions + parameter schemas that get sent to the AI.
CB_SRC="$(cd "$(dirname "$0")" && pwd)/src/server.rs"
tool_desc_chars=$(grep 'tool(description' "$CB_SRC" 2>/dev/null | wc -c | tr -d ' ')
param_desc_chars=$(grep 'schemars(description' "$CB_SRC" 2>/dev/null | wc -c | tr -d ' ')
tool_count=$(grep -c '#\[tool(description' "$CB_SRC" 2>/dev/null || echo 0)
# Each tool also contributes: function name, param names/types, JSON schema boilerplate (~50 tokens each)
mcp_overhead=$(( (tool_desc_chars + param_desc_chars) / 4 + tool_count * 50 ))

echo "  MCP tool schema overhead (measured):  ~${mcp_overhead} tokens"
echo "  Savings from 4 files (summary):        $(( total_full - total_summ )) tokens"
net=$(( total_full - total_summ - mcp_overhead ))
if [ "$net" -gt 0 ]; then
    echo ""
    echo "  ✅ NET SAVINGS: ${net} tokens per conversation"
    per_file_save=$(( (total_full - total_summ) / 4 ))
    if [ "$per_file_save" -gt 0 ]; then
        echo "  ✅ Break-even at ~$(( mcp_overhead / per_file_save + 1 )) files"
    fi
else
    echo ""
    echo "  ❌ NET COST: $((-net)) tokens (need more files to break even)"
    per_file_save=$(( (total_full - total_summ) / 4 ))
    if [ "$per_file_save" -gt 0 ]; then
        echo "  Break-even at ~$(( mcp_overhead / per_file_save + 1 )) files"
    fi
fi
echo ""
